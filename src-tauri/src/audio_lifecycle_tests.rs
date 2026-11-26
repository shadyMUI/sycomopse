use super::*;
use crate::models::{EqualizerBand, EqualizerSettings};
use crate::profiles::ProfileStore;
use serde_json::json;
use std::process::Stdio;

const HEADSET: &str = "alsa_output.headset";
const OTHER: &str = "alsa_output.other";

#[derive(Debug, PartialEq)]
enum Action {
    Level(String, Option<f64>, Option<bool>),
    Equalizer(String, EqualizerSettings),
    Default(Option<String>),
    Spawn,
    Stop,
}

#[derive(Default)]
pub(crate) struct Backend {
    registry: Registry,
    selection: Option<HrtfSelection>,
    auto_output: Option<&'static str>,
    actions: Vec<Action>,
    fail_spawn: bool,
    journal_before_failure: bool,
    post_spawn_store_fault: bool,
    pause_stop: bool,
    hide_graph: bool,
    fail_stop: bool,
    fail_read: bool,
    fail_equalizer: bool,
    disconnect_after_default_readback: bool,
    default_read_back: bool,
    damage_published_graph: bool,
    hide_next_graph: bool,
    hidden_name: Option<String>,
    external_effective_on_spawn: Option<String>,
    external_configured_on_spawn: Option<String>,
    external_effective_after_equalizer: Option<String>,
    external_on_stop: Option<String>,
    external_mute_before_level: Option<bool>,
    external_volume_before_level: Option<f64>,
    generation: u64,
}

impl Backend {
    pub(super) fn read_registry(&mut self) -> AppResult<Registry> {
        // Let the write observe its new default once; disconnect at the next observation.
        if self.fail_read
            || (self.disconnect_after_default_readback
                && matches!(self.actions.last(), Some(Action::Default(_)))
                && std::mem::replace(&mut self.default_read_back, true))
        {
            return Err(AppError::new(
                ErrorCode::AudioUnavailable,
                "Fixture: server disconnesso.",
            ));
        }
        if let Some(Action::Default(Some(name))) = self.actions.last() {
            if std::mem::take(&mut self.damage_published_graph) {
                let id = self.registry.node(name)?.id;
                self.registry.objects.get_mut(&id).unwrap()["info"]["state"] = json!("error");
            }
        }
        let mut registry = self.registry.clone();
        if self.hide_graph || self.hidden_name.is_some() {
            let ids: Vec<_> = registry
                .objects
                .iter()
                .filter_map(|(id, object)| {
                    text_property(object, "node.name")
                        .is_some_and(|name| {
                            (self.hide_graph && name.starts_with(APP_NODE_PREFIX))
                                || Some(name) == self.hidden_name.as_deref()
                        })
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

    pub(super) fn write_equalizer(
        &mut self,
        node: &AudioNode,
        settings: &EqualizerSettings,
    ) -> AppResult<()> {
        let current = self.registry.node(&node.node_name)?;
        if current.id != node.id
            || current.object_serial.is_none()
            || current.object_serial != node.object_serial
            || current.media_class != node.media_class
        {
            return Err(AppError::new(
                ErrorCode::TargetUnavailable,
                "Fixture: istanza sostituita.",
            ));
        }
        let (effective, configured, _) = self.registry.defaults();
        let name = Some(node.node_name.as_str());
        if !current.is_virtual
            || current.media_class != "Audio/Sink"
            || !current.node_name.starts_with(APP_NODE_PREFIX)
            || effective.as_deref() != name
            || configured.as_deref() != name
        {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Fixture: route EQ non confermata.",
            ));
        }
        self.actions
            .push(Action::Equalizer(node.node_name.clone(), settings.clone()));
        if self.fail_equalizer {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Fixture: coefficienti EQ non confermati.",
            ));
        }
        // Success represents the worker's coefficient readback, not a level write.
        if let Some(external) = self.external_effective_after_equalizer.take() {
            self.defaults(configured.as_deref(), Some(&external));
        }
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
        expected_effective: Option<&str>,
    ) -> AppResult<()> {
        let (effective, configured, _) = self.registry.defaults();
        if configured.as_deref() != expected || effective.as_deref() != expected_effective {
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
        self.defaults(name.as_deref(), name.as_deref().or(self.auto_output));
        Ok(())
    }

    fn record(
        &self,
        target: &AudioNode,
        configured: Option<String>,
        effective: Option<String>,
    ) -> RestoreRecord {
        let session = uuid::Uuid::new_v4();
        RestoreRecord {
            stereo_eq: false,
            schema_version: 1,
            session_id: session.to_string(),
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
        hrtf: Option<&HrtfSelection>,
        eq: &EqualizerSettings,
        configured: Option<String>,
        effective: Option<String>,
    ) -> AppResult<ActiveFilter> {
        self.actions.push(Action::Spawn);
        let mut record = self.record(target, configured, effective);
        record.stereo_eq = hrtf.is_none();
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
        if record.stereo_eq {
            graph.retain(|o| !matches!(o["id"].as_u64(), Some(82..=87)));
            let input = graph.iter_mut().find(|o| o["id"] == 70).unwrap();
            input["info"]["props"]["audio.position"] = json!(["FL", "FR"]);
            input["info"]["params"]["Props"][0]["channelVolumes"] = json!([1, 1]);
        }
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
            let template_id = object["id"].as_u64().unwrap();
            object["id"] = json!(template_id + self.generation);
            if let Some(props) = object
                .pointer_mut("/info/props")
                .and_then(Value::as_object_mut)
            {
                let old = props.get("object.serial").and_then(number).unwrap();
                props.insert("object.serial".into(), json!(old + self.generation));
                for key in ["node.id", "client.id"] {
                    if let Some(id) = props.get_mut(key) {
                        *id = json!(id.as_u64().unwrap() + self.generation);
                    }
                }
                if props.contains_key("node.name") {
                    props.insert(
                        "node.name".into(),
                        json!(if template_id == 70 {
                            &record.virtual_node_name
                        } else {
                            &record.playback_node_name
                        }),
                    );
                    props.insert("peripheral-hub.session".into(), json!(record.session_id));
                }
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
                for key in ["output-node-id", "output-port-id"] {
                    object["info"][key] =
                        json!(object["info"][key].as_u64().unwrap() + self.generation);
                }
            }
        }
        self.registry.merge(graph)?;
        if std::mem::take(&mut self.hide_next_graph) {
            self.hidden_name = Some(record.virtual_node_name.clone());
        }
        if let Some(name) = self.external_effective_on_spawn.take() {
            let configured = self.registry.defaults().1;
            self.defaults(configured.as_deref(), Some(&name));
        }
        if let Some(name) = self.external_configured_on_spawn.take() {
            let effective = self.registry.defaults().0;
            self.defaults(Some(&name), effective.as_deref());
        }
        Ok(ActiveFilter {
            process,
            record,
            directory,
            hrtf_reference: hrtf.map(|s| s.status.reference.clone()),
            equalizer: eq.clone(),
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
                    || name == Some(record.playback_node_name.as_str())
                    || (is_type(object, ":Client")
                        && property(object, "application.process.id").and_then(super::id)
                            == record.filter_process_id))
                    .then_some(*id)
            })
            .collect();
        for id in ids {
            self.registry.invalidate_dependencies(id);
            self.registry.objects.remove(&id);
        }
        if self.registry.defaults().0.as_deref() == Some(&record.virtual_node_name) {
            // WirePlumber may immediately route audio on removal, without any controller write.
            let configured = self.registry.defaults().1;
            self.defaults(configured.as_deref(), self.auto_output);
        }
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
        auto_output: Some(HEADSET),
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
        equalizer: None,
    }
}

