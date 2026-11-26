use super::registry::{id, is_type, property, text_property, Registry};
use crate::models::{AppError, AppResult, AudioNode, ErrorCode};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const CHANNELS_71: [&str; 8] = ["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"];

pub fn channel_names(value: &Value) -> Vec<String> {
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

pub fn same_channels(actual: &[String], expected: &[&str]) -> bool {
    actual.len() == expected.len()
        && actual.iter().map(String::as_str).collect::<BTreeSet<_>>()
            == expected.iter().copied().collect()
}

pub fn stereo_target(node: &AudioNode) -> AppResult<()> {
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
            "EQ e surround richiedono un'uscita stereo FL/FR. Mono/HFP resta disponibile senza effetti; nessun profilo Bluetooth viene cambiato."));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn verify_graph(
    registry: &Registry,
    record: &crate::surround::RestoreRecord,
) -> AppResult<bool> {
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
    let inputs: &[&str] = if record.stereo_eq {
        &["FL", "FR"]
    } else {
        &CHANNELS_71
    };
    if input.media_class != "Audio/Sink"
        || playback.media_class != "Stream/Output/Audio"
        || !same_channels(&input.channels, inputs)
        || !same_channels(&playback.channels, &["FL", "FR"])
    {
        return Err(AppError::new(
            ErrorCode::NotConfirmed,
            "I canali di ingresso e uscita del filtro non corrispondono alla modalità richiesta.",
        ));
    }
    if !same_channels(&registry.port_channels(input.id, "in"), inputs)
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
