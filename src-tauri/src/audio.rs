#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::models::{
    AppError, AppResult, AudioNode, AudioProfile, AudioSnapshot, ErrorCode, SurroundState,
};
use crate::profiles::{validate_node_name, AppPaths, APP_NODE_PREFIX};
#[cfg(any(target_os = "linux", test))]
use crate::surround;
use crate::surround::HrtfSelection;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedSender;

#[cfg(target_os = "linux")]
use crate::process;
#[cfg(target_os = "linux")]
use crate::profiles::{validate_profile, SYSTEM_HRTF};
#[cfg(target_os = "linux")]
use crate::surround::{
    runtime::{ActiveFilter, OwnedChild},
    RestoreAction, RestoreIntent, RestoreRecord,
};
#[cfg(target_os = "linux")]
use std::time::Duration;

const MAX_ARRAY_BYTES: usize = 16 * 1024 * 1024;
const MAX_OBJECTS: usize = 24_000;
const MAX_REGISTRY_BYTES: usize = 32 * 1024 * 1024;
const CHANNELS_71: [&str; 8] = ["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"];
#[cfg(target_os = "linux")]
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub enum AudioEvent {
    Snapshot(Vec<Value>),
    Changed(Vec<Value>),
    Disconnected,
}

#[derive(Default)]
struct JsonArrayParser {
    buffer: Vec<u8>,
    depth: usize,
    quoted: bool,
    escaped: bool,
}

fn invalid_dump(detail: impl AsRef<str>) -> AppError {
    AppError::new(ErrorCode::InvalidData, "Registro JSON PipeWire non valido.").detail(detail)
}

impl JsonArrayParser {
    fn feed(&mut self, bytes: &[u8]) -> AppResult<Vec<Vec<Value>>> {
        let mut arrays = Vec::new();
        for byte in bytes {
            if self.buffer.is_empty() {
                if byte.is_ascii_whitespace() {
                    continue;
                }
                if *byte != b'[' {
                    return Err(invalid_dump("Atteso un array JSON senza colori."));
                }
            }
            if self.buffer.len() == MAX_ARRAY_BYTES {
                return Err(invalid_dump("Array oltre il limite di 16 MiB."));
            }
            self.buffer.push(*byte);
            if self.quoted {
                if self.escaped {
                    self.escaped = false;
                } else if *byte == b'\\' {
                    self.escaped = true;
                } else if *byte == b'"' {
                    self.quoted = false;
                }
                continue;
            }
            match byte {
                b'"' => self.quoted = true,
                b'[' | b'{' => {
                    self.depth += 1;
                    if self.depth > 128 {
                        return Err(invalid_dump("JSON troppo annidato."));
                    }
                }
                b']' | b'}' => {
                    self.depth = self
                        .depth
                        .checked_sub(1)
                        .ok_or_else(|| invalid_dump("Chiusura JSON inattesa."))?;
                    if self.depth == 0 {
                        let array: Vec<Value> = serde_json::from_slice(&self.buffer)
                            .map_err(|error| invalid_dump(error.to_string()))?;
                        if array.len() > MAX_OBJECTS {
                            return Err(invalid_dump("Troppi oggetti PipeWire."));
                        }
                        arrays.push(array);
                        self.buffer.clear();
                    }
                }
                _ => {}
            }
        }
        Ok(arrays)
    }

    fn finish(&self) -> AppResult<()> {
        if self.buffer.is_empty() {
            Ok(())
        } else {
            Err(invalid_dump("Array JSON incompleto."))
        }
    }
}

fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value.as_str().and_then(|text| {
            text.strip_prefix("0x").map_or_else(
                || text.parse().ok(),
                |hex| u64::from_str_radix(hex, 16).ok(),
            )
        })
    })
}

fn id(value: &Value) -> Option<u32> {
    number(value).and_then(|value| value.try_into().ok())
}

fn props(object: &Value) -> Option<&Map<String, Value>> {
    object
        .get("info")
        .and_then(|info| info.get("props"))
        .and_then(Value::as_object)
        .or_else(|| object.get("props").and_then(Value::as_object))
}

fn property<'a>(object: &'a Value, key: &str) -> Option<&'a Value> {
    props(object)?.get(key)
}
fn text_property<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    property(object, key)?.as_str()
}
fn serial(object: &Value) -> Option<u64> {
    property(object, "object.serial").and_then(number)
}
fn is_type(object: &Value, kind: &str) -> bool {
    object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|name| name.ends_with(kind))
}
fn truth(value: &Value) -> bool {
    value
        .as_bool()
        .unwrap_or_else(|| value.as_str() == Some("true"))
}

fn merge_json(current: &mut Value, patch: &Value) {
    match (current, patch) {
        (Value::Object(current), Value::Object(patch)) => {
            for (key, value) in patch {
                if value.is_null() {
                    current.remove(key);
                } else {
                    merge_json(current.entry(key.clone()).or_insert(Value::Null), value);
                }
            }
        }
        (current, patch) => *current = patch.clone(),
    }
}

fn merge_metadata(current: &mut Value, patch: &Value) -> bool {
    let Some(updates) = patch.as_array() else {
        return true;
    };
    if updates.is_empty() {
        return true;
    }
    let entries = current.as_array_mut();
    let Some(entries) = entries else {
        *current = Value::Array(updates.clone());
        return false;
    };
    for update in updates {
        let subject = update.get("subject").and_then(number);
        let key = update.get("key").and_then(Value::as_str);
        entries.retain(|entry| {
            !(entry.get("subject").and_then(number) == subject
                && (key.is_none() || entry.get("key").and_then(Value::as_str) == key))
        });
        if key.is_some() && update.get("value").is_some_and(|value| !value.is_null()) {
            entries.push(update.clone());
        }
    }
    false
}

#[derive(Clone, Default)]
struct Registry {
    objects: BTreeMap<u32, Value>,
    removed_serials: BTreeMap<u32, u64>,
}

impl Registry {
    fn cookie(&self) -> Option<u64> {
        self.objects
            .values()
            .find(|object| is_type(object, ":Core"))?
            .get("info")?
            .get("cookie")
            .and_then(number)
    }

    fn replace(&mut self, objects: Vec<Value>) -> AppResult<()> {
        let mut replacement = Self::default();
        replacement.merge(objects)?;
        if self.cookie().is_some() && self.cookie() == replacement.cookie() {
            replacement.removed_serials = self.removed_serials.clone();
            for (object_id, object) in &self.objects {
                if let Some(old_serial) = serial(object) {
                    if replacement.objects.get(object_id).and_then(serial) != Some(old_serial) {
                        replacement
                            .removed_serials
                            .entry(*object_id)
                            .and_modify(|old| *old = (*old).max(old_serial))
                            .or_insert(old_serial);
                    }
                }
            }
            replacement.objects.retain(|object_id, object| {
                !serial(object).is_some_and(|serial| {
                    replacement
                        .removed_serials
                        .get(object_id)
                        .is_some_and(|removed| serial <= *removed)
                })
            });
        }
        *self = replacement;
        Ok(())
    }