fn curve(gain_db: f64) -> EqualizerSettings {
    let mut eq = EqualizerSettings {
        enabled: true,
        ..Default::default()
    };
    eq.bands[9] = EqualizerBand { gain_db, q: 0.75 };
    eq
}

fn assert_processing(
    controller: &AudioController,
    target: &str,
    spatial: bool,
    eq: &EqualizerSettings,
) -> AudioNode {
    let snapshot = controller.snapshot();
    let filter = controller.filter.as_ref().expect("an owned filter");
    let capture = controller
        .registry
        .node(&filter.record.virtual_node_name)
        .unwrap();
    let playback = controller
        .registry
        .node(&filter.record.playback_node_name)
        .unwrap();
    let inputs: &[&str] = if spatial { &CHANNELS_71 } else { &["FL", "FR"] };
    assert_eq!(controller.registry.port_channels(capture.id, "in"), inputs);
    assert_eq!(
        controller.registry.port_channels(playback.id, "out"),
        ["FL", "FR"]
    );
    assert_eq!(filter.record.selected_physical_target, target);
    assert_eq!(filter.hrtf_reference.is_some(), spatial);
    assert_eq!(&filter.equalizer, eq);
    assert_eq!(snapshot.selected_output.as_deref(), Some(target));
    assert_eq!(
        snapshot.default_output.as_deref(),
        Some(capture.node_name.as_str())
    );
    assert_eq!(snapshot.configured_default_output, snapshot.default_output);
    assert_eq!(
        snapshot.equalizer_output.as_deref(),
        eq.enabled.then_some(target)
    );
    assert_eq!(
        snapshot.surround_state,
        if spatial {
            SurroundState::On
        } else {
            SurroundState::Off
        }
    );
    capture
}

#[tokio::test]
async fn muted_named_profile_handoffs_only_publish_prepared_virtual_defaults_until_final_bypass() {
    let (_dir, mut c) = controller();
    let b = c.test_backend.as_mut().unwrap();
    b.auto_output = Some(OTHER);
    b.defaults(None, Some(OTHER));
    let mut p = profile(HEADSET, true);
    p.equalizer = Some(curve(-2.0));
    p.volume = 0.125;
    c.apply_profile(&p).await.unwrap();
    let bypass = EqualizerSettings::default();
    for step in 0..6 {
        let old = c.filter.as_ref().unwrap().record.clone();
        c.test_backend.as_mut().unwrap().actions.clear();
        match step {
            0 => {
                let mut edited = p.clone();
                edited.equalizer = Some(curve(5.0));
                c.apply_profile(&edited).await
            }
            1 => c.set_equalizer(HEADSET, &bypass).await,
            2 => c.set_equalizer(HEADSET, &curve(-3.0)).await,
            3 => c.set_surround(false).await,
            4 => c.set_surround(true).await,
            _ => {
                p.output_node_name = OTHER.into();
                p.surround_requested = false;
                p.volume = 0.25;
                c.apply_profile(&p).await
            }
        }
        .unwrap();
        let new = &c.filter.as_ref().unwrap().record;
        assert_ne!(old.session_id, new.session_id);
        assert_ne!(old.virtual_object_serial, new.virtual_object_serial);
        assert_eq!(new.previous_configured_default, None);
        assert_eq!(new.previous_effective_default.as_deref(), Some(OTHER));
        assert_eq!(
            surround::read_restore(&c.paths).unwrap().as_ref(),
            Some(new)
        );
        assert!(surround::read_restore(&surround::handoff_paths(&c.paths))
            .unwrap()
            .is_none());
        let node = c.registry.node(&new.virtual_node_name).unwrap();
        assert_eq!((node.volume, node.muted), (Some(p.volume), Some(true)));
        assert!(verify_graph(&c.registry, new).unwrap());
        assert!(c.registry.node(&old.virtual_node_name).is_err());
        assert_eq!(
            c.registry.defaults().0.as_deref(),
            Some(new.virtual_node_name.as_str())
        );
        assert_eq!(
            c.test_backend.as_ref().unwrap().actions,
            [
                Action::Spawn,
                Action::Level(new.virtual_node_name.clone(), Some(p.volume), Some(true)),
                Action::Default(Some(new.virtual_node_name.clone())),
                Action::Stop,
            ],
            "transition {step}: only the prepared replacement may be published before teardown"
        );
    }
    c.test_backend.as_mut().unwrap().actions.clear();
    c.set_equalizer(OTHER, &bypass).await.unwrap();
    assert_eq!(
        c.test_backend.as_ref().unwrap().actions,
        [Action::Stop, Action::Default(None)]
    );
    assert_eq!(
        c.registry.defaults(),
        (
            Some(OTHER.into()),
            None,
            Some("alsa_input.microphone".into())
        )
    );
    assert!(surround::read_restore(&c.paths).unwrap().is_none());
}

