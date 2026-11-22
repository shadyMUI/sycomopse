use super::*;
use serde_json::json;
use std::process::Stdio;

#[derive(Debug, PartialEq)]
enum Action {
    Level(String, Option<f64>, Option<bool>),
    Default(Option<String>),
    Spawn,
    Stop,
}

#[derive(Default)]
pub(crate) struct Backend {
    registry: Registry,
    selection: Option<HrtfSelection>,
    actions: Vec<Action>,
    fail_spawn: bool,
    journal_before_failure: bool,
    post_spawn_store_fault: bool,
    pause_stop: bool,
    hide_graph: bool,
    fail_stop: bool,
    fail_read: bool,
    external_on_stop: Option<String>,
    external_mute_before_level: Option<bool>,
    external_volume_before_level: Option<f64>,
    generation: u64,
}

impl Backend {
    pub(super) fn read_registry(&self) -> AppResult<Registry> {
        if self.fail_read {
            return Err(AppError::new(
                ErrorCode::AudioUnavailable,
                "Fixture: server disconnesso.",
            ));
        }
        let mut registry = self.registry.clone();
        if self.hide_graph {
            let ids: Vec<_> = registry
                .objects
                .iter()
                .filter_map(|(id, object)| {
                    text_property(object, "node.name")
                        .is_some_and(|name| name.starts_with(APP_NODE_PREFIX))
                        .then_some(*id)
                })
                .collect();
            for id in ids {
                registry.invalidate_dependencies(id);
                registry.objects.remove(&id);
            }
        }
        Ok(registry)
    }

    pub(super) fn resolve_hrtf(&self, reference: &str) -> AppResult<HrtfSelection> {
        self.selection
            .as_ref()
            .filter(|selection| selection.status.reference == reference)
            .cloned()
            .ok_or_else(|| AppError::new(ErrorCode::HrtfUnavailable, "Fixture HRTF assente."))
    }

    pub(super) fn write_level(
        &mut self,
        node: &AudioNode,
        volume: Option<f64>,
        muted: Option<bool>,
    ) -> AppResult<()> {
        let current = self.registry.node(&node.node_name)?;
        if current.object_serial != node.object_serial || current.id != node.id {
            return Err(AppError::new(
                ErrorCode::TargetUnavailable,
                "Fixture: istanza sostituita.",
            ));
        }
        self.actions
            .push(Action::Level(node.node_name.clone(), volume, muted));
        let actual_volume = volume
            .or(self.external_volume_before_level.take())
            .or(current.volume)
            .unwrap_or(1.0);
        let actual_mute = muted
            .or(self.external_mute_before_level.take())
            .or(current.muted)
            .unwrap_or(false);
        let channels = vec![actual_volume.powi(3); current.channels.len().max(1)];
        self.registry.objects.get_mut(&current.id).unwrap()["info"]["params"]["Props"] =
            json!([{ "channelVolumes": channels, "mute": actual_mute }]);
        Ok(())
    }

    fn defaults(&mut self, configured: Option<&str>, effective: Option<&str>) {
        let entries = self.registry.objects.get_mut(&21).unwrap()["metadata"]
            .as_array_mut()
            .unwrap();
        entries.retain(|entry| {
            !matches!(
                entry["key"].as_str(),
                Some("default.audio.sink" | "default.configured.audio.sink")
            )
        });
        for (key, name) in [
            ("default.configured.audio.sink", configured),
            ("default.audio.sink", effective),
        ] {
            if let Some(name) = name {
                entries.push(json!({"subject":0, "key":key, "type":"Spa:String:JSON", "value":{"name":name}}));
            }
        }
    }