    fn merge(&mut self, objects: Vec<Value>) -> AppResult<bool> {
        if objects.len() > MAX_OBJECTS {
            return Err(invalid_dump("Troppi oggetti."));
        }
        let mut next = self.clone();
        let mut resync = false;
        for object in objects {
            let object_id = object
                .get("id")
                .and_then(id)
                .ok_or_else(|| invalid_dump("ID oggetto mancante."))?;
            if !object.is_object() {
                return Err(invalid_dump("Oggetto non valido."));
            }
            let incoming_serial = serial(&object);
            let current_serial = next.objects.get(&object_id).and_then(serial);
            let removal = object.get("info").is_some_and(Value::is_null)
                || object.get("props").is_some_and(Value::is_null);
            if removal {
                if incoming_serial
                    .zip(current_serial)
                    .is_some_and(|(incoming, current)| incoming < current)
                {
                    continue;
                }
                resync |= incoming_serial.is_none();
                if let Some(serial) = incoming_serial.or(current_serial) {
                    next.removed_serials
                        .entry(object_id)
                        .and_modify(|old| *old = (*old).max(serial))
                        .or_insert(serial);
                }
                next.invalidate_dependencies(object_id);
                next.objects.remove(&object_id);
                continue;
            }
            if incoming_serial.is_some_and(|incoming| {
                next.removed_serials
                    .get(&object_id)
                    .is_some_and(|removed| incoming <= *removed)
            }) {
                continue;
            }
            if incoming_serial
                .zip(current_serial)
                .is_some_and(|(incoming, current)| incoming < current)
            {
                continue;
            }
            let recycled = incoming_serial
                .zip(current_serial)
                .is_some_and(|(incoming, current)| incoming != current);
            if recycled {
                next.invalidate_dependencies(object_id);
                next.objects.remove(&object_id);
            }
            let current = next
                .objects
                .entry(object_id)
                .or_insert_with(|| serde_json::json!({}));
            let mut patch = object.clone();
            let metadata = patch.as_object_mut().and_then(|map| map.remove("metadata"));
            merge_json(current, &patch);
            if let Some(metadata) = metadata {
                if !current.get("metadata").is_some_and(Value::is_array) {
                    current["metadata"] = serde_json::json!([]);
                }
                merge_metadata(&mut current["metadata"], &metadata);
                // pw-dump can omit deleted metadata keys even in a nonempty update.
                resync = true;
            }
        }
        if next.objects.len() > MAX_OBJECTS
            || serde_json::to_vec(&next.objects)
                .map_err(|error| invalid_dump(error.to_string()))?
                .len()
                > MAX_REGISTRY_BYTES
        {
            return Err(invalid_dump("Registro oltre i limiti."));
        }
        while next.removed_serials.len() > MAX_OBJECTS {
            if let Some(oldest) = next
                .removed_serials
                .iter()
                .min_by_key(|(_, serial)| *serial)
                .map(|(id, _)| *id)
            {
                next.removed_serials.remove(&oldest);
                resync = true;
            }
        }
        *self = next;
        Ok(resync)
    }

    fn invalidate_dependencies(&mut self, removed_id: u32) {
        let node = self
            .objects
            .get(&removed_id)
            .is_some_and(|object| is_type(object, ":Node"));
        let port = self
            .objects
            .get(&removed_id)
            .is_some_and(|object| is_type(object, ":Port"));
        if !node && !port {
            return;
        }
        let descendants: Vec<u32> = self
            .objects
            .iter()
            .filter_map(|(object_id, object)| {
                let dependent_port = node
                    && is_type(object, ":Port")
                    && property(object, "node.id").and_then(id) == Some(removed_id);
                let info = object.get("info");
                let dependent_link = is_type(object, ":Link")
                    && info.is_some_and(|info| {
                        let fields = if node {
                            ["input-node-id", "output-node-id"]
                        } else {
                            ["input-port-id", "output-port-id"]
                        };
                        fields
                            .iter()
                            .any(|key| info.get(key).and_then(id) == Some(removed_id))
                    });
                (dependent_port || dependent_link).then_some(*object_id)
            })
            .collect();
        for descendant in descendants {
            if let Some(serial) = self.objects.get(&descendant).and_then(serial) {
                self.removed_serials.insert(descendant, serial);
            }
            self.objects.remove(&descendant);
        }
    }

    fn defaults(&self) -> (Option<String>, Option<String>, Option<String>) {
        let metadata = self.objects.values().find(|object| {
            is_type(object, ":Metadata")
                && text_property(object, "metadata.name") == Some("default")
        });
        let entries = metadata
            .and_then(|metadata| metadata.get("metadata"))
            .and_then(Value::as_array);
        let lookup = |key: &str| -> Option<String> {
            let entry = entries?.iter().rev().find(|entry| {
                entry.get("subject").and_then(number) == Some(0)
                    && entry.get("key").and_then(Value::as_str) == Some(key)
            })?;
            let value = entry.get("value")?;
            let json = match value.as_str() {
                Some(text) => serde_json::from_str::<Value>(text).ok()?,
                None => value.clone(),
            };
            let name = json.get("name")?.as_str()?;
            validate_node_name(name).ok()?;
            Some(name.into())
        };
        (
            lookup("default.audio.sink"),
            lookup("default.configured.audio.sink"),
            lookup("default.audio.source"),
        )
    }

    fn node(&self, name: &str) -> AppResult<AudioNode> {
        validate_node_name(name)?;
        let mut matches = self.objects.iter().filter(|(_, object)| {
            is_type(object, ":Node") && text_property(object, "node.name") == Some(name)
        });
        let (node_id, object) = matches.next().ok_or_else(|| {
            AppError::new(ErrorCode::TargetUnavailable, "Nodo audio non disponibile.").detail(name)
        })?;
        if matches.next().is_some() {
            return Err(AppError::new(
                ErrorCode::TargetUnavailable,
                "Nome audio ambiguo; operazione bloccata.",
            ));
        }
        Ok(self.audio_node(*node_id, object))
    }

    fn nodes(&self, output: bool) -> Vec<AudioNode> {
        let class = if output { "Audio/Sink" } else { "Audio/Source" };
        let mut nodes: Vec<_> = self
            .objects
            .iter()
            .filter(|(_, object)| {
                is_type(object, ":Node")
                    && text_property(object, "media.class")
                        .is_some_and(|value| value.starts_with(class))
            })
            .map(|(node_id, object)| self.audio_node(*node_id, object))
            .collect();
        nodes.sort_by(|left, right| {
            left.description
                .cmp(&right.description)
                .then(left.node_name.cmp(&right.node_name))
        });
        nodes
    }