#[tokio::test]
async fn replacement_aborts_on_either_external_metadata_change_and_never_reclaims_on_retirement() {
    for change in ["configured", "effective", "retire"] {
        let (_dir, mut c) = controller();
        active(&mut c);
        let old = c.filter.as_ref().unwrap().record.clone();
        let b = c.test_backend.as_mut().unwrap();
        match change {
            "configured" => b.external_configured_on_spawn = Some(OTHER.into()),
            "effective" => b.external_effective_on_spawn = Some(OTHER.into()),
            _ => b.external_on_stop = Some(OTHER.into()),
        }
        let mut edited = profile(HEADSET, true);
        edited.equalizer = Some(curve(-2.0));
        assert_eq!(
            c.apply_profile(&edited).await.unwrap_err().code,
            ErrorCode::NotConfirmed
        );
        let (effective, configured, _) = c.registry.defaults();
        if change == "retire" {
            assert_eq!(
                (effective.as_deref(), configured.as_deref()),
                (Some(OTHER), Some(OTHER))
            );
            assert_eq!(
                c.test_backend
                    .as_ref()
                    .unwrap()
                    .actions
                    .iter()
                    .filter(|a| matches!(a, Action::Default(_)))
                    .count(),
                1
            );
        } else {
            let owned = Some(old.virtual_node_name.as_str());
            let expected = if change == "configured" {
                (owned, Some(OTHER))
            } else {
                (Some(OTHER), owned)
            };
            assert_eq!((effective.as_deref(), configured.as_deref()), expected);
            assert_eq!(
                c.test_backend.as_ref().unwrap().actions,
                [Action::Spawn, Action::Stop]
            );
            assert_eq!(c.filter.as_ref().unwrap().record, old);
            assert!(verify_graph(&c.registry, &old).unwrap());
        }
        assert!(surround::read_restore(&surround::handoff_paths(&c.paths))
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn failure_after_publication_reselects_the_old_verified_muted_filter_before_stopping_the_new_one(
) {
    let (_dir, mut c) = controller();
    let eq = curve(-4.0);
    c.set_equalizer(HEADSET, &eq).await.unwrap();
    let old = c.filter.as_ref().unwrap().record.clone();
    c.write_level(&old.virtual_node_name, 0.125, true)
        .await
        .unwrap();
    let b = c.test_backend.as_mut().unwrap();
    b.damage_published_graph = true;
    b.actions.clear();
    let mut edited = profile(HEADSET, false);
    edited.equalizer = Some(curve(6.0));
    edited.volume = 0.125;
    assert_eq!(
        c.apply_profile(&edited).await.unwrap_err().code,
        ErrorCode::NotConfirmed
    );
    let actions = &c.test_backend.as_ref().unwrap().actions;
    let Action::Default(Some(next)) = &actions[2] else {
        panic!("candidate was not published: {actions:?}");
    };
    assert_ne!(next, &old.virtual_node_name);
    assert_eq!(
        actions,
        &[
            Action::Spawn,
            Action::Level(next.clone(), Some(0.125), Some(true)),
            Action::Default(Some(next.clone())),
            Action::Default(Some(old.virtual_node_name.clone())),
            Action::Stop
        ]
    );
    assert_eq!(c.filter.as_ref().unwrap().record, old);
    let node = assert_processing(&c, HEADSET, false, &eq);
    assert_eq!((node.volume, node.muted), (Some(0.125), Some(true)));
    assert!(surround::read_restore(&surround::handoff_paths(&c.paths))
        .unwrap()
        .is_none());
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelling_a_handoff_preserves_both_journals_and_retries_only_the_unrouted_child() {
    for published in [false, true] {
        let (_dir, mut c) = controller();
        let eq = curve(-4.0);
        c.set_equalizer(HEADSET, &eq).await.unwrap();
        let old = c.filter.as_ref().unwrap().record.clone();
        let b = c.test_backend.as_mut().unwrap();
        b.hide_next_graph = !published;
        b.pause_stop = published;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), c.set_surround(true))
                .await
                .is_err()
        );
        let staged = surround::handoff_paths(&c.paths);
        let new = surround::read_restore(&staged).unwrap().unwrap();
        assert_ne!(new.session_id, old.session_id);
        assert_eq!(surround::read_restore(&c.paths).unwrap().unwrap(), old);
        assert!(c.recovery_pending);
        let live = if published { new } else { old };
        assert_eq!(
            c.test_backend
                .as_ref()
                .unwrap()
                .registry
                .defaults()
                .1
                .as_deref(),
            Some(live.virtual_node_name.as_str())
        );
        let b = c.test_backend.as_mut().unwrap();
        b.pause_stop = false;
        b.hidden_name = None;
        b.actions.clear();
        c.refresh().await.unwrap();
        assert_eq!(
            assert_processing(&c, HEADSET, published, &eq).node_name,
            live.virtual_node_name
        );
        assert_eq!(c.test_backend.as_ref().unwrap().actions, [Action::Stop]);
        assert_eq!(surround::read_restore(&c.paths).unwrap(), Some(live));
        assert!(surround::read_restore(&staged).unwrap().is_none());
        c.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn restart_recovers_the_routed_journal_even_if_the_other_graph_is_still_visible() {
    for published in [false, true] {
        let (_dir, mut c) = controller();
        active(&mut c);
        let (paths, staged) = (c.paths.clone(), surround::handoff_paths(&c.paths));
        let old = c.filter.as_ref().unwrap().record.clone();
        let b = c.test_backend.as_mut().unwrap();
        b.hide_next_graph = !published;
        b.pause_stop = published;
        let mut edited = profile(HEADSET, true);
        edited.equalizer = Some(curve(2.0));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), c.apply_profile(&edited))
                .await
                .is_err()
        );
        let new = surround::read_restore(&staged).unwrap().unwrap();
        let mut b = c.test_backend.take().unwrap();
        drop(c);
        b.hidden_name = None;
        b.actions.clear();
        let mut restarted = AudioController::new(paths.clone());
        restarted.test_backend = Some(b);
        assert_eq!(restarted.refresh().await.unwrap_err().code, ErrorCode::Busy);
        assert_eq!(
            restarted.test_backend.as_ref().unwrap().actions,
            [Action::Default(None)]
        );
        assert!(surround::read_restore(&paths).unwrap().is_some());
        assert!(surround::read_restore(&staged).unwrap().is_some());
        let b = restarted.test_backend.as_mut().unwrap();
        b.stopped(&old);
        b.stopped(&new);
        restarted.refresh().await.unwrap();
        assert!(!restarted.recovery_pending);
        assert!(surround::read_restore(&paths).unwrap().is_none());
        assert!(surround::read_restore(&staged).unwrap().is_none());
        assert_eq!(
            restarted.test_backend.as_ref().unwrap().actions,
            [Action::Default(None)]
        );
    }
}

