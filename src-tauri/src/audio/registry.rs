use super::graph::channel_names;
use super::parser::{invalid_dump, MAX_OBJECTS};
use crate::models::{AppError, AppResult, AudioNode, ErrorCode};
use crate::profiles::{validate_node_name, APP_NODE_PREFIX};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const MAX_REGISTRY_BYTES: usize = 32 * 1024 * 1024;

pub fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value.as_str().and_then(|text| {
            text.strip_prefix("0x").map_or_else(
                || text.parse().ok(),
                |hex| u64::from_str_radix(hex, 16).ok(),
            )
        })
    })
}

pub fn id(value: &Value) -> Option<u32> {
    number(value).and_then(|value| value.try_into().ok())
}

pub fn props(object: &Value) -> Option<&Map<String, Value>> {
    object
        .get("info")
        .and_then(|info| info.get("props"))
        .and_then(Value::as_object)
        .or_else(|| object.get("props").and_then(Value::as_object))
}

pub fn property<'a>(object: &'a Value, key: &str) -> Option<&'a Value> {
    props(object)?.get(key)
}

pub fn text_property<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    property(object, key)?.as_str()
}

pub fn serial(object: &Value) -> Option<u64> {
    property(object, "object.serial").and_then(number)
}

pub fn is_type(object: &Value, kind: &str) -> bool {
    object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|name| name.ends_with(kind))
}

pub fn truth(value: &Value) -> bool {
    value
        .as_bool()
        .unwrap_or_else(|| value.as_str() == Some("true"))
}

pub fn merge_json(current: &mut Value, patch: &Value) {
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

pub fn merge_metadata(current: &mut Value, patch: &Value) -> bool {
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
pub struct Registry {
    pub objects: BTreeMap<u32, Value>,
    pub removed_serials: BTreeMap<u32, u64>,
}

impl Registry {
    pub fn cookie(&self) -> Option<u64> {
        self.objects
            .values()
            .find(|object| is_type(object, ":Core"))?
            .get("info")?
            .get("cookie")
            .and_then(number)
    }

    pub fn replace(&mut self, objects: Vec<Value>) -> AppResult<()> {
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

    pub fn merge(&mut self, objects: Vec<Value>) -> AppResult<bool> {
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
                // A serial-less removal can predate a fresh snapshot. Only an
                // authoritative resync may decide which current instance died.
                let Some(incoming_serial) = incoming_serial else {
                    resync = true;
                    continue;
                };
                if current_serial.is_some_and(|current| incoming_serial < current) {
                    continue;
                }
                next.removed_serials
                    .entry(object_id)
                    .and_modify(|old| *old = (*old).max(incoming_serial))
                    .or_insert(incoming_serial);
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

    pub fn invalidate_dependencies(&mut self, removed_id: u32) {
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

    pub fn defaults(&self) -> (Option<String>, Option<String>, Option<String>) {
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

    pub fn node(&self, name: &str) -> AppResult<AudioNode> {
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

    pub fn nodes(&self, output: bool) -> Vec<AudioNode> {
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

    pub fn audio_node(&self, node_id: u32, object: &Value) -> AudioNode {
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
            device_serial: inherited("device.serial").and_then(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| value.as_u64().map(|serial| serial.to_string()))
            }),
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

    pub fn ports(&self, node_id: u32, direction: &str) -> Vec<(u32, &Value)> {
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
                        .and_then(|direction| match direction {
                            "input" | "in" => Some("in"),
                            "output" | "out" => Some("out"),
                            _ => None,
                        })
                        == Some(direction)
                    && !property(object, "port.monitor").is_some_and(truth)
            })
            .map(|(id, object)| (*id, object))
            .collect()
    }

    pub fn port_channels(&self, node_id: u32, direction: &str) -> Vec<String> {
        self.ports(node_id, direction)
            .into_iter()
            .filter_map(|(_, port)| text_property(port, "audio.channel").map(str::to_owned))
            .collect()
    }

    pub fn process_id(&self, node_id: u32) -> Option<u32> {
        let node = self.objects.get(&node_id)?;
        property(node, "application.process.id")
            .and_then(id)
            .or_else(|| {
                let client_id = property(node, "client.id").and_then(id)?;
                property(self.objects.get(&client_id)?, "application.process.id").and_then(id)
            })
    }
}