    fn audio_node(&self, node_id: u32, object: &Value) -> AudioNode {
        let device = property(object, "device.id")
            .and_then(id)
            .and_then(|id| self.objects.get(&id));
        let inherited = |key: &str| -> Option<&Value> {
            property(object, key).or_else(|| device.and_then(|device| property(device, key)))
        };
        let inherited_text = |key: &str| inherited(key).and_then(Value::as_str).map(str::to_owned);
        let node_name = text_property(object, "node.name")
            .unwrap_or_default()
            .to_owned();
        let media_class = text_property(object, "media.class")
            .unwrap_or_default()
            .to_owned();
        let device_api = inherited("device.api").and_then(Value::as_str);
        let known_physical = matches!(device_api, Some("alsa" | "bluez5"))
            || inherited("api.alsa.path").is_some()
            || inherited("api.bluez5.address").is_some();
        let is_virtual = property(object, "node.virtual").is_some_and(truth)
            || media_class.contains("Virtual")
            || node_name.starts_with(APP_NODE_PREFIX)
            || !known_physical;
        let params = object.get("info").and_then(|info| info.get("params"));
        let node_props = params
            .and_then(|params| params.get("Props"))
            .and_then(Value::as_array)
            .and_then(|values| {
                values.iter().find(|props| {
                    props.get("channelVolumes").is_some() || props.get("volume").is_some()
                })
            });
        // WirePlumber changes a device Route where available, not necessarily Node Props.
        let route_props = property(object, "card.profile.device")
            .and_then(number)
            .and_then(|profile_device| {
                device?
                    .get("info")?
                    .get("params")?
                    .get("Route")?
                    .as_array()?
                    .iter()
                    .find(|route| {
                        route.get("device").and_then(number) == Some(profile_device)
                            && route
                                .get("props")
                                .is_some_and(|props| props.get("channelVolumes").is_some())
                    })?
                    .get("props")
            });
        let volume_props = route_props.or(node_props);
        let volume = volume_props
            .and_then(|props| {
                props
                    .get("channelVolumes")
                    .and_then(Value::as_array)
                    .and_then(|volumes| {
                        volumes
                            .iter()
                            .filter_map(Value::as_f64)
                            .filter(|volume| volume.is_finite() && *volume >= 0.0)
                            .reduce(f64::max)
                    })
                    .or_else(|| props.get("volume").and_then(Value::as_f64))
            })
            .filter(|volume| volume.is_finite() && *volume >= 0.0)
            .map(f64::cbrt);
        let muted = volume_props
            .and_then(|props| props.get("mute"))
            .and_then(Value::as_bool)
            .or_else(|| {
                params
                    .and_then(|params| params.get("Props"))
                    .and_then(Value::as_array)
                    .and_then(|values| {
                        values
                            .iter()
                            .find_map(|props| props.get("mute").and_then(Value::as_bool))
                    })
            });
        let channels = params
            .and_then(|params| params.get("Format"))
            .and_then(Value::as_array)
            .and_then(|formats| {
                formats
                    .iter()
                    .find_map(|format| format.get("position").map(channel_names))
            })
            .filter(|channels| !channels.is_empty())
            .or_else(|| {
                property(object, "audio.position")
                    .map(channel_names)
                    .filter(|channels| !channels.is_empty())
            })
            .unwrap_or_else(|| {
                self.port_channels(
                    node_id,
                    if media_class.starts_with("Audio/Sink") {
                        "in"
                    } else {
                        "out"
                    },
                )
            });
        AudioNode {
            id: node_id,
            object_serial: serial(object),
            node_name,
            description: text_property(object, "node.description")
                .or_else(|| text_property(object, "node.nick"))
                .or_else(|| text_property(object, "node.name"))
                .unwrap_or("Nodo audio")
                .into(),
            media_class,
            channels,
            volume,
            muted,
            is_virtual,
            device_serial: inherited_text("device.serial"),
            device_bus_path: inherited_text("device.bus-path")
                .or_else(|| inherited_text("device.bus-id")),
            vendor_id: inherited("device.vendor.id")
                .and_then(number)
                .and_then(|value| value.try_into().ok()),
            product_id: inherited("device.product.id")
                .and_then(number)
                .and_then(|value| value.try_into().ok()),
            transport: inherited_text("device.bus")
                .or_else(|| inherited_text("api.bluez5.profile")),
        }
    }

    fn ports(&self, node_id: u32, direction: &str) -> Vec<(u32, &Value)> {
        self.objects
            .iter()
            .filter(|(_, object)| {
                is_type(object, ":Port")
                    && property(object, "node.id").and_then(id) == Some(node_id)
                    && object
                        .get("info")
                        .and_then(|info| info.get("direction"))
                        .and_then(Value::as_str)
                        .or_else(|| text_property(object, "port.direction"))
                        == Some(direction)
                    && !property(object, "port.monitor").is_some_and(truth)
            })
            .map(|(id, object)| (*id, object))
            .collect()
    }

    fn port_channels(&self, node_id: u32, direction: &str) -> Vec<String> {
        self.ports(node_id, direction)
            .into_iter()
            .filter_map(|(_, port)| text_property(port, "audio.channel").map(str::to_owned))
            .collect()
    }

    fn process_id(&self, node_id: u32) -> Option<u32> {
        let node = self.objects.get(&node_id)?;
        property(node, "application.process.id")
            .and_then(id)
            .or_else(|| {
                let client_id = property(node, "client.id").and_then(id)?;
                property(self.objects.get(&client_id)?, "application.process.id").and_then(id)
            })
    }
}

fn channel_names(value: &Value) -> Vec<String> {
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Value::String(text) => text
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(|character: char| character.is_whitespace() || character == ',')
            .map(|name| name.trim_matches('"'))
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn same_channels(actual: &[String], expected: &[&str]) -> bool {
    actual.len() == expected.len()
        && actual.iter().map(String::as_str).collect::<BTreeSet<_>>()
            == expected.iter().copied().collect()
}

fn stereo_target(node: &AudioNode) -> AppResult<()> {
    if node.is_virtual
        || !node.media_class.starts_with("Audio/Sink")
        || node.object_serial.is_none()
    {
        return Err(AppError::new(
            ErrorCode::TargetUnavailable,
            "Selezionare un'uscita fisica identificabile, non un filtro virtuale.",
        ));
    }
    if !same_channels(&node.channels, &["FL", "FR"]) {
        return Err(AppError::new(ErrorCode::TargetUnavailable,
            "Il 7.1 binaurale richiede un'uscita stereo FL/FR. Mono/HFP resta disponibile senza surround; nessun profilo Bluetooth viene cambiato."));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn verify_graph(registry: &Registry, record: &surround::RestoreRecord) -> AppResult<bool> {
    if record.core_cookie.is_some() && registry.cookie() != record.core_cookie {
        return Err(AppError::new(
            ErrorCode::AudioUnavailable,
            "PipeWire e' stato riavviato; la vecchia sessione surround non viene riattivata.",
        ));
    }
    let target = registry.node(&record.selected_physical_target)?;
    stereo_target(&target)?;
    if target.object_serial != Some(record.target_serial) {
        return Err(AppError::new(
            ErrorCode::TargetUnavailable,
            "Il target e' stato scollegato o il suo ID e' stato riciclato.",
        ));
    }
    let (input, playback) = match (
        registry.node(&record.virtual_node_name),
        registry.node(&record.playback_node_name),
    ) {
        (Ok(input), Ok(playback)) => (input, playback),
        _ => return Ok(false),
    };
    for node in [&input, &playback] {
        let object = &registry.objects[&node.id];
        if !node.is_virtual
            || text_property(object, "peripheral-hub.session") != Some(&record.session_id)
            || registry.process_id(node.id) != record.filter_process_id
            || node.object_serial.is_none()
            || object
                .get("info")
                .and_then(|info| info.get("state"))
                .and_then(Value::as_str)
                == Some("error")
        {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Il nodo osservato non appartiene al client surround atteso.",
            ));
        }
    }
    if record
        .virtual_object_serial
        .is_some_and(|serial| input.object_serial != Some(serial))
        || record
            .playback_object_serial
            .is_some_and(|serial| playback.object_serial != Some(serial))
    {
        return Err(AppError::new(
            ErrorCode::NotConfirmed,
            "Identita del filtro cambiata.",
        ));
    }
    if input.media_class != "Audio/Sink"
        || playback.media_class != "Stream/Output/Audio"
        || !same_channels(&input.channels, &CHANNELS_71)
        || !same_channels(&playback.channels, &["FL", "FR"])
    {
        return Err(AppError::new(
            ErrorCode::NotConfirmed,
            "Il filtro non espone un vero ingresso 7.1 e uscita stereo.",
        ));
    }
    if !same_channels(&registry.port_channels(input.id, "in"), &CHANNELS_71)
        || !same_channels(&registry.port_channels(playback.id, "out"), &["FL", "FR"])
        || !same_channels(&registry.port_channels(target.id, "in"), &["FL", "FR"])
    {
        return Ok(false);
    }
    let mut linked_channels = BTreeSet::new();
    let mut links = 0;
    let mut adjacency: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for object in registry
        .objects
        .values()
        .filter(|object| is_type(object, ":Link"))
    {
        let Some(info) = object.get("info") else {
            continue;
        };
        let Some(output_node) = info.get("output-node-id").and_then(id) else {
            continue;
        };
        let Some(input_node) = info.get("input-node-id").and_then(id) else {
            continue;
        };
        adjacency.entry(output_node).or_default().push(input_node);
        if output_node != playback.id {
            continue;
        }
        if input_node != target.id {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Collegamento del filtro a un target diverso o virtuale: attivazione annullata.",
            ));
        }
        links += 1;
        if !matches!(
            info.get("state").and_then(Value::as_str),
            Some("active" | "paused")
        ) {
            return Ok(false);
        }
        let output_port = info
            .get("output-port-id")
            .and_then(id)
            .and_then(|id| registry.objects.get(&id));
        let input_port = info
            .get("input-port-id")
            .and_then(id)
            .and_then(|id| registry.objects.get(&id));
        let (Some(output_port), Some(input_port)) = (output_port, input_port) else {
            return Ok(false);
        };
        let channel = text_property(output_port, "audio.channel");
        if property(output_port, "node.id").and_then(id) != Some(playback.id)
            || property(input_port, "node.id").and_then(id) != Some(target.id)
            || channel != text_property(input_port, "audio.channel")
            || !matches!(channel, Some("FL" | "FR"))
        {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Link stereo incoerente.",
            ));
        }
        linked_channels.insert(channel.unwrap_or_default());
    }
    // The DSP graph adds an implicit capture -> playback edge. Reject return paths.
    let mut pending = vec![playback.id];
    let mut visited = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if node == input.id {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Loop audio rilevato; filtro rimosso.",
            ));
        }
        if visited.insert(node) {
            pending.extend(adjacency.get(&node).into_iter().flatten().copied());
        }
    }
    if links > 2 {
        return Err(AppError::new(
            ErrorCode::NotConfirmed,
            "Link duplicati o non isolati.",
        ));
    }
    Ok(links == 2 && linked_channels == BTreeSet::from(["FL", "FR"]))
}

