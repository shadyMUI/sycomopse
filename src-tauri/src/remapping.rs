#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
#![allow(unused_imports)]

pub mod agent;
pub mod bridge;
pub mod controller;
pub mod store;
pub mod types;

pub use agent::*;
pub use bridge::*;
pub use controller::*;
pub use store::*;
pub use types::*;

pub(crate) use crate::models::*;
pub(crate) use crate::profiles::{self, AppPaths};
pub(crate) use std::collections::{HashMap, HashSet, VecDeque};
pub(crate) use std::fs::{self, File, OpenOptions};
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::Mutex;
pub(crate) use std::time::{Duration, Instant};
pub(crate) use uuid::Uuid;

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::Path;
    use uuid::Uuid;

    pub(super) fn paths(root: &Path) -> AppPaths {
        AppPaths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            state_dir: root.join("state"),
            runtime_dir: root.join("runtime"),
        }
    }

    pub(super) fn mouse() -> InputDevice {
        InputDevice {
            id: "mouse-serial".into(),
            fingerprint: "mouse-serial".into(),
            identity_strength: RemappingIdentityStrength::Serial,
            name: "Mouse".into(),
            serial: Some("serial".into()),
            physical_path: Some("/sys/devices/usb1/1-1".into()),
            vendor_id: 0x1234,
            product_id: 0x4321,
            transport: "usb".into(),
            nodes: vec![InputNode {
                id: "mouse".into(),
                origin_hash: "hash".into(),
                name: "Mouse".into(),
                path: "/dev/input/event1".into(),
                phys: "usb1/input0".into(),
                key_codes: vec![272, 273, 275],
                relative_codes: vec![0, 1],
            }],
            buttons: vec![
                InputButton {
                    id: "left".into(),
                    label: "Sinistro".into(),
                    code: 272,
                    node_id: "mouse".into(),
                    origin_hash: "hash".into(),
                },
                InputButton {
                    id: "side".into(),
                    label: "Laterale".into(),
                    code: 275,
                    node_id: "mouse".into(),
                    origin_hash: "hash".into(),
                },
            ],
        }
    }

    #[test]
    fn rejects_unknown_actions_duplicate_buttons_and_loss_of_primary_click() {
        assert!(
            serde_json::from_str::<MouseAction>(r#"{"kind":"shell","command":"anything"}"#)
                .is_err()
        );
        assert!(validate_action(
            &MouseAction::Chord {
                modifiers: vec![29, 29],
                key: 46
            },
            None
        )
        .is_err());
        assert!(validate_action(&MouseAction::Key { code: 0 }, None).is_err());
        let disable = MouseBinding {
            button_id: "left".into(),
            action: MouseAction::Disabled,
        };
        assert!(validate_bindings(&[disable], &mouse(), &[29, 46, 272]).is_err());
        let copy = MouseBinding {
            button_id: "side".into(),
            action: MouseAction::Chord {
                modifiers: vec![29],
                key: 46,
            },
        };
        assert!(validate_bindings(std::slice::from_ref(&copy), &mouse(), &[29, 46, 272]).is_ok());
        let autoclick = MouseBinding {
            button_id: "side".into(),
            action: MouseAction::Autoclicker {
                cps: 15,
                button: 272,
            },
        };
        assert!(
            validate_bindings(std::slice::from_ref(&autoclick), &mouse(), &[29, 46, 272]).is_ok()
        );
        let bad_autoclick = MouseBinding {
            button_id: "side".into(),
            action: MouseAction::Autoclicker {
                cps: 0,
                button: 272,
            },
        };
        assert!(validate_bindings(
            std::slice::from_ref(&bad_autoclick),
            &mouse(),
            &[29, 46, 272]
        )
        .is_err());
        assert!(validate_bindings(&[copy.clone(), copy], &mouse(), &[29, 46, 272]).is_err());
    }

    #[test]
    fn immutable_presets_match_byte_hash_and_reject_external_edits() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::at(paths(temp.path()));
        let profile = RemappingProfile {
            id: Uuid::new_v4().to_string(),
            fingerprint: "mouse-serial".into(),
            identity_strength: RemappingIdentityStrength::Serial,
            identity: mouse().identity(),
            bindings: vec![MouseBinding {
                button_id: "side".into(),
                action: MouseAction::Key { code: 46 },
            }],
            enabled: true,
            auto_apply: true,
            allow_weak_identity: false,
            paused_for_safety: false,
            compiled_preset: None,
        };
        let name = store.preset(&profile, &mouse()).unwrap();
        assert!(valid_preset_name(&name, current_uid()));
        let path = store
            .directory()
            .join("presets")
            .join(format!("{name}.json"));
        let bytes = fs::read(&path).unwrap();
        assert!(name.ends_with(&format!("{:x}", Sha256::digest(&bytes))));
        assert_eq!(store.preset(&profile, &mouse()).unwrap(), name);
        fs::write(path, b"{}").unwrap();
        assert!(store.preset(&profile, &mouse()).is_err());
    }

    #[test]
    fn future_or_corrupt_schema_is_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::at(paths(temp.path()));
        let _lock = store.lock().unwrap();
        profiles::atomic_write(
            &store.path(),
            br#"{"schemaVersion":2,"revision":1,"profiles":[]}"#,
        )
        .unwrap();
        assert!(store.load().is_err());
        assert_eq!(
            fs::read(store.path()).unwrap(),
            br#"{"schemaVersion":2,"revision":1,"profiles":[]}"#
        );
    }

    #[test]
    fn recovery_disarm_is_independent_of_corrupt_profile_document() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::at(paths(temp.path()));
        let _lock = store.lock().unwrap();
        profiles::atomic_write(&store.path(), b"corrupt document").unwrap();
        store.disarm(&["mouse-serial".into()], true).unwrap();
        assert_eq!(store.recovery().unwrap().disarmed, vec!["mouse-serial"]);
        assert!(store.load().is_err());
        assert_eq!(fs::read(store.path()).unwrap(), b"corrupt document");
        store.disarm(&["mouse-serial".into()], false).unwrap();
        assert!(store.recovery().unwrap().disarmed.is_empty());
        assert_eq!(store.recovery().unwrap().identities, vec!["mouse-serial"]);
    }

    #[test]
    fn topology_generation_change_is_not_a_manual_stop() {
        let previous = (":1.10".to_owned(), Some("preset".to_owned()), 7);
        assert!(unchanged_observation(
            &previous,
            ":1.10",
            &Some("preset".to_owned()),
            7
        ));
        assert!(!unchanged_observation(
            &previous,
            ":1.10",
            &Some("preset".to_owned()),
            8
        ));
        assert!(!unchanged_observation(
            &previous,
            ":1.11",
            &Some("preset".to_owned()),
            7
        ));
    }

    #[test]
    fn inventory_rejects_virtual_sources_and_duplicate_identity() {
        let mut inventory = Inventory {
            version: 1,
            generation: 1,
            keyboard_output_codes: vec![29, 46],
            devices: vec![mouse()],
        };
        assert!(inventory.validate().is_ok());
        inventory.devices[0].nodes[0].phys = "input-remapper/usb1".into();
        assert!(inventory.validate().is_err());
        inventory.devices = vec![mouse(), mouse()];
        assert!(inventory.validate().is_err());
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/remapping/recovery.rs"]
mod recovery_tests;