#[tokio::test]
async fn failed_retirement_keeps_both_handles_and_journals_for_refresh_or_shutdown() {
    for shutdown in [false, true] {
        let (_dir, mut c) = controller();
        c.set_equalizer(HEADSET, &curve(-4.0)).await.unwrap();
        let old = c.filter.as_ref().unwrap().record.clone();
        c.test_backend.as_mut().unwrap().fail_stop = true;
        assert_eq!(
            c.set_surround(true).await.unwrap_err().code,
            ErrorCode::Timeout
        );
        let staged = surround::handoff_paths(&c.paths);
        let new = surround::read_restore(&staged).unwrap().unwrap();
        assert!(c.recovery_pending);
        assert_eq!(
            c.registry.defaults().0.as_ref(),
            Some(&new.virtual_node_name)
        );
        assert!(
            verify_graph(&c.registry, &old).unwrap() && verify_graph(&c.registry, &new).unwrap()
        );
        assert_eq!(
            surround::read_restore(&c.paths).unwrap().as_ref(),
            Some(&old)
        );
        if shutdown {
            assert_eq!(c.shutdown().await.unwrap_err().code, ErrorCode::Timeout);
        }
        let b = c.test_backend.as_mut().unwrap();
        b.fail_stop = false;
        b.actions.clear();
        c.refresh().await.unwrap();
        if shutdown {
            assert_eq!(
                c.test_backend.as_ref().unwrap().actions,
                [Action::Stop, Action::Stop, Action::Default(None)]
            );
            assert!(c.filter.is_none());
            assert!(surround::read_restore(&c.paths).unwrap().is_none());
        } else {
            assert_eq!(c.test_backend.as_ref().unwrap().actions, [Action::Stop]);
            assert_eq!(surround::read_restore(&c.paths).unwrap(), Some(new));
            assert_processing(&c, HEADSET, true, &curve(-4.0));
            c.shutdown().await.unwrap();
        }
        assert!(!c.recovery_pending);
        assert!(surround::read_restore(&staged).unwrap().is_none());
    }
}