pub struct AudioController {
    paths: AppPaths,
    registry: Registry,
    available: bool,
    selected_output: Option<String>,
    hrtf: Option<HrtfSelection>,
    state: SurroundState,
    reason: Option<String>,
    recovery_pending: bool,
    #[cfg(target_os = "linux")]
    filter: Option<ActiveFilter>,
}

impl AudioController {
    pub fn new(paths: AppPaths) -> Self {
        Self {
            paths,
            registry: Registry::default(),
            available: false,
            selected_output: None,
            hrtf: None,
            state: SurroundState::Off,
            reason: if cfg!(target_os = "linux") {
                None
            } else {
                Some("Il backend audio richiede Linux.".into())
            },
            recovery_pending: true,
            #[cfg(target_os = "linux")]
            filter: None,
        }
    }

    pub fn snapshot(&self) -> AudioSnapshot {
        let (outputs, inputs, effective, configured, default_input) = if self.available {
            let defaults = self.registry.defaults();
            (
                self.registry.nodes(true),
                self.registry.nodes(false),
                defaults.0,
                defaults.1,
                defaults.2,
            )
        } else {
            (Vec::new(), Vec::new(), None, None, None)
        };
        let target = self
            .selected_output
            .as_deref()
            .and_then(|name| outputs.iter().find(|node| node.node_name == name));
        let hrtf = self.hrtf.as_ref().map(|selection| {
            let mut status = selection.status.clone();
            status.readable = selection.readable();
            status
        });
        let target_error = target.map_or_else(
            || Some("Selezionare un'uscita fisica disponibile.".into()),
            |target| stereo_target(target).err().map(|error| error.message),
        );
        let reason = if !self.available {
            Some(if cfg!(target_os = "linux") {
                "PipeWire non disponibile.".into()
            } else {
                "Il backend audio richiede Linux.".into()
            })
        } else if self.reason.is_some() {
            self.reason.clone()
        } else if target_error.is_some() {
            target_error
        } else if !hrtf.as_ref().is_some_and(|hrtf| hrtf.readable) {
            Some("Importare un HRTF SOFA valido o installare libmysofa.".into())
        } else {
            None
        };
        let can_enable_surround = self.available
            && target.is_some_and(|target| stereo_target(target).is_ok())
            && hrtf.as_ref().is_some_and(|hrtf| hrtf.readable)
            && !self.recovery_pending
            && !matches!(
                self.state,
                SurroundState::Starting | SurroundState::Stopping
            );
        AudioSnapshot {
            available: self.available,
            outputs,
            inputs,
            default_output: effective,
            configured_default_output: configured,
            selected_output: self.selected_output.clone(),
            default_input,
            surround_state: self.state,
            can_enable_surround,
            surround_reason: reason,
            hrtf,
        }
    }

