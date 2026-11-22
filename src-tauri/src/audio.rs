#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
#![allow(unused_imports)]

pub mod controller;
pub mod graph;
pub mod monitor;
pub mod parser;
pub mod registry;

pub use controller::*;
pub use graph::*;
pub use monitor::*;
pub use parser::*;
pub use registry::*;

pub use crate::models::{
    AppError, AppResult, AudioNode, AudioProfile, AudioSnapshot, ErrorCode, SurroundState,
};
pub(crate) use crate::profiles::{
    validate_node_name, validate_profile, AppPaths, APP_NODE_PREFIX, SYSTEM_HRTF,
};
#[cfg(any(target_os = "linux", test))]
pub(crate) use crate::surround::{self, HrtfSelection};
#[cfg(target_os = "linux")]
pub(crate) use crate::surround::{
    runtime::{ActiveFilter, OwnedChild},
    RestoreAction, RestoreIntent, RestoreRecord,
};
pub use serde_json::{json, Map, Value};
#[cfg(target_os = "linux")]
pub use std::time::Duration;

#[cfg(all(test, target_os = "linux"))]
#[path = "audio_lifecycle_tests.rs"]
mod lifecycle_tests;

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
    fn real_pipewire_direction_names_and_property_fallback_share_the_same_ports() {
        let mut data = fixture();
        for object in &mut data {
            if is_type(object, ":Port") {
                let raw: String = object
                    .get("info")
                    .and_then(|info| info.get("direction"))
                    .and_then(Value::as_str)
                    .or_else(|| text_property(object, "port.direction"))
                    .map(str::to_owned)
                    .expect("fixture port direction");
                let direction = match raw.as_str() {
                    "in" => "input",
                    "out" => "output",
                    other => panic!("Unexpected fixture direction: {other}"),
                };
                object["info"]["direction"] = json!(direction);
                if let Some(props) = object
                    .pointer_mut("/info/props")
                    .and_then(Value::as_object_mut)
                {
                    props.insert("port.direction".into(), json!(raw));
                }
            }
        }
        let mut registry = Registry::default();
        registry.replace(data.clone()).unwrap();
        assert_eq!(registry.port_channels(40, "in"), vec!["FL", "FR"]);
        assert_eq!(registry.port_channels(41, "out"), vec!["MONO"]);
        for object in &mut data {
            if is_type(object, ":Port") {
                object["info"].as_object_mut().unwrap().remove("direction");
            }
        }
        registry.replace(data).unwrap();
        assert_eq!(registry.port_channels(40, "in"), vec!["FL", "FR"]);
        assert_eq!(registry.port_channels(41, "out"), vec!["MONO"]);
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
        assert!(registry.node("replacement").is_ok());
        let authoritative = registry
            .objects
            .values()
            .filter(|object| object["id"] != 40)
            .cloned()
            .collect();
        registry.replace(authoritative).unwrap();
        assert!(registry.node("replacement").is_err());
        registry.merge(vec![json!({"id":40,"type":"PipeWire:Interface:Node","info":{"props":{"object.serial":200,"node.name":"replacement"}}})]).unwrap();
        assert!(!registry.objects.contains_key(&40));
    }

    #[test]
    fn queued_serialless_removal_cannot_tombstone_a_fresh_instance() {
        let mut registry = Registry::default();
        registry.replace(fixture()).unwrap();
        let mut fresh = fixture();
        fresh.iter_mut().find(|object| object["id"] == 40).unwrap()["info"]["props"]
            ["object.serial"] = json!(200);
        registry.replace(fresh.clone()).unwrap();
        assert!(registry.merge(vec![json!({"id":40,"info":null})]).unwrap());
        assert_eq!(
            registry.node("alsa_output.headset").unwrap().object_serial,
            Some(200)
        );
        registry.replace(fresh).unwrap();
        assert_eq!(
            registry.node("alsa_output.headset").unwrap().object_serial,
            Some(200)
        );
        assert!(!registry
            .removed_serials
            .get(&40)
            .is_some_and(|serial| *serial >= 200));
        assert!(registry.objects.contains_key(&50));
    }

    #[test]
    fn numeric_hardware_serial_is_not_discarded() {
        let mut data = fixture();
        for object in &mut data {
            if let Some(props) = object
                .pointer_mut("/info/props")
                .and_then(Value::as_object_mut)
            {
                if props.contains_key("device.serial") {
                    props.insert("device.serial".into(), json!(123456789));
                }
            }
        }
        let mut registry = Registry::default();
        registry.replace(data).unwrap();
        assert_eq!(
            registry
                .node("alsa_output.headset")
                .unwrap()
                .device_serial
                .as_deref(),
            Some("123456789")
        );
    }

    #[tokio::test]
    async fn bounded_monitor_backpressures_instead_of_dropping_patches() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(MONITOR_CAPACITY);
        for _ in 0..MONITOR_CAPACITY {
            sender.try_send(AudioEvent::Changed(Vec::new())).unwrap();
        }
        assert!(matches!(
            sender.try_send(AudioEvent::Disconnected),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));
        let pending = tokio::spawn(async move { sender.send(AudioEvent::Disconnected).await });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        assert!(matches!(
            receiver.recv().await,
            Some(AudioEvent::Changed(_))
        ));
        pending.await.unwrap().unwrap();
        for _ in 1..MONITOR_CAPACITY {
            assert!(matches!(
                receiver.recv().await,
                Some(AudioEvent::Changed(_))
            ));
        }
        assert!(matches!(
            receiver.recv().await,
            Some(AudioEvent::Disconnected)
        ));
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