    pub(super) fn write_default(
        &mut self,
        node: Option<&AudioNode>,
        expected: Option<&str>,
    ) -> AppResult<()> {
        if self.registry.defaults().1.as_deref() != expected {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Fixture: scelta esterna conservata.",
            ));
        }
        if let Some(node) = node {
            let current = self.registry.node(&node.node_name)?;
            if current.object_serial != node.object_serial || current.id != node.id {
                return Err(AppError::new(
                    ErrorCode::TargetUnavailable,
                    "Fixture: default su istanza diversa.",
                ));
            }
        }
        let name = node.map(|node| node.node_name.clone());
        self.actions.push(Action::Default(name.clone()));
        self.defaults(
            name.as_deref(),
            name.as_deref().or(Some("alsa_output.headset")),
        );
        Ok(())
    }

    fn record(
        &self,
        target: &AudioNode,
        configured: Option<String>,
        effective: Option<String>,
    ) -> RestoreRecord {
        let session = "4ccf058b-1487-4cda-9b5a-11a6033a5ad0";
        RestoreRecord {
            schema_version: 1,
            session_id: session.into(),
            previous_configured_default: configured,
            previous_effective_default: effective,
            selected_physical_target: target.node_name.clone(),
            target_serial: target.object_serial.unwrap(),
            virtual_node_name: format!("{APP_NODE_PREFIX}input.{session}"),
            playback_node_name: format!("{APP_NODE_PREFIX}playback.{session}"),
            app_process_id: std::process::id(),
            filter_process_id: None,
            virtual_object_serial: None,
            playback_object_serial: None,
            core_cookie: self.registry.cookie(),
        }
    }

    pub(super) fn spawn(
        &mut self,
        paths: &AppPaths,
        target: &AudioNode,
        hrtf: &HrtfSelection,
        configured: Option<String>,
        effective: Option<String>,
    ) -> AppResult<ActiveFilter> {
        self.actions.push(Action::Spawn);
        let mut record = self.record(target, configured, effective);
        if self.fail_spawn {
            if self.journal_before_failure {
                surround::write_restore(paths, &record)?;
            }
            return Err(AppError::new(
                ErrorCode::Io,
                "Fixture: spawn/configurazione fallita.",
            ));
        }
        let directory = paths.runtime_dir.join(&record.session_id);
        std::fs::create_dir_all(&directory)?;
        surround::write_restore(paths, &record)?;
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command
            .args([
                "--ignored",
                "--exact",
                "audio::lifecycle_tests::filter_child",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let process = OwnedChild::spawn(command)?;
        record.filter_process_id = process.id();
        let mut graph: Vec<Value> =
            serde_json::from_str(include_str!("../tests/audio_surround_graph.json")).unwrap();
        self.generation += 1000;
        let left_port = self
            .registry
            .ports(target.id, "in")
            .iter()
            .find(|(_, port)| text_property(port, "audio.channel") == Some("FL"))
            .unwrap()
            .0;
        let right_port = self
            .registry
            .ports(target.id, "in")
            .iter()
            .find(|(_, port)| text_property(port, "audio.channel") == Some("FR"))
            .unwrap()
            .0;
        for object in &mut graph {
            if let Some(props) = object
                .pointer_mut("/info/props")
                .and_then(Value::as_object_mut)
            {
                let old = props.get("object.serial").and_then(number).unwrap();
                props.insert("object.serial".into(), json!(old + self.generation));
                if props.contains_key("application.process.id") {
                    props.insert(
                        "application.process.id".into(),
                        json!(process.id().unwrap().to_string()),
                    );
                }
            }
            if is_type(object, ":Link") {
                let port = if object["info"]["output-port-id"] == 88 {
                    left_port
                } else {
                    right_port
                };
                object["info"]["input-node-id"] = json!(target.id);
                object["info"]["input-port-id"] = json!(port);
            }
        }
        self.registry.merge(graph)?;
        Ok(ActiveFilter {
            process,
            record,
            directory,
            hrtf_reference: hrtf.status.reference.clone(),
        })
    }

    pub(super) async fn before_stop(&mut self) -> AppResult<()> {
        self.actions.push(Action::Stop);
        if self.pause_stop {
            std::future::pending::<()>().await;
        }
        if self.fail_stop {
            return Err(AppError::new(
                ErrorCode::Timeout,
                "Fixture: terminazione non confermata.",
            ));
        }
        Ok(())
    }

    pub(super) fn prepare_post_spawn_store(&mut self, paths: &AppPaths) -> AppResult<()> {
        if self.post_spawn_store_fault {
            let file = paths.state_dir.join("audio-restore.json");
            std::fs::rename(&file, paths.state_dir.join("saved-pre-spawn.json"))?;
            std::fs::create_dir(file)?;
            self.post_spawn_store_fault = false;
        }
        Ok(())
    }

    pub(super) fn stopped(&mut self, record: &RestoreRecord) {
        let ids: Vec<_> = self
            .registry
            .objects
            .iter()
            .filter_map(|(id, object)| {
                let name = text_property(object, "node.name");
                (name == Some(record.virtual_node_name.as_str())
                    || name == Some(record.playback_node_name.as_str()))
                .then_some(*id)
            })
            .collect();
        for id in ids {
            self.registry.invalidate_dependencies(id);
            self.registry.objects.remove(&id);
        }
        self.registry.objects.remove(&60);
        if let Some(name) = self.external_on_stop.take() {
            self.defaults(Some(&name), Some(&name));
        }
    }
}

fn controller() -> (tempfile::TempDir, AudioController) {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths {
        config_dir: directory.path().join("config"),
        data_dir: directory.path().join("data"),
        state_dir: directory.path().join("state"),
        runtime_dir: directory.path().join("runtime"),
    };
    let hrtf_path = directory.path().join("fixture.sofa");
    std::fs::write(&hrtf_path, vec![0; 128]).unwrap();
    let selection = HrtfSelection {
        path: hrtf_path,
        status: crate::models::HrtfStatus {
            reference: "d8e30606-9eac-4896-9b85-cab1566e12a3.sofa".into(),
            display_name: "Fixture: semantic validation belongs to worker tests".into(),
            readable: true,
        },
    };
    let mut registry = Registry::default();
    registry
        .replace(serde_json::from_str(include_str!("../tests/audio_registry.json")).unwrap())
        .unwrap();
    let mut other = registry.objects[&40].clone();
    other["id"] = json!(44);
    other["info"]["props"]["object.serial"] = json!(104);
    other["info"]["props"]["node.name"] = json!("alsa_output.other");
    other["info"]["params"]["Props"] = json!([{ "channelVolumes":[1.0,1.0], "mute":false }]);
    let mut ports = Vec::new();
    for old_id in [50, 51] {
        let mut port = registry.objects[&old_id].clone();
        port["id"] = json!(old_id + 4);
        port["info"]["props"]["object.serial"] = json!(old_id + 1000);
        port["info"]["props"]["node.id"] = json!(44);
        ports.push(port);
    }
    ports.push(other);
    registry.merge(ports).unwrap();
    let mut controller = AudioController::new(paths);
    controller.registry = registry.clone();
    controller.available = true;
    controller.recovery_pending = false;
    controller.selected_output = Some("alsa_output.headset".into());
    controller.hrtf = Some(selection.clone());
    controller.test_backend = Some(Backend {
        registry,
        selection: Some(selection),
        ..Backend::default()
    });
    (directory, controller)
}

fn profile(output: &str, surround: bool) -> AudioProfile {
    AudioProfile {
        schema_version: 1,
        id: uuid::Uuid::new_v4().to_string(),
        name: "Regression".into(),
        output_node_name: output.into(),
        hrtf: surround.then(|| "d8e30606-9eac-4896-9b85-cab1566e12a3.sofa".into()),
        volume: 0.1,
        mute: true,
        surround_requested: surround,
    }
}

fn active(controller: &mut AudioController) {
    let backend = controller.test_backend.as_mut().unwrap();
    let target = backend.registry.node("alsa_output.headset").unwrap();
    let selection = backend.selection.clone().unwrap();
    let mut filter = backend
        .spawn(
            &controller.paths,
            &target,
            &selection,
            None,
            Some(target.node_name.clone()),
        )
        .unwrap();
    filter.record.virtual_object_serial = backend
        .registry
        .node(&filter.record.virtual_node_name)
        .unwrap()
        .object_serial;
    filter.record.playback_object_serial = backend
        .registry
        .node(&filter.record.playback_node_name)
        .unwrap()
        .object_serial;
    surround::write_restore(&controller.paths, &filter.record).unwrap();
    backend.defaults(
        Some(&filter.record.virtual_node_name),
        Some(&filter.record.virtual_node_name),
    );
    backend.actions.clear();
    controller.registry = backend.registry.clone();
    controller.filter = Some(filter);
    controller.state = SurroundState::On;
}

#[test]
#[ignore = "owned no-audio child fixture invoked by lifecycle tests only"]
fn filter_child() {
    std::thread::sleep(Duration::from_secs(60));
}

#[tokio::test]
async fn physical_profile_prepares_mute_and_volume_before_publishing_default() {
    let (_directory, mut controller) = controller();
    controller
        .apply_profile(&profile("alsa_output.other", false))
        .await
        .unwrap();
    assert_eq!(
        controller.test_backend.as_ref().unwrap().actions,
        vec![
            Action::Level("alsa_output.other".into(), Some(0.1), Some(true)),
            Action::Default(Some("alsa_output.other".into())),
        ]
    );
    assert_eq!(
        controller.registry.node("alsa_output.other").unwrap().muted,
        Some(true)
    );
}

#[tokio::test]
async fn queued_snapshot_cannot_remove_an_instance_already_seen_by_a_newer_read() {
    let (_directory, mut controller) = controller();
    controller
        .test_backend
        .as_mut()
        .unwrap()
        .registry
        .objects
        .get_mut(&40)
        .unwrap()["info"]["props"]["object.serial"] = json!(200);
    controller.read_registry().await.unwrap();
    assert_eq!(
        controller
            .registry
            .node("alsa_output.headset")
            .unwrap()
            .object_serial,
        Some(200)
    );
    let old = serde_json::from_str(include_str!("../tests/audio_registry.json")).unwrap();
    controller
        .handle_event(AudioEvent::Snapshot(old))
        .await
        .unwrap();
    assert_eq!(
        controller
            .registry
            .node("alsa_output.headset")
            .unwrap()
            .object_serial,
        Some(200)
    );
    assert!(!controller
        .registry
        .removed_serials
        .get(&40)
        .is_some_and(|serial| *serial >= 200));
    controller.read_registry().await.unwrap();
    assert_eq!(
        controller
            .registry
            .node("alsa_output.headset")
            .unwrap()
            .object_serial,
        Some(200)
    );
}

#[tokio::test]
async fn physical_profile_never_routes_through_the_old_unprepared_fallback() {
    let (_directory, mut controller) = controller();
    active(&mut controller);
    controller
        .apply_profile(&profile("alsa_output.other", false))
        .await
        .unwrap();
    let actions = &controller.test_backend.as_ref().unwrap().actions;
    assert_eq!(
        actions[0],
        Action::Level("alsa_output.other".into(), Some(0.1), Some(true))
    );
    assert_eq!(actions[1], Action::Stop);
    assert!(actions[2..]
        .iter()
        .all(|action| *action == Action::Default(Some("alsa_output.other".into()))));
    assert!(controller.filter.is_none());
    assert!(surround::read_restore(&controller.paths).unwrap().is_none());
}

#[tokio::test]
async fn surrounding_profile_does_not_reuse_a_filter_after_an_external_default_choice() {
    let (_directory, mut controller) = controller();
    active(&mut controller);
    let backend = controller.test_backend.as_mut().unwrap();
    backend.defaults(Some("virtual.other"), Some("virtual.other"));
    backend.fail_spawn = true;
    assert!(controller
        .apply_profile(&profile("alsa_output.headset", true))
        .await
        .is_err());
    let actions = &controller.test_backend.as_ref().unwrap().actions;
    assert!(actions.contains(&Action::Stop));
    assert!(actions.contains(&Action::Spawn));
    assert_eq!(
        controller.registry.defaults().1.as_deref(),
        Some("virtual.other")
    );
    assert_eq!(controller.state, SurroundState::Error);
}

#[tokio::test]
async fn spawn_failure_finishes_starting_and_keeps_the_retry_available() {
    let (_directory, mut controller) = controller();
    controller.test_backend.as_mut().unwrap().fail_spawn = true;
    assert!(controller
        .apply_profile(&profile("alsa_output.headset", true))
        .await
        .is_err());
    assert_eq!(controller.state, SurroundState::Error);
    assert!(!controller.recovery_pending);
    assert!(controller.snapshot().can_enable_surround);
}

#[tokio::test]
async fn failure_after_writing_the_journal_is_recovered_without_a_restart() {
    let (_directory, mut controller) = controller();
    let backend = controller.test_backend.as_mut().unwrap();
    backend.fail_spawn = true;
    backend.journal_before_failure = true;
    assert!(controller
        .apply_profile(&profile("alsa_output.headset", true))
        .await
        .is_err());
    assert!(controller.recovery_pending);
    assert!(surround::read_restore(&controller.paths).unwrap().is_some());
    controller.refresh().await.unwrap();
    assert!(!controller.recovery_pending);
    assert_eq!(controller.state, SurroundState::Off);
    assert!(surround::read_restore(&controller.paths).unwrap().is_none());
}

#[tokio::test]
async fn failed_restore_cleanup_is_retried_after_filesystem_repair() {
    let (_directory, mut controller) = controller();
    active(&mut controller);
    let file = controller.paths.state_dir.join("audio-restore.json");
    let saved = controller.paths.state_dir.join("saved-restore.json");
    std::fs::rename(&file, &saved).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(controller.stop_filter(RestoreIntent::Bypass).await.is_err());
    assert!(controller.recovery_pending);
    assert_eq!(controller.state, SurroundState::Error);
    assert!(controller.filter.is_none());
    std::fs::remove_dir(&file).unwrap();
    std::fs::rename(saved, file).unwrap();
    controller.refresh().await.unwrap();
    assert!(!controller.recovery_pending);
    assert_eq!(controller.state, SurroundState::Off);
    assert!(surround::read_restore(&controller.paths).unwrap().is_none());
}

#[tokio::test]
async fn unconfirmed_stop_preserves_the_owned_filter_and_retries_it() {
    let (_directory, mut controller) = controller();
    active(&mut controller);
    controller.test_backend.as_mut().unwrap().fail_stop = true;
    assert!(controller.stop_filter(RestoreIntent::Bypass).await.is_err());
    assert!(controller.filter.is_some());
    assert!(controller.recovery_pending);
    assert!(surround::read_restore(&controller.paths).unwrap().is_some());
    controller.test_backend.as_mut().unwrap().fail_stop = false;
    controller.refresh().await.unwrap();
    assert!(controller.filter.is_none());
    assert!(!controller.recovery_pending);
    assert_eq!(controller.state, SurroundState::Off);
}

#[tokio::test]
async fn volume_and_mute_controls_do_not_supply_the_other_field_from_a_stale_snapshot() {
    let (_directory, mut controller) = controller();
    controller
        .test_backend
        .as_mut()
        .unwrap()
        .external_mute_before_level = Some(true);
    controller
        .write_volume("alsa_output.headset", 0.2)
        .await
        .unwrap();
    assert_eq!(
        controller
            .registry
            .node("alsa_output.headset")
            .unwrap()
            .muted,
        Some(true)
    );
    controller
        .test_backend
        .as_mut()
        .unwrap()
        .external_volume_before_level = Some(0.3);
    controller
        .write_mute("alsa_output.headset", false)
        .await
        .unwrap();
    assert!(
        (controller
            .registry
            .node("alsa_output.headset")
            .unwrap()
            .volume
            .unwrap()
            - 0.3)
            .abs()
            < 0.002
    );
    assert_eq!(
        controller.test_backend.as_ref().unwrap().actions,
        vec![
            Action::Level("alsa_output.headset".into(), Some(0.2), None),
            Action::Level("alsa_output.headset".into(), None, Some(false)),
        ]
    );
}

#[tokio::test]
async fn new_default_chosen_during_stop_is_not_overwritten_by_the_profile() {
    let (_directory, mut controller) = controller();
    active(&mut controller);
    controller.test_backend.as_mut().unwrap().external_on_stop = Some("virtual.other".into());
    assert!(controller
        .apply_profile(&profile("alsa_output.other", false))
        .await
        .is_err());
    assert_eq!(
        controller.registry.defaults().1.as_deref(),
        Some("virtual.other")
    );
    assert!(!controller
        .test_backend
        .as_ref()
        .unwrap()
        .actions
        .iter()
        .any(|action| *action == Action::Default(Some("alsa_output.other".into()))));
    controller.refresh().await.unwrap();
    assert!(!controller.recovery_pending);
    assert_eq!(
        controller.registry.defaults().1.as_deref(),
        Some("virtual.other")
    );
}

#[tokio::test]
async fn surrounding_profile_prepares_the_virtual_level_before_the_default() {
    let (_directory, mut controller) = controller();
    controller
        .apply_profile(&profile("alsa_output.headset", true))
        .await
        .unwrap();
    let actions = &controller.test_backend.as_ref().unwrap().actions;
    let virtual_name = controller
        .filter
        .as_ref()
        .unwrap()
        .record
        .virtual_node_name
        .clone();
    assert_eq!(
        actions,
        &vec![
            Action::Spawn,
            Action::Level(virtual_name.clone(), Some(0.1), Some(true)),
            Action::Default(Some(virtual_name))
        ]
    );
    assert_eq!(controller.state, SurroundState::On);
    controller.shutdown().await.unwrap();
}

#[tokio::test]
async fn explicit_profile_retries_stop_the_owned_unknown_filter_before_any_default_write() {
    for surround in [false, true] {
        let (_directory, mut controller) = controller();
        active(&mut controller);
        let original = controller.registry.defaults().1;
        controller.test_backend.as_mut().unwrap().fail_stop = true;
        assert!(controller.stop_filter(RestoreIntent::Bypass).await.is_err());
        assert!(controller
            .apply_profile(&profile("alsa_output.headset", surround))
            .await
            .is_err());
        assert_eq!(controller.registry.defaults().1, original);
        assert!(controller.filter.is_some());
        assert!(controller
            .test_backend
            .as_ref()
            .unwrap()
            .actions
            .iter()
            .all(|action| matches!(action, Action::Stop)));
        controller.test_backend.as_mut().unwrap().fail_stop = false;
        controller.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn explicit_surround_retry_never_treats_an_owned_filter_as_stale() {
    let (_directory, mut controller) = controller();
    active(&mut controller);
    let original = controller.registry.defaults().1;
    controller.test_backend.as_mut().unwrap().fail_stop = true;
    assert!(controller.stop_filter(RestoreIntent::Bypass).await.is_err());
    assert!(controller.set_surround(true).await.is_err());
    assert_eq!(controller.registry.defaults().1, original);
    assert!(controller.filter.is_some());
    assert!(!controller
        .test_backend
        .as_ref()
        .unwrap()
        .actions
        .iter()
        .any(|action| matches!(action, Action::Default(_))));
    controller.test_backend.as_mut().unwrap().fail_stop = false;
    controller.shutdown().await.unwrap();
}

#[tokio::test]
async fn stale_recovery_cannot_mutate_default_while_an_owned_handle_exists() {
    let (_directory, mut controller) = controller();
    active(&mut controller);
    let original = controller.registry.defaults().1;
    assert_eq!(
        controller.recover_stale().await.unwrap_err().code,
        ErrorCode::Busy
    );
    assert_eq!(controller.registry.defaults().1, original);
    assert!(controller.test_backend.as_ref().unwrap().actions.is_empty());
    controller.shutdown().await.unwrap();
}

#[tokio::test]
async fn post_spawn_persistence_failure_keeps_handle_and_journal_until_stop_is_confirmed() {
    let (_directory, mut controller) = controller();
    controller
        .test_backend
        .as_mut()
        .unwrap()
        .post_spawn_store_fault = true;
    controller.test_backend.as_mut().unwrap().fail_stop = true;
    assert!(controller
        .apply_profile(&profile("alsa_output.headset", true))
        .await
        .is_err());
    let filter = controller
        .filter
        .as_mut()
        .expect("The spawned child must already be owned");
    assert_eq!(filter.record.filter_process_id, filter.process.id());
    assert!(filter.process.id().is_some());
    assert!(!filter.process.has_exited().unwrap());
    assert!(controller.recovery_pending);
    assert_eq!(controller.state, SurroundState::Error);
    assert!(!controller.snapshot().can_enable_surround);
    assert!(controller
        .test_backend
        .as_ref()
        .unwrap()
        .actions
        .iter()
        .all(|action| matches!(action, Action::Spawn | Action::Stop)));
    let file = controller.paths.state_dir.join("audio-restore.json");
    std::fs::remove_dir(&file).unwrap();
    std::fs::rename(
        controller.paths.state_dir.join("saved-pre-spawn.json"),
        file,
    )
    .unwrap();
    controller.test_backend.as_mut().unwrap().fail_stop = false;
    controller.refresh().await.unwrap();
    assert!(controller.filter.is_none());
    assert!(!controller.recovery_pending);
    assert!(surround::read_restore(&controller.paths).unwrap().is_none());
}

#[tokio::test]
async fn cancelling_start_preserves_global_ownership_for_recovery() {
    let (_directory, mut controller) = controller();
    controller.test_backend.as_mut().unwrap().hide_graph = true;
    assert!(
        tokio::time::timeout(Duration::from_millis(25), controller.enable_surround(None))
            .await
            .is_err()
    );
    assert!(controller.filter.is_some());
    assert!(controller.recovery_pending);
    assert!(surround::read_restore(&controller.paths).unwrap().is_some());
    controller.test_backend.as_mut().unwrap().hide_graph = false;
    controller.refresh().await.unwrap();
    assert!(controller.filter.is_none());
    assert!(!controller.recovery_pending);
}

#[tokio::test]
async fn cancelling_stop_keeps_the_handle_in_the_controller() {
    let (_directory, mut controller) = controller();
    active(&mut controller);
    let original = controller.registry.defaults().1;
    controller.test_backend.as_mut().unwrap().pause_stop = true;
    assert!(tokio::time::timeout(
        Duration::from_millis(25),
        controller.stop_filter(RestoreIntent::Bypass)
    )
    .await
    .is_err());
    assert!(controller.filter.is_some());
    assert!(controller.recovery_pending);
    assert_eq!(controller.registry.defaults().1, original);
    controller.test_backend.as_mut().unwrap().pause_stop = false;
    controller.refresh().await.unwrap();
    assert!(controller.filter.is_none());
    assert!(!controller.recovery_pending);
    assert!(surround::read_restore(&controller.paths).unwrap().is_none());
}