    pub async fn refresh(&mut self) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.read_registry().await;
            if result.is_ok() {
                if self.hrtf.is_none() {
                    match surround::resolve_hrtf(&self.paths, SYSTEM_HRTF) {
                        Ok(selection) => {
                            self.hrtf = Some(selection);
                            self.reason = None;
                        }
                        Err(error) => self.reason = Some(error.message),
                    }
                }
                if self.selected_output.is_none() {
                    let defaults = self.registry.defaults();
                    self.selected_output = defaults.0.into_iter().chain(defaults.1).find(|name| {
                        self.registry.node(name).is_ok_and(|node| {
                            !node.is_virtual && node.media_class.starts_with("Audio/Sink")
                        })
                    });
                }
                if self.recovery_pending {
                    self.recover_stale().await?;
                }
                self.enforce_safety().await?;
            } else if self.filter.is_some() {
                let _ = self.stop_filter(RestoreIntent::TargetLost).await;
                self.state = SurroundState::Error;
                self.reason = Some("PipeWire disconnesso: surround fermato, ripristino in attesa della riconnessione.".into());
            }
            result
        }
    }

    pub async fn set_default_output(&mut self, node_name: &str) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = node_name;
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.select_output(node_name).await;
            self.complete(result).await
        }
    }

    pub async fn set_volume(&mut self, node_name: &str, volume: f64) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (node_name, volume);
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.write_volume(node_name, volume).await;
            self.complete(result).await
        }
    }

    pub async fn set_mute(&mut self, node_name: &str, muted: bool) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (node_name, muted);
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.write_mute(node_name, muted).await;
            self.complete(result).await
        }
    }

    pub async fn set_surround(&mut self, enabled: bool) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = enabled;
            self.available = false;
            self.state = SurroundState::Error;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            self.state = if enabled {
                SurroundState::Starting
            } else {
                SurroundState::Stopping
            };
            let result = if enabled {
                self.enable_surround(None).await
            } else {
                self.stop_filter(RestoreIntent::Bypass).await
            };
            if let Err(error) = &result {
                if enabled && self.filter.is_some() {
                    let _ = self.stop_filter(RestoreIntent::Rollback).await;
                }
                self.state = SurroundState::Error;
                self.reason = Some(error.message.clone());
            }
            self.complete(result).await
        }
    }

    pub async fn import_hrtf(&mut self, path: PathBuf) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = match surround::import_hrtf(&self.paths, &path) {
                Ok(selection) => {
                    // A new selection must not be shown as the HRTF of an old running filter.
                    let stopped = if self.filter.is_some() {
                        self.stop_filter(RestoreIntent::Bypass).await
                    } else {
                        Ok(())
                    };
                    if stopped.is_ok() {
                        self.hrtf = Some(selection);
                        self.reason = None;
                    }
                    stopped
                }
                Err(error) => Err(error),
            };
            self.complete(result).await
        }
    }

    pub async fn apply_profile(&mut self, profile: &AudioProfile) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = profile;
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.apply_validated_profile(profile).await;
            if let Err(error) = &result {
                self.reason = Some(error.message.clone());
            } else if !profile.surround_requested {
                self.state = SurroundState::Off;
                self.reason = None;
            }
            self.complete(result).await
        }
    }

    pub async fn shutdown(&mut self) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            self.state = SurroundState::Off;
            Ok(())
        }
        #[cfg(target_os = "linux")]
        {
            self.state = SurroundState::Stopping;
            let result = self.stop_filter(RestoreIntent::Bypass).await;
            if result.is_err() {
                self.state = SurroundState::Error;
            }
            result
        }
    }

    pub async fn handle_event(&mut self, event: AudioEvent) -> AppResult<()> {
        match event {
            AudioEvent::Disconnected => {
                self.available = false;
                #[cfg(target_os = "linux")]
                if self.filter.is_some() {
                    let _ = self.stop_filter(RestoreIntent::TargetLost).await;
                    self.state = SurroundState::Error;
                    self.reason = Some("PipeWire disconnesso: filtro fermato senza cambiare un default non osservabile.".into());
                }
            }
            AudioEvent::Snapshot(objects) => {
                self.registry.replace(objects)?;
                #[cfg(target_os = "linux")]
                {
                    // A monitor snapshot may have queued behind a mutation. Reconcile with a fresh generation.
                    self.read_registry().await?;
                    if self.recovery_pending {
                        self.recover_stale().await?;
                    }
                    self.enforce_safety().await?;
                }
            }
            AudioEvent::Changed(objects) => {
                let resync = self.registry.merge(objects)?;
                #[cfg(not(target_os = "linux"))]
                let _ = resync;
                #[cfg(target_os = "linux")]
                {
                    if resync {
                        self.read_registry().await?;
                    }
                    self.enforce_safety().await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl AudioController {
    async fn read_registry(&mut self) -> AppResult<()> {
        let result = async {
            let output = process::run("pw-dump", &["-N"], COMMAND_TIMEOUT).await?;
            let mut parser = JsonArrayParser::default();
            let mut arrays = parser.feed(&output.stdout)?;
            parser.finish()?;
            if arrays.len() != 1 {
                return Err(invalid_dump("Snapshot completa non ricevuta."));
            }
            self.registry.replace(arrays.remove(0))?;
            Ok(())
        }
        .await;
        self.available = result.is_ok();
        result
    }

    async fn complete(&mut self, result: AppResult<()>) -> AppResult<()> {
        let observation = self.read_registry().await;
        let safety = self.enforce_safety().await;
        match result {
            Err(error) => Err(error),
            Ok(()) => {
                observation?;
                safety
            }
        }
    }

    async fn resolve_current(&mut self, name: &str, output_only: bool) -> AppResult<AudioNode> {
        validate_node_name(name)?;
        self.read_registry().await?;
        let node = self.registry.node(name)?;
        if !(node.media_class.starts_with("Audio/Sink")
            || (!output_only && node.media_class.starts_with("Audio/Source")))
        {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Il nodo non e' un'uscita o ingresso audio controllabile.",
            ));
        }
        Ok(node)
    }

    async fn write_default(
        &mut self,
        name: &str,
        expected: Option<&Option<String>>,
    ) -> AppResult<()> {
        let node = self.resolve_current(name, true).await?;
        let previous = self.registry.defaults().1;
        if expected.is_some_and(|expected| *expected != previous) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Scelta esterna del default conservata; transazione annullata.",
            ));
        }
        if name.starts_with(APP_NODE_PREFIX) {
            let record = surround::read_restore(&self.paths)?.ok_or_else(|| {
                AppError::new(
                    ErrorCode::NotConfirmed,
                    "Il filtro non dispone di uno stato di ripristino valido.",
                )
            })?;
            if record.virtual_node_name != name
                || record.app_process_id != std::process::id()
                || !verify_graph(&self.registry, &record)?
            {
                return Err(AppError::new(
                    ErrorCode::NotConfirmed,
                    "Il filtro non e' verificato subito prima del cambio default.",
                ));
            }
        }
        process::run(
            "wpctl",
            &["set-default", &node.id.to_string()],
            COMMAND_TIMEOUT,
        )
        .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            self.read_registry().await?;
            if self.registry.node(name)?.object_serial != node.object_serial {
                return Err(AppError::new(
                    ErrorCode::NotConfirmed,
                    "Identita del target cambiata durante il cambio default.",
                ));
            }
            let (effective, configured, _) = self.registry.defaults();
            if configured.as_deref() == Some(name)
                && (!name.starts_with(APP_NODE_PREFIX) || effective.as_deref() == Some(name))
            {
                return Ok(());
            }
            if (configured != previous && configured.as_deref() != Some(name))
                || tokio::time::Instant::now() >= deadline
            {
                return Err(AppError::new(
                    ErrorCode::NotConfirmed,
                    "Cambio del default non confermato o superato da una scelta esterna.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn write_volume(&mut self, name: &str, volume: f64) -> AppResult<()> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Volume ammesso da 0 a 1, senza amplificazione fisica.",
            ));
        }
        let node = self.resolve_current(name, false).await?;
        process::run(
            "wpctl",
            &[
                "set-volume",
                &node.id.to_string(),
                &format!("{volume:.6}"),
                "--limit",
                "1.0",
            ],
            COMMAND_TIMEOUT,
        )
        .await?;
        self.read_registry().await?;
        let observed = self.registry.node(name)?;
        if observed.object_serial != node.object_serial
            || !observed
                .volume
                .is_some_and(|actual| (actual - volume).abs() <= 0.002)
        {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Volume non confermato dal nodo o dalla route PipeWire.",
            ));
        }
        Ok(())
    }

    async fn write_mute(&mut self, name: &str, muted: bool) -> AppResult<()> {
        let node = self.resolve_current(name, false).await?;
        process::run(
            "wpctl",
            &[
                "set-mute",
                &node.id.to_string(),
                if muted { "1" } else { "0" },
            ],
            COMMAND_TIMEOUT,
        )
        .await?;
        self.read_registry().await?;
        let observed = self.registry.node(name)?;
        if observed.object_serial != node.object_serial || observed.muted != Some(muted) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Mute non confermato dal nodo PipeWire.",
            ));
        }
        Ok(())
    }

    async fn write_level(&mut self, name: &str, volume: f64, muted: bool) -> AppResult<()> {
        // Mute before a requested level increase; unmute only after the level is confirmed.
        if muted {
            self.write_mute(name, true).await?;
        }
        self.write_volume(name, volume).await?;
        if !muted {
            self.write_mute(name, false).await?;
        }
        Ok(())
    }

    async fn select_output(&mut self, name: &str) -> AppResult<()> {
        self.resolve_current(name, true).await?;
        if name.starts_with(APP_NODE_PREFIX) {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Il nodo 7.1 si seleziona soltanto dopo la verifica del filtro.",
            ));
        }
        if self.filter.is_some() {
            self.stop_filter(RestoreIntent::Bypass).await?;
        }
        self.write_default(name, None).await?;
        self.selected_output = (!self.registry.node(name)?.is_virtual).then(|| name.into());
        self.reason = None;
        Ok(())
    }

    async fn recover_stale(&mut self) -> AppResult<()> {
        let record = match surround::read_restore(&self.paths) {
            Ok(record) => record,
            Err(error) => {
                self.state = SurroundState::Error;
                self.reason = Some(error.message.clone());
                return Err(error);
            }
        };
        if let Some(record) = record {
            // A recovery record is never permission to signal a process by a persisted PID.
            self.restore_default(&record, RestoreIntent::Recovery)
                .await?;
            if self.registry.node(&record.virtual_node_name).is_ok()
                || self.registry.node(&record.playback_node_name).is_ok()
            {
                self.state = SurroundState::Error;
                self.reason = Some("Una vecchia sessione surround e' ancora visibile: nessun PID persistito viene terminato e nessun secondo filtro viene creato.".into());
                return Err(AppError::new(
                    ErrorCode::Busy,
                    self.reason.clone().unwrap_or_default(),
                ));
            }
            surround::clear_restore(&self.paths, &record.session_id)?;
            self.selected_output = Some(record.selected_physical_target.clone());
            let directory = self.paths.runtime_dir.join(&record.session_id);
            let _ = std::fs::remove_file(directory.join("spatializer.conf"));
            let _ = std::fs::remove_dir(directory);
        }
        self.recovery_pending = false;
        self.state = SurroundState::Off;
        Ok(())
    }

    async fn restore_default(
        &mut self,
        record: &RestoreRecord,
        intent: RestoreIntent,
    ) -> AppResult<()> {
        self.read_registry().await?;
        let (_, configured, _) = self.registry.defaults();
        let outputs = self.registry.nodes(true);
        let target_present = outputs.iter().any(|node| {
            node.node_name == record.selected_physical_target
                && !node.is_virtual
                && (node.object_serial == Some(record.target_serial)
                    || (intent == RestoreIntent::Recovery
                        && self.registry.cookie() != record.core_cookie))
        });
        let previous_present = record
            .previous_configured_default
            .as_ref()
            .is_some_and(|name| {
                outputs.iter().any(|node| {
                    &node.node_name == name && !node.node_name.starts_with(APP_NODE_PREFIX)
                })
            });
        let fallback = record
            .previous_configured_default
            .as_ref()
            .and_then(|name| {
                outputs
                    .iter()
                    .find(|node| &node.node_name == name && !node.is_virtual)
            })
            .or_else(|| {
                record.previous_effective_default.as_ref().and_then(|name| {
                    outputs
                        .iter()
                        .find(|node| &node.node_name == name && !node.is_virtual)
                })
            })
            .or_else(|| outputs.iter().find(|node| !node.is_virtual));
        let action = surround::restore_decision(
            record,
            configured.as_deref(),
            target_present,
            previous_present,
            fallback.map(|node| node.node_name.as_str()),
            intent,
        );
        if action == RestoreAction::KeepExternal {
            return Ok(());
        }
        let target_name = match &action {
            RestoreAction::SetOutput(name) => Some(name.as_str()),
            _ => None,
        };
        let node = match target_name {
            Some(name) => Some(self.resolve_current(name, true).await?),
            None => {
                self.read_registry().await?;
                None
            }
        };
        // Observe ownership immediately before the write, not from a previous UI snapshot.
        if self.registry.defaults().1.as_deref() != Some(record.virtual_node_name.as_str()) {
            return Ok(());
        }
        if let Some(node) = node {
            process::run(
                "wpctl",
                &["set-default", &node.id.to_string()],
                COMMAND_TIMEOUT,
            )
            .await?;
        } else {
            process::run("wpctl", &["clear-default", "0"], COMMAND_TIMEOUT).await?;
        }
        self.read_registry().await?;
        if self.registry.defaults().1.as_deref() == Some(record.virtual_node_name.as_str()) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Il ripristino del default audio non e' confermato.",
            ));
        }
        Ok(())
    }

    async fn stop_filter(&mut self, intent: RestoreIntent) -> AppResult<()> {
        self.state = SurroundState::Stopping;
        let Some(mut filter) = self.filter.take() else {
            if self.recovery_pending && self.available {
                self.recover_stale().await?;
            }
            self.state = SurroundState::Off;
            return Ok(());
        };
        // Kill the owned DSP before any fallback. It must never follow a fallback to speakers.
        let stopped = filter.stop().await;
        let restored = self.restore_default(&filter.record, intent).await;
        if restored.is_ok() {
            surround::clear_restore(&self.paths, &filter.record.session_id)?;
        } else {
            self.recovery_pending = true;
        }
        self.state = if stopped.is_ok() && restored.is_ok() {
            SurroundState::Off
        } else {
            SurroundState::Error
        };
        if self.state == SurroundState::Off {
            self.reason = None;
        }
        stopped?;
        restored
    }

    async fn enforce_safety(&mut self) -> AppResult<()> {
        let Some(filter) = self.filter.as_mut() else {
            return Ok(());
        };
        let error = if !self.available {
            Some(AppError::new(
                ErrorCode::AudioUnavailable,
                "PipeWire non disponibile; filtro fermato.",
            ))
        } else if filter.process.has_exited()? {
            Some(
                AppError::new(
                    ErrorCode::AudioUnavailable,
                    "Il client surround e' terminato.",
                )
                .detail(filter.process.stderr()),
            )
        } else if !self.hrtf.as_ref().is_some_and(HrtfSelection::readable) {
            Some(AppError::new(
                ErrorCode::HrtfUnavailable,
                "Il file SOFA selezionato non e' piu leggibile.",
            ))
        } else {
            match verify_graph(&self.registry, &filter.record) {
                Ok(true) => None,
                Ok(false) => Some(AppError::new(
                    ErrorCode::NotConfirmed,
                    "Il filtro ha perso i suoi nodi, le porte o i link stereo.",
                )),
                Err(error) => Some(error),
            }
        };
        if let Some(error) = error {
            let intent = if error.code == ErrorCode::TargetUnavailable {
                RestoreIntent::TargetLost
            } else {
                RestoreIntent::Bypass
            };
            let _ = self.stop_filter(intent).await;
            self.state = SurroundState::Error;
            self.reason = Some(error.message.clone());
            return Err(error);
        }
        Ok(())
    }

    async fn enable_surround(&mut self, level: Option<(f64, bool)>) -> AppResult<()> {
        self.read_registry().await?;
        if self.recovery_pending {
            self.recover_stale().await?;
        }
        let name = self.selected_output.clone().ok_or_else(|| {
            AppError::new(
                ErrorCode::TargetUnavailable,
                "Selezionare un'uscita fisica.",
            )
        })?;
        let target = self.registry.node(&name)?;
        stereo_target(&target)?;
        if !same_channels(&self.registry.port_channels(target.id, "in"), &["FL", "FR"]) {
            return Err(AppError::new(
                ErrorCode::TargetUnavailable,
                "Le porte stereo del target non sono disponibili.",
            ));
        }
        let reference = self
            .hrtf
            .as_ref()
            .map(|selection| selection.status.reference.as_str())
            .unwrap_or(SYSTEM_HRTF);
        let hrtf = surround::resolve_hrtf(&self.paths, reference)?;
        if let Some(filter) = &self.filter {
            if filter.record.selected_physical_target == name
                && filter.hrtf_reference == hrtf.status.reference
            {
                self.enforce_safety().await?;
                if let Some((volume, mute)) = level {
                    let input = self
                        .filter
                        .as_ref()
                        .unwrap()
                        .record
                        .virtual_node_name
                        .clone();
                    let result = self.write_level(&input, volume, mute).await;
                    if result.is_err() {
                        let _ = self.stop_filter(RestoreIntent::Rollback).await;
                    }
                    result?;
                }
                self.state = SurroundState::On;
                self.reason = None;
                return Ok(());
            }
            self.stop_filter(RestoreIntent::Bypass).await?;
        }
        self.state = SurroundState::Starting;
        self.read_registry().await?;
        let (effective, configured, _) = self.registry.defaults();
        let current_target = self.registry.node(&name)?;
        if current_target.object_serial != target.object_serial {
            return Err(AppError::new(
                ErrorCode::TargetUnavailable,
                "Il target e' cambiato durante la preparazione del filtro.",
            ));
        }
        let target = current_target;
        stereo_target(&target)?;
        let mut filter = ActiveFilter::spawn(
            &self.paths,
            &target,
            &hrtf,
            configured,
            effective,
            self.registry.cookie(),
        )
        .await?;
        let result = async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
            loop {
                if filter.process.has_exited()? {
                    return Err(AppError::new(ErrorCode::HrtfUnavailable, "Il filtro SOFA non si e' avviato: verificare il plugin PipeWire/libmysofa.").detail(filter.process.stderr()));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(AppError::new(ErrorCode::Timeout, "Nodi, otto porte di ingresso e link stereo del filtro non osservati in tempo.").detail(filter.process.stderr()));
                }
                self.read_registry().await?;
                if verify_graph(&self.registry, &filter.record)? { break; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            filter.record.virtual_object_serial = self.registry.node(&filter.record.virtual_node_name)?.object_serial;
            filter.record.playback_object_serial = self.registry.node(&filter.record.playback_node_name)?.object_serial;
            surround::write_restore(&self.paths, &filter.record)?;
            if let Some((volume, mute)) = level {
                self.write_level(&filter.record.virtual_node_name, volume, mute).await?;
            }
            self.read_registry().await?;
            if !verify_graph(&self.registry, &filter.record)? { return Err(AppError::new(ErrorCode::NotConfirmed, "Link del filtro persi prima del cambio default.")); }
            // Do not replace a choice made externally during the start transaction.
            if self.registry.defaults().1 != filter.record.previous_configured_default {
                return Err(AppError::new(ErrorCode::NotConfirmed, "Default cambiato da un altro client durante l'attivazione; scelta esterna conservata."));
            }
            self.write_default(&filter.record.virtual_node_name, Some(&filter.record.previous_configured_default)).await?;
            self.read_registry().await?;
            if !verify_graph(&self.registry, &filter.record)? { return Err(AppError::new(ErrorCode::NotConfirmed, "Il collegamento fisico non e' confermato dopo il cambio default.")); }
            if self.registry.defaults().0.as_deref() != Some(filter.record.virtual_node_name.as_str()) {
                return Err(AppError::new(ErrorCode::NotConfirmed, "Il sink 7.1 non e' il default effettivo."));
            }
            Ok(())
        }.await;
        if let Err(error) = result {
            let _ = filter.stop().await;
            if self
                .restore_default(&filter.record, RestoreIntent::Rollback)
                .await
                .is_ok()
            {
                let _ = surround::clear_restore(&self.paths, &filter.record.session_id);
            } else {
                self.recovery_pending = true;
            }
            self.state = SurroundState::Error;
            self.reason = Some(error.message.clone());
            return Err(error);
        }
        self.filter = Some(filter);
        self.hrtf = Some(hrtf);
        self.state = SurroundState::On;
        self.reason = None;
        Ok(())
    }

    async fn apply_validated_profile(&mut self, profile: &AudioProfile) -> AppResult<()> {
        validate_profile(profile)?;
        self.read_registry().await?;
        if self.recovery_pending {
            self.recover_stale().await?;
        }
        let target = self.registry.node(&profile.output_node_name)?;
        if target.is_virtual || !target.media_class.starts_with("Audio/Sink") {
            return Err(AppError::new(
                ErrorCode::TargetUnavailable,
                "Il profilo deve indicare un'uscita fisica disponibile.",
            ));
        }
        let hrtf = profile
            .hrtf
            .as_deref()
            .map(|reference| surround::resolve_hrtf(&self.paths, reference))
            .transpose()?;
        if profile.surround_requested {
            stereo_target(&target)?;
            if !same_channels(&self.registry.port_channels(target.id, "in"), &["FL", "FR"]) {
                return Err(AppError::new(
                    ErrorCode::TargetUnavailable,
                    "Porte stereo del target non disponibili.",
                ));
            }
        }
        // All target/file/schema checks precede the first audio mutation.
        let previous_selected = self.selected_output.clone();
        let previous_hrtf = self.hrtf.clone();
        let previous_default = self.registry.defaults().1;
        self.selected_output = Some(profile.output_node_name.clone());
        if let Some(hrtf) = hrtf {
            self.hrtf = Some(hrtf);
        }
        let result = if profile.surround_requested {
            self.enable_surround(Some((profile.volume, profile.mute)))
                .await
        } else {
            async {
                if self.filter.is_some() {
                    self.stop_filter(RestoreIntent::Bypass).await?;
                }
                self.write_default(&profile.output_node_name, None).await?;
                self.write_level(&profile.output_node_name, profile.volume, profile.mute)
                    .await
            }
            .await
        };
        if result.is_err() {
            self.selected_output = previous_selected;
            self.hrtf = previous_hrtf;
            if !profile.surround_requested
                && self.read_registry().await.is_ok()
                && self.registry.defaults().1.as_deref() == Some(profile.output_node_name.as_str())
            {
                match previous_default.filter(|name| !name.starts_with(APP_NODE_PREFIX)) {
                    Some(name) if self.registry.node(&name).is_ok() => {
                        let expected = Some(profile.output_node_name.clone());
                        let _ = self.write_default(&name, Some(&expected)).await;
                    }
                    _ => {
                        let _ =
                            process::run("wpctl", &["clear-default", "0"], COMMAND_TIMEOUT).await;
                    }
                }
                // Do not restore a higher physical volume after a partial application.
            }
        }
        result
    }
}