#[tokio::test]
async fn a_damaged_staged_journal_cannot_overwrite_or_stop_the_working_route() {
    let (_dir, mut c) = controller();
    let eq = curve(-4.0);
    c.set_equalizer(HEADSET, &eq).await.unwrap();
    let old = c.filter.as_ref().unwrap().record.clone();
    let b = c.test_backend.as_mut().unwrap();
    b.post_spawn_store_fault = true;
    b.fail_stop = true;
    b.actions.clear();
    assert!(c.set_surround(true).await.is_err());
    assert!(c.recovery_pending);
    assert_eq!(
        c.test_backend.as_ref().unwrap().actions,
        [Action::Spawn, Action::Stop]
    );
    assert_eq!(
        c.registry.defaults().1.as_ref(),
        Some(&old.virtual_node_name)
    );
    assert_eq!(
        surround::read_restore(&c.paths).unwrap().as_ref(),
        Some(&old)
    );

    let staged = surround::handoff_paths(&c.paths);
    assert!(surround::read_restore(&staged).is_err());
    let b = c.test_backend.as_mut().unwrap();
    b.fail_stop = false;
    assert!(c.refresh().await.is_err());
    assert_eq!(
        c.registry.defaults().1.as_ref(),
        Some(&old.virtual_node_name)
    );
    assert!(surround::read_restore(&staged).is_err());
    let file = staged.state_dir.join("audio-restore.json");
    std::fs::remove_dir(&file).unwrap();
    std::fs::rename(staged.state_dir.join("saved-pre-spawn.json"), file).unwrap();
    c.refresh().await.unwrap();
    assert!(!c.recovery_pending);
    assert_eq!(
        assert_processing(&c, HEADSET, false, &eq).node_name,
        old.virtual_node_name
    );
    assert!(surround::read_restore(&staged).unwrap().is_none());
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn stereo_eq_without_sofa_reports_only_the_routed_physical_output() {
    let (_directory, mut controller) = controller();
    std::fs::remove_file(controller.hrtf.take().unwrap().path).unwrap();
    controller.test_backend.as_mut().unwrap().selection = None;

    let eq = curve(3.0);
    controller.set_equalizer(HEADSET, &eq).await.unwrap();
    controller.refresh().await.unwrap();
    assert_processing(&controller, HEADSET, false, &eq);
    assert!(controller.snapshot().hrtf.is_none());
    assert!(!controller.snapshot().can_enable_surround);
    assert!(
        surround::read_restore(&controller.paths)
            .unwrap()
            .unwrap()
            .stereo_eq
    );

    controller
        .test_backend
        .as_mut()
        .unwrap()
        .defaults(Some(OTHER), Some(OTHER));
    controller.refresh().await.unwrap();
    let snapshot = controller.snapshot();
    assert_eq!(snapshot.default_output.as_deref(), Some(OTHER));
    assert_eq!(snapshot.equalizer_output, None);
    assert_eq!(snapshot.equalizers.get(HEADSET), Some(&eq));
    controller.shutdown().await.unwrap();
    assert_eq!(controller.registry.defaults().1.as_deref(), Some(OTHER));
}

#[tokio::test]
async fn stereo_eq_stops_if_its_capture_gains_a_surround_port() {
    let (_directory, mut controller) = controller();
    let eq = curve(-2.5);
    controller.set_equalizer(HEADSET, &eq).await.unwrap();
    let capture = assert_processing(&controller, HEADSET, false, &eq);
    let registry = &mut controller.test_backend.as_mut().unwrap().registry;
    let mut extra = registry.ports(capture.id, "in")[0].1.clone();
    extra["id"] = json!(130);
    extra["info"]["props"]["object.serial"] = json!(5000);
    extra["info"]["props"]["audio.channel"] = json!("FC");
    registry.merge(vec![extra]).unwrap();

    assert_eq!(
        controller.refresh().await.unwrap_err().code,
        ErrorCode::NotConfirmed
    );
    assert!(controller.filter.is_none());
    let snapshot = controller.snapshot();
    assert_eq!(snapshot.default_output.as_deref(), Some(HEADSET));
    assert_eq!(snapshot.equalizer_output, None);
    assert_eq!(snapshot.equalizers.get(HEADSET), Some(&eq));
    assert!(surround::read_restore(&controller.paths).unwrap().is_none());
}

#[tokio::test]
async fn bypassing_either_effect_preserves_the_other_and_the_virtual_level() {
    let (_directory, mut controller) = controller();
    let eq = curve(4.5);
    let bypass = EqualizerSettings {
        enabled: false,
        ..eq.clone()
    };
    controller.set_equalizer(HEADSET, &eq).await.unwrap();
    let capture = assert_processing(&controller, HEADSET, false, &eq);
    controller
        .write_level(&capture.node_name, 0.25, true)
        .await
        .unwrap();

    controller
        .set_surround_for(true, Some(HEADSET))
        .await
        .unwrap();
    assert_processing(&controller, HEADSET, true, &eq);
    controller.set_equalizer(HEADSET, &bypass).await.unwrap();
    let capture = assert_processing(&controller, HEADSET, true, &bypass);
    assert_eq!((capture.volume, capture.muted), (Some(0.25), Some(true)));
    assert_eq!(controller.snapshot().equalizers.get(HEADSET), Some(&bypass));

    controller.set_equalizer(HEADSET, &eq).await.unwrap();
    assert_processing(&controller, HEADSET, true, &eq);
    controller.set_surround(false).await.unwrap();
    let capture = assert_processing(&controller, HEADSET, false, &eq);
    assert_eq!((capture.volume, capture.muted), (Some(0.25), Some(true)));
    controller.set_equalizer(HEADSET, &bypass).await.unwrap();
    let snapshot = controller.snapshot();
    assert!(controller.filter.is_none());
    assert_eq!(snapshot.surround_state, SurroundState::Off);
    assert_eq!(snapshot.equalizer_output, None);
    assert_eq!(snapshot.default_output.as_deref(), Some(HEADSET));
    assert!(surround::read_restore(&controller.paths).unwrap().is_none());
}

#[tokio::test]
async fn live_eq_edits_after_refresh_preserve_the_filter_and_level_in_both_modes() {
    for spatial in [false, true] {
        let (_dir, mut c) = controller();
        if spatial {
            c.set_surround(true).await.unwrap();
        }
        let original = curve(-4.0);
        c.set_equalizer(HEADSET, &original).await.unwrap();
        let capture = assert_processing(&c, HEADSET, spatial, &original);
        c.write_level(&capture.node_name, 0.25, true).await.unwrap();
        let record = c.filter.as_ref().unwrap().record.clone();

        let mut next = EqualizerSettings {
            preamp_db: Some(0.0),
            ..curve(6.0)
        };
        for q in [1.25, 2.0] {
            next.bands[9].q = q;
            c.test_backend.as_mut().unwrap().actions.clear();
            c.set_equalizer(HEADSET, &next).await.unwrap();
            c.refresh().await.unwrap();

            let live = assert_processing(&c, HEADSET, spatial, &next);
            assert_eq!(
                (live.id, live.object_serial),
                (capture.id, capture.object_serial)
            );
            assert_eq!((live.volume, live.muted), (Some(0.25), Some(true)));
            assert_eq!(
                surround::read_restore(&c.paths).unwrap().as_ref(),
                Some(&record)
            );
            assert!(surround::read_restore(&surround::handoff_paths(&c.paths))
                .unwrap()
                .is_none());
            let saved = c.snapshot().equalizers;
            assert_eq!(saved.get(HEADSET), Some(&next));
            assert_eq!(equalizer::load(&c.paths).unwrap(), saved);
            assert_eq!(
                c.test_backend.as_ref().unwrap().actions,
                [Action::Equalizer(capture.node_name.clone(), next.clone())]
            );
        }
        c.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn stale_routes_and_unconfirmed_live_eq_writes_do_not_save_the_proposed_curve() {
    let (_dir, mut c) = controller();
    let original = curve(-4.0);
    c.set_equalizer(HEADSET, &original).await.unwrap();
    let capture = assert_processing(&c, HEADSET, false, &original);
    let owned = Some(capture.node_name.as_str());
    let proposed = EqualizerSettings {
        preamp_db: Some(0.0),
        ..curve(6.0)
    };

    for (case, configured, effective, fail_write, change_after_write) in [
        ("stale configured default", Some(OTHER), owned, false, None),
        ("stale effective default", owned, Some(OTHER), false, None),
        ("unconfirmed coefficient write", owned, owned, true, None),
        (
            "effective default changed after write",
            owned,
            owned,
            false,
            Some(OTHER),
        ),
    ] {
        let backend = c.test_backend.as_mut().unwrap();
        backend.defaults(configured, effective);
        backend.actions.clear();
        backend.fail_equalizer = fail_write;
        backend.external_effective_after_equalizer = change_after_write.map(String::from);

        assert_eq!(
            c.set_equalizer(HEADSET, &proposed)
                .await
                .expect_err(case)
                .code,
            ErrorCode::NotConfirmed,
            "{case}"
        );
        let backend = c.test_backend.as_ref().unwrap();
        if fail_write || change_after_write.is_some() {
            assert_eq!(
                backend.actions,
                [Action::Equalizer(
                    capture.node_name.clone(),
                    proposed.clone()
                )]
            );
        } else {
            assert!(backend.actions.is_empty());
        }
        let (actual_effective, actual_configured, _) = backend.registry.defaults();
        assert_eq!(
            actual_effective.as_deref(),
            change_after_write.or(effective)
        );
        assert_eq!(actual_configured.as_deref(), configured);
        let saved = c.snapshot().equalizers;
        assert_eq!(saved.get(HEADSET), Some(&original));
        assert_eq!(equalizer::load(&c.paths).unwrap(), saved);
    }
    c.test_backend.as_mut().unwrap().defaults(owned, owned);
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn surround_handoffs_inherit_amplified_virtual_levels_without_touching_the_physical_sink() {
    let (_dir, mut c) = controller();
    let eq = curve(-4.0);
    c.set_equalizer(HEADSET, &eq).await.unwrap();
    let mut capture = assert_processing(&c, HEADSET, false, &eq);
    let physical = c.registry.node(HEADSET).unwrap();

    for (spatial, muted) in [(true, false), (false, true)] {
        let backend = c.test_backend.as_mut().unwrap();
        // Another client can set virtual levels above the app's 0..=1 volume control.
        backend.registry.objects.get_mut(&capture.id).unwrap()["info"]["params"]["Props"] = json!([{
            "channelVolumes": vec![1.25_f64.powi(3); capture.channels.len()],
            "mute": muted,
        }]);
        backend.actions.clear();

        c.set_surround(spatial).await.unwrap();
        let next = assert_processing(&c, HEADSET, spatial, &eq);
        assert_ne!(next.id, capture.id);
        assert!((next.volume.unwrap() - 1.25).abs() < 0.002);
        assert_eq!(next.muted, Some(muted));
        let current_physical = c.registry.node(HEADSET).unwrap();
        assert_eq!(
            (current_physical.volume, current_physical.muted),
            (physical.volume, physical.muted)
        );
        assert!(!c
            .test_backend
            .as_ref()
            .unwrap()
            .actions
            .iter()
            .any(|action| matches!(action, Action::Level(name, _, _) if name == HEADSET)));
        capture = next;
    }
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn stale_output_selection_cannot_enable_eq_or_surround() {
    // Includes automatic defaults, a pending configured choice, and a stale UI target.
    for (requested, configured, effective) in [
        (HEADSET, Some(OTHER), OTHER),
        (HEADSET, None, OTHER),
        (HEADSET, Some(OTHER), HEADSET),
        (OTHER, Some(OTHER), OTHER),
    ] {
        let (_directory, mut controller) = controller();
        controller
            .test_backend
            .as_mut()
            .unwrap()
            .defaults(configured, Some(effective));
        assert_eq!(
            controller
                .set_equalizer(requested, &curve(2.0))
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotConfirmed
        );
        assert_eq!(
            controller
                .set_surround_for(true, Some(requested))
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotConfirmed
        );
        let backend = controller.test_backend.as_ref().unwrap();
        assert_eq!(backend.registry.defaults().0.as_deref(), Some(effective));
        assert_eq!(backend.registry.defaults().1.as_deref(), configured);
        assert!(
            backend.actions.is_empty(),
            "unexpected writes: {:?}",
            backend.actions
        );
        assert!(!controller.paths.config_dir.exists());
        assert!(controller.snapshot().equalizer_output.is_none());
    }
}

#[tokio::test]
async fn effective_default_change_during_filter_start_is_not_overwritten() {
    for spatial in [false, true] {
        let (_directory, mut controller) = controller();
        controller
            .test_backend
            .as_mut()
            .unwrap()
            .external_effective_on_spawn = Some(OTHER.into());
        let result = if spatial {
            controller.set_surround_for(true, Some(HEADSET)).await
        } else {
            controller.set_equalizer(HEADSET, &curve(3.0)).await
        };
        let backend = controller.test_backend.as_ref().unwrap();
        assert_eq!(
            backend.registry.defaults().0.as_deref(),
            Some(OTHER),
            "spatial={spatial}, result={result:?}, actions={:?}",
            backend.actions
        );
        assert_eq!(backend.registry.defaults().1, None);
        assert_eq!(result.unwrap_err().code, ErrorCode::NotConfirmed);
        assert!(!backend
            .actions
            .iter()
            .any(|action| matches!(action, Action::Default(_))));
        assert!(controller.filter.is_none());
        assert!(controller.snapshot().equalizer_output.is_none());
        assert!(surround::read_restore(&controller.paths).unwrap().is_none());
    }
}

#[tokio::test]
async fn per_output_curves_load_without_routing_until_explicit_selection() {
    let (_directory, mut controller) = controller();
    let headset = curve(5.0);
    let other = curve(-7.5);
    controller.set_equalizer(HEADSET, &headset).await.unwrap();
    controller.set_default_output(OTHER).await.unwrap();
    assert_eq!(controller.snapshot().equalizer_output, None);
    controller.set_equalizer(OTHER, &other).await.unwrap();
    assert_processing(&controller, OTHER, false, &other);
    controller.shutdown().await.unwrap();

    let mut restarted = AudioController::new(controller.paths.clone());
    let mut backend = controller.test_backend.take().unwrap();
    backend.defaults(None, Some(HEADSET));
    backend.actions.clear();
    restarted.test_backend = Some(backend);
    restarted.refresh().await.unwrap();
    let snapshot = restarted.snapshot();
    assert_eq!(snapshot.equalizers.get(HEADSET), Some(&headset));
    assert_eq!(snapshot.equalizers.get(OTHER), Some(&other));
    assert_eq!(snapshot.default_output.as_deref(), Some(HEADSET));
    assert_eq!(snapshot.configured_default_output, None);
    assert_eq!(snapshot.equalizer_output, None);
    assert!(restarted.filter.is_none());
    assert!(restarted.test_backend.as_ref().unwrap().actions.is_empty());

    for (target, eq) in [(HEADSET, &headset), (OTHER, &other)] {
        restarted.set_default_output(target).await.unwrap();
        assert_processing(&restarted, target, false, eq);
    }
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_eq_controls_and_nonphysical_targets_make_no_writes() {
    let (_directory, mut controller) = controller();
    let original_defaults = controller.registry.defaults();
    let mut store = ProfileStore::at(controller.paths.clone());
    for (gain_db, q) in [
        (-12.01, 1.0),
        (12.01, 1.0),
        (f64::NAN, 1.0),
        (f64::INFINITY, 1.0),
        (0.0, 0.39),
        (0.0, 4.01),
        (0.0, f64::NAN),
        (0.0, f64::NEG_INFINITY),
    ] {
        let mut eq = curve(gain_db);
        eq.bands[9].q = q;
        assert_eq!(
            controller
                .set_equalizer(HEADSET, &eq)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        let mut named = profile(HEADSET, false);
        named.equalizer = Some(eq);
        assert_eq!(
            controller.apply_profile(&named).await.unwrap_err().code,
            ErrorCode::Validation
        );
        assert_eq!(store.save(named).unwrap_err().code, ErrorCode::Validation);
    }
    for target in ["alsa_input.microphone", "virtual.other"] {
        let eq = curve(1.0);
        assert_eq!(
            controller
                .set_equalizer(target, &eq)
                .await
                .unwrap_err()
                .code,
            ErrorCode::TargetUnavailable
        );
        let mut named = profile(target, false);
        named.equalizer = Some(eq);
        assert_eq!(
            controller.apply_profile(&named).await.unwrap_err().code,
            ErrorCode::TargetUnavailable
        );
    }
    let backend = controller.test_backend.as_ref().unwrap();
    assert!(
        backend.actions.is_empty(),
        "unexpected writes: {:?}",
        backend.actions
    );
    assert_eq!(backend.registry.defaults(), original_defaults);
    assert!(controller.snapshot().equalizers.is_empty());
    assert!(!controller.paths.config_dir.exists());
    assert!(!controller.paths.state_dir.exists());
    assert!(!controller.paths.runtime_dir.exists());
}

#[tokio::test]
async fn legacy_and_current_named_profiles_use_the_target_outputs_curve() {
    let (_directory, mut controller) = controller();
    let headset = curve(-4.0);
    let other = curve(5.5);
    controller.set_equalizer(HEADSET, &headset).await.unwrap();
    let file = controller.paths.config_dir.join("profiles.json");
    let legacy = json!({"schemaVersion": 1, "profiles": [{
        "schemaVersion": 1, "id": "f2cf91ba-a142-4b59-b09a-0910877c0d91", "name": "Legacy",
        "outputNodeName": OTHER, "volume": 0.25, "mute": true, "hrtf": null, "surroundRequested": false
    }]});
    std::fs::write(&file, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let mut store = ProfileStore::at(controller.paths.clone());
    let legacy = store.load().unwrap().pop().unwrap();
    assert_eq!(legacy.equalizer, None);

    controller.apply_profile(&legacy).await.unwrap();
    let snapshot = controller.snapshot();
    assert_eq!(snapshot.default_output.as_deref(), Some(OTHER));
    assert_eq!(snapshot.equalizer_output, None);
    assert!(!snapshot.equalizers.contains_key(OTHER));
    assert_eq!(snapshot.equalizers.get(HEADSET), Some(&headset));

    let mut current = profile(OTHER, true);
    current.equalizer = Some(other.clone());
    store.save(current.clone()).unwrap();
    let saved = ProfileStore::at(controller.paths.clone()).load().unwrap();
    assert_eq!(saved, [legacy, current]);
    controller.apply_profile(&saved[1]).await.unwrap();
    let capture = assert_processing(&controller, OTHER, true, &other);
    assert_eq!(capture.muted, Some(true));
    assert!((capture.volume.unwrap() - saved[1].volume).abs() < 0.002);

    controller.set_default_output(HEADSET).await.unwrap();
    assert_processing(&controller, HEADSET, false, &headset);
    controller.apply_profile(&saved[0]).await.unwrap();
    let capture = assert_processing(&controller, OTHER, false, &other);
    assert_eq!((capture.volume, capture.muted), (Some(0.25), Some(true)));
    assert_eq!(
        controller.snapshot().equalizers.get(HEADSET),
        Some(&headset)
    );
    controller.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_named_eq_rebuild_keeps_the_owned_filter_and_recovers_its_staging_journal() {
    let (_directory, mut controller) = controller();
    let original = curve(-3.0);
    controller.set_equalizer(HEADSET, &original).await.unwrap();
    let old = controller.filter.as_ref().unwrap().record.clone();
    let backend = controller.test_backend.as_mut().unwrap();
    backend.actions.clear();
    backend.fail_spawn = true;
    backend.journal_before_failure = true;
    let next = curve(6.0);
    let mut edited = profile(HEADSET, false);
    edited.equalizer = Some(next.clone());
    assert_eq!(
        controller.apply_profile(&edited).await.unwrap_err().code,
        ErrorCode::Io
    );

    assert!(!controller.recovery_pending);
    assert_eq!(
        controller.test_backend.as_ref().unwrap().actions,
        [Action::Spawn]
    );
    assert_eq!(
        surround::read_restore(&controller.paths).unwrap(),
        Some(old.clone())
    );
    assert!(
        surround::read_restore(&surround::handoff_paths(&controller.paths))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        assert_processing(&controller, HEADSET, false, &original).node_name,
        old.virtual_node_name
    );
    let snapshot = controller.snapshot();
    assert_eq!(snapshot.equalizers.get(HEADSET), Some(&original));
    assert_eq!(
        equalizer::load(&controller.paths).unwrap(),
        snapshot.equalizers
    );

    controller.test_backend.as_mut().unwrap().fail_spawn = false;
    controller.refresh().await.unwrap();
    controller.apply_profile(&edited).await.unwrap();
    assert_processing(&controller, HEADSET, false, &next);
    controller.shutdown().await.unwrap();
}

#[tokio::test]
async fn final_read_failure_rolls_back_direct_and_named_eq_changes() {
    for named in [false, true] {
        let (_directory, mut controller) = controller();
        let original = curve(-4.0);
        controller.set_equalizer(HEADSET, &original).await.unwrap();
        let bypass = EqualizerSettings {
            enabled: false,
            ..original.clone()
        };
        if named {
            controller.set_default_output(OTHER).await.unwrap();
        }
        let backend = controller.test_backend.as_mut().unwrap();
        backend.actions.clear();
        backend.disconnect_after_default_readback = true;
        let result = if named {
            let mut profile = profile(HEADSET, false);
            profile.equalizer = Some(bypass);
            controller.apply_profile(&profile).await
        } else {
            controller.set_equalizer(HEADSET, &bypass).await
        };
        let snapshot = controller.snapshot();
        assert_eq!(result.unwrap_err().code, ErrorCode::AudioUnavailable);
        assert!(controller.test_backend.as_ref().unwrap().default_read_back);
        assert!(!snapshot.available);
        assert_eq!(snapshot.equalizer_output, None);
        let on_disk = equalizer::load(&controller.paths).unwrap();
        assert_eq!(
            snapshot.equalizers.get(HEADSET),
            Some(&original),
            "named={named}"
        );
        assert_eq!(on_disk, snapshot.equalizers, "named={named}");
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
            Some(&selection),
            &Default::default(),
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
    assert!(controller
        .registry
        .removed_serials
        .get(&40)
        .is_none_or(|serial| *serial < 200));
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
    assert_eq!(actions, &[Action::Spawn]);
    assert_eq!(
        controller.registry.defaults().1.as_deref(),
        Some("virtual.other")
    );
    assert_eq!(controller.state, SurroundState::On);
    assert!(controller.filter.is_some()); // still available to its pinned streams
    controller.shutdown().await.unwrap();
    assert_eq!(
        controller.registry.defaults().1.as_deref(),
        Some("virtual.other")
    );
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
        tokio::time::timeout(Duration::from_millis(25), controller.set_surround(true))
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

#[tokio::test]
async fn surround_off_stops_pinned_clients_without_reclaiming_an_external_default() {
    let (_dir, mut controller) = controller();
    let mut named = profile(HEADSET, true);
    let eq = curve(-4.);
    named.equalizer = Some(eq.clone());
    controller.apply_profile(&named).await.unwrap();
    for name in [HEADSET, OTHER] {
        let backend = controller.test_backend.as_mut().unwrap();
        backend.defaults(Some(name), Some(name));
        backend.actions.clear();
        controller.refresh().await.unwrap();
        assert_eq!(
            controller.snapshot().owned_filter_output.as_deref(),
            Some(HEADSET)
        );
        assert_eq!(controller.snapshot().equalizer_output, None);
        controller.set_surround(false).await.unwrap();
        assert!(controller.filter.is_none());
        assert_eq!(controller.snapshot().default_output.as_deref(), Some(name));
        assert_eq!(controller.snapshot().equalizers.get(HEADSET), Some(&eq));
        assert_eq!(
            controller.test_backend.as_ref().unwrap().actions,
            [Action::Stop]
        );
        if name == HEADSET {
            controller.apply_profile(&named).await.unwrap();
        }
    }
}

#[tokio::test]
async fn switching_a_headphone_to_mono_keeps_its_stereo_curve_saved_not_active() {
    let (_dir, mut controller) = controller();
    let eq = curve(-4.);
    controller.set_equalizer(HEADSET, &eq).await.unwrap();
    controller.set_default_output(OTHER).await.unwrap();
    let backend = controller.test_backend.as_mut().unwrap();
    let node = backend.registry.objects.get_mut(&40).unwrap();
    node["info"]["props"]["audio.position"] = json!(["MONO"]);
    node["info"]["params"]["Props"][0]["channelVolumes"] = json!([1.]);
    backend.actions.clear();
    controller.set_default_output(HEADSET).await.unwrap();
    assert_eq!(
        controller.snapshot().default_output.as_deref(),
        Some(HEADSET)
    );
    assert_eq!(controller.snapshot().equalizers.get(HEADSET), Some(&eq));
    assert!(controller.filter.is_none());
    assert!(!controller
        .test_backend
        .as_ref()
        .unwrap()
        .actions
        .contains(&Action::Spawn));
}

#[tokio::test]
async fn old_stereo_profile_still_selects_its_explicit_hrtf_without_starting_surround() {
    let (_dir, mut controller) = controller();
    let reference = controller.hrtf.take().unwrap().status.reference;
    let mut legacy = profile(HEADSET, false);
    legacy.hrtf = Some(reference.clone());
    controller.apply_profile(&legacy).await.unwrap();
    assert_eq!(controller.snapshot().hrtf.unwrap().reference, reference);
    assert_eq!(controller.state, SurroundState::Off);
    assert!(controller.filter.is_none());
}

#[tokio::test]
async fn cancelled_preset_never_becomes_the_saved_curve() {
    let (_dir, mut controller) = controller();
    let previous = EqualizerSettings::default();
    controller.set_equalizer(HEADSET, &previous).await.unwrap();
    let expected = std::fs::read(controller.paths.config_dir.join("equalizers.json")).unwrap();
    controller.test_backend.as_mut().unwrap().hide_graph = true;
    let mut profile = profile(HEADSET, false);
    profile.equalizer = Some(curve(5.));
    assert!(tokio::time::timeout(
        Duration::from_millis(25),
        controller.apply_profile(&profile)
    )
    .await
    .is_err());
    assert_eq!(controller.snapshot().equalizers[HEADSET], previous);
    assert_eq!(
        std::fs::read(controller.paths.config_dir.join("equalizers.json")).unwrap(),
        expected
    );
    assert!(std::fs::read_dir(&controller.paths.config_dir)
        .unwrap()
        .all(|f| !f
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".pending")));
    controller.test_backend.as_mut().unwrap().hide_graph = false;
    controller.shutdown().await.unwrap();
}