pub async fn monitor(sender: UnboundedSender<AudioEvent>) {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = sender.send(AudioEvent::Disconnected);
    }
    #[cfg(target_os = "linux")]
    {
        loop {
            if sender.is_closed() {
                return;
            }
            let result = monitor_once(&sender).await;
            if sender.send(AudioEvent::Disconnected).is_err() {
                return;
            }
            let delay = if result.is_ok() {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(2)
            };
            tokio::select! { _ = sender.closed() => return, _ = tokio::time::sleep(delay) => {} }
        }
    }
}

#[cfg(target_os = "linux")]
async fn monitor_once(sender: &UnboundedSender<AudioEvent>) -> AppResult<()> {
    let mut command = process::owned_command("pw-dump")?;
    command.args(["-m", "-N"]);
    let mut process = OwnedChild::spawn(command)?;
    let mut stdout = process
        .stdout()
        .ok_or_else(|| AppError::new(ErrorCode::Io, "Monitor PipeWire senza stdout."))?;
    let mut parser = JsonArrayParser::default();
    let mut initial = true;
    let mut buffer = [0; 8192];
    let result = async {
        loop {
            let read = tokio::select! {
                _ = sender.closed() => return Ok(()),
                read = stdout.read(&mut buffer) => read,
            };
            let length = read?;
            if length == 0 {
                parser.finish()?;
                return Err(AppError::new(
                    ErrorCode::AudioUnavailable,
                    "Monitor PipeWire terminato.",
                )
                .detail(process.stderr()));
            }
            for array in parser.feed(&buffer[..length])? {
                let event = if initial {
                    initial = false;
                    AudioEvent::Snapshot(array)
                } else {
                    AudioEvent::Changed(array)
                };
                if sender.send(event).is_err() {
                    return Ok(());
                }
            }
        }
    }
    .await;
    let _ = process.terminate().await;
    result
}

#[cfg(target_os = "linux")]
use tokio::io::AsyncReadExt;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Vec<Value> {
        serde_json::from_str(include_str!("../tests/audio_registry.json")).unwrap()
    }

    #[test]
    fn incremental_parser_handles_every_split_and_multiple_arrays() {
        let input = b" [ {\"id\":7,\"name\":\"quoted \\\" ] \\\\ value\",\"nested\":[{},[]]} ]\n[{\"id\":8,\"info\":null}]\n";
        for split in 0..=input.len() {
            let mut parser = JsonArrayParser::default();
            let mut arrays = parser.feed(&input[..split]).unwrap();
            arrays.extend(parser.feed(&input[split..]).unwrap());
            parser.finish().unwrap();
            assert_eq!(arrays.len(), 2);
            assert_eq!(arrays[0][0]["id"], 7);
            assert_eq!(arrays[1][0]["id"], 8);
        }
        let mut parser = JsonArrayParser::default();
        parser.feed(b"[{\"id\":").unwrap();
        assert!(parser.finish().is_err());
        assert!(JsonArrayParser::default().feed(b"\x1b[31m[]").is_err());
    }

    #[test]
    fn parser_limits_memory_and_nesting() {
        let mut parser = JsonArrayParser::default();
        assert!(parser.feed(b"").is_ok());
        assert!(parser.feed(&[b'['; 129]).is_err());
        let mut parser = JsonArrayParser::default();
        parser.feed(b"[\"").unwrap();
        assert!(parser.feed(&vec![b'x'; MAX_ARRAY_BYTES]).is_err());
    }

    #[test]
    fn registry_reads_defaults_device_identity_and_cubic_volumes() {
        let mut registry = Registry::default();
        registry.replace(fixture()).unwrap();
        assert_eq!(
            registry.defaults(),
            (
                Some("alsa_output.headset".into()),
                None,
                Some("alsa_input.microphone".into())
            )
        );
        let node = registry.node("alsa_output.headset").unwrap();
        assert!(!node.is_virtual);
        assert_eq!(node.object_serial, Some(100));
        assert_eq!(node.vendor_id, Some(0x1532));
        assert_eq!(node.device_serial.as_deref(), Some("specific-unit"));
        assert_eq!(node.volume, Some(0.5));
        assert_eq!(node.muted, Some(false));
        assert!(stereo_target(&node).is_ok());
        assert_eq!(registry.nodes(false).len(), 1);
        assert!(registry.node("virtual.other").unwrap().is_virtual);
    }

    #[test]
    fn partial_updates_metadata_removals_and_recycled_ids_do_not_leak_state() {
        let mut registry = Registry::default();
        registry.replace(fixture()).unwrap();
        registry.merge(vec![json!({"id":40,"info":{"params":{"Props":[{"channelVolumes":[0.008,0.008],"mute":true}]}}})]).unwrap();
        let node = registry.node("alsa_output.headset").unwrap();
        assert!((node.volume.unwrap() - 0.2).abs() < 0.000001);
        assert_eq!(node.object_serial, Some(100));
        assert_eq!(node.muted, Some(true));
        registry.merge(vec![json!({"id":21,"metadata":[{"subject":0,"key":"default.configured.audio.sink","value":{"name":"virtual.other"}}]})]).unwrap();
        assert_eq!(registry.defaults().1.as_deref(), Some("virtual.other"));
        assert_eq!(
            registry.defaults().0.as_deref(),
            Some("alsa_output.headset")
        );
        assert!(registry
            .merge(vec![json!({"id":21,"metadata":[]})])
            .unwrap());
        registry.merge(vec![json!({"id":21,"metadata":[{"subject":0,"key":"default.configured.audio.sink","value":null}]})]).unwrap();
        assert!(registry.defaults().1.is_none());
        registry.merge(vec![json!({"id":40,"type":"PipeWire:Interface:Node","info":{"props":{"object.serial":200,"node.name":"replacement","media.class":"Audio/Sink","node.virtual":true}}})]).unwrap();
        assert_eq!(registry.node("replacement").unwrap().volume, None);
        assert!(registry.node("alsa_output.headset").is_err());
        assert!(!registry.objects.contains_key(&50));
        registry
            .merge(vec![
                json!({"id":40,"props":{"object.serial":100},"info":null}),
            ])
            .unwrap();
        assert!(registry.node("replacement").is_ok());
        assert!(registry.merge(vec![json!({"id":40,"info":null})]).unwrap());
        assert!(registry.node("replacement").is_err());
        registry.merge(vec![json!({"id":40,"type":"PipeWire:Interface:Node","info":{"props":{"object.serial":200,"node.name":"replacement"}}})]).unwrap();
        assert!(!registry.objects.contains_key(&40));
    }

    fn graph() -> (Registry, surround::RestoreRecord) {
        let mut registry = Registry::default();
        registry.replace(fixture()).unwrap();
        registry
            .merge(
                serde_json::from_str(include_str!("../tests/audio_surround_graph.json")).unwrap(),
            )
            .unwrap();
        let session = "4ccf058b-1487-4cda-9b5a-11a6033a5ad0";
        let record = surround::RestoreRecord {
            schema_version: 1,
            session_id: session.into(),
            previous_configured_default: None,
            previous_effective_default: Some("alsa_output.headset".into()),
            selected_physical_target: "alsa_output.headset".into(),
            target_serial: 100,
            virtual_node_name: format!("{APP_NODE_PREFIX}input.{session}"),
            playback_node_name: format!("{APP_NODE_PREFIX}playback.{session}"),
            app_process_id: 1000,
            filter_process_id: Some(1001),
            virtual_object_serial: Some(200),
            playback_object_serial: Some(201),
            core_cookie: Some(1),
        };
        (registry, record)
    }

    #[test]
    fn graph_requires_real_eight_to_two_ports_and_physical_only_links() {
        let (mut registry, record) = graph();
        assert!(verify_graph(&registry, &record).unwrap());
        registry.objects.remove(&91);
        assert!(!verify_graph(&registry, &record).unwrap());
        let (mut registry, record) = graph();
        registry.objects.get_mut(&90).unwrap()["info"]["input-node-id"] = json!(42);
        assert!(verify_graph(&registry, &record).is_err());
        let (mut registry, record) = graph();
        registry.objects.get_mut(&40).unwrap()["info"]["props"]["audio.position"] = json!(["MONO"]);
        assert!(verify_graph(&registry, &record).is_err());
        let (mut registry, record) = graph();
        registry.objects.get_mut(&40).unwrap()["info"]["props"]["object.serial"] = json!(999);
        assert!(verify_graph(&registry, &record).is_err());
        let (mut registry, record) = graph();
        registry.objects.insert(92, json!({"id":92,"type":"PipeWire:Interface:Link","info":{"output-node-id":40,"input-node-id":70,"state":"active"}}));
        assert!(verify_graph(&registry, &record).is_err());
    }

    #[test]
    fn failed_registry_merge_is_atomic() {
        let mut registry = Registry::default();
        registry.replace(fixture()).unwrap();
        assert!(registry
            .merge(vec![json!({"id":40,"info":null}), json!({"notAnId":1})])
            .is_err());
        assert!(registry.node("alsa_output.headset").is_ok());
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn nonlinux_runtime_is_explicitly_unsupported_never_demo() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().to_path_buf();
        let mut controller = AudioController::new(AppPaths {
            config_dir: path.clone(),
            data_dir: path.clone(),
            state_dir: path.clone(),
            runtime_dir: path,
        });
        assert_eq!(
            controller.refresh().await.unwrap_err().code,
            ErrorCode::UnsupportedPlatform
        );
        assert!(!controller.snapshot().available);
        assert_eq!(
            controller.set_surround(true).await.unwrap_err().code,
            ErrorCode::UnsupportedPlatform
        );
        assert_ne!(controller.snapshot().surround_state, SurroundState::On);
        controller.shutdown().await.unwrap();
    }
}
