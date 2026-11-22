use super::tests::{mouse, paths};
use super::*;
use std::sync::Arc;
use tokio::sync::{oneshot, Notify};

#[derive(Clone)]
struct FakeBackend {
    inner: Arc<Mutex<FakeState>>,
    apply_entered: Arc<Notify>,
}

struct FakeState {
    inventory: Inventory,
    states: HashMap<String, BridgeState>,
    stops: Vec<(String, String)>,
    applies: Vec<(String, String)>,
    events: Vec<String>,
    stop_failures: HashSet<String>,
    replacement_on_stop: HashMap<String, BridgeState>,
    inventory_failure: Option<AppError>,
    pending_apply: Option<(String, String)>,
    block_apply: bool,
}

fn state(identity: &str, preset: Option<String>, owner_uid: Option<u32>) -> BridgeState {
    BridgeState {
        version: 1,
        state: if preset.is_some() {
            "RUNNING"
        } else {
            "STOPPED"
        }
        .into(),
        preset,
        owner_uid,
        generation: 1,
        fingerprint: Some(identity.into()),
        message: None,
    }
}

fn preset_name(uid: u32) -> String {
    format!("ph-{uid}-{}-{}", Uuid::new_v4(), "a".repeat(64))
}

impl FakeBackend {
    fn event(&self, event: &str) {
        self.inner.lock().unwrap().events.push(event.into());
    }

    fn complete_pending_apply(&self) {
        let mut inner = self.inner.lock().unwrap();
        if let Some((identity, preset)) = inner.pending_apply.take() {
            inner.states.insert(
                identity.clone(),
                state(&identity, Some(preset), Some(current_uid())),
            );
            inner.events.push("late-apply-completed".into());
        }
    }

    async fn quiesce(&self) -> AppResult<()> {
        self.event("quiesce-user-agent");
        self.complete_pending_apply();
        Ok(())
    }
}

impl RemappingBackend for FakeBackend {
    fn owner(&self) -> &str {
        ":1.42"
    }

    async fn inventory(&self) -> AppResult<Inventory> {
        let mut inner = self.inner.lock().unwrap();
        inner.events.push("inventory".into());
        if let Some(error) = &inner.inventory_failure {
            return Err(error.clone());
        }
        Ok(inner.inventory.clone())
    }

    async fn state(&self, identity: &str) -> AppResult<BridgeState> {
        let mut inner = self.inner.lock().unwrap();
        inner.events.push(format!("state:{identity}"));
        Ok(inner
            .states
            .get(identity)
            .cloned()
            .unwrap_or_else(|| state(identity, None, None)))
    }

    async fn stop(&self, identity: &str, expected: &str) -> AppResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.events.push(format!("stop:{identity}"));
        inner.stops.push((identity.into(), expected.into()));
        if let Some(replacement) = inner.replacement_on_stop.remove(identity) {
            inner.states.insert(identity.into(), replacement);
        }
        let current = inner.states.get(identity).unwrap();
        if current.owner_uid != Some(current_uid()) || current.preset.as_deref() != Some(expected) {
            return Err(AppError::new(
                ErrorCode::Busy,
                "CAS o proprietario cambiato.",
            ));
        }
        if inner.stop_failures.contains(identity) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Fake stop non confermato.",
            ));
        }
        inner
            .states
            .insert(identity.into(), state(identity, None, None));
        Ok(())
    }

    async fn apply(
        &self,
        input: &InputDevice,
        _generation: u64,
        preset: &str,
        expected: &str,
        _directory: &Path,
    ) -> AppResult<()> {
        let blocked = {
            let mut inner = self.inner.lock().unwrap();
            let current = inner.states.get(&input.fingerprint).unwrap();
            if current.preset.as_deref().unwrap_or("") != expected {
                return Err(AppError::new(ErrorCode::Busy, "Fake apply CAS fallito."));
            }
            inner.events.push("apply".into());
            inner
                .applies
                .push((input.fingerprint.clone(), preset.into()));
            if inner.block_apply {
                inner.pending_apply = Some((input.fingerprint.clone(), preset.into()));
                true
            } else {
                inner.states.insert(
                    input.fingerprint.clone(),
                    state(&input.fingerprint, Some(preset.into()), Some(current_uid())),
                );
                false
            }
        };
        if blocked {
            self.apply_entered.notify_one();
            // The root request deliberately survives cancellation of its client.
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}

fn fixture(count: usize) -> (tempfile::TempDir, Controller, FakeBackend) {
    let temp = tempfile::tempdir().unwrap();
    let controller = Controller::new(paths(temp.path()));
    let lock = controller.store.lock().unwrap();
    let mut document = Document::default();
    let mut devices = Vec::new();
    let mut states = HashMap::new();
    for index in 0..count {
        let mut input = mouse();
        input.fingerprint = format!("mouse-{index}");
        input.id = input.fingerprint.clone();
        input.serial = Some(input.fingerprint.clone());
        let mut profile = RemappingProfile {
            id: Uuid::new_v4().to_string(),
            fingerprint: input.fingerprint.clone(),
            identity_strength: input.identity_strength,
            identity: input.identity(),
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
        let preset = controller.store.preset(&profile, &input).unwrap();
        profile.compiled_preset = Some(preset.clone());
        states.insert(
            input.fingerprint.clone(),
            state(&input.fingerprint, Some(preset), Some(current_uid())),
        );
        devices.push(input);
        document.profiles.push(profile);
    }
    controller.store.save(&mut document).unwrap();
    controller
        .store
        .disarm(
            &document
                .profiles
                .iter()
                .map(|p| p.fingerprint.clone())
                .collect::<Vec<_>>(),
            false,
        )
        .unwrap();
    drop(lock);
    let backend = FakeBackend {
        inner: Arc::new(Mutex::new(FakeState {
            inventory: Inventory {
                version: 1,
                generation: 1,
                keyboard_output_codes: vec![29, 30, 46],
                devices,
            },
            states,
            stops: Vec::new(),
            applies: Vec::new(),
            events: Vec::new(),
            stop_failures: HashSet::new(),
            replacement_on_stop: HashMap::new(),
            inventory_failure: None,
            pending_apply: None,
            block_apply: false,
        })),
        apply_entered: Arc::new(Notify::new()),
    };
    (temp, controller, backend)
}

fn no_space(_path: &Path, _bytes: &[u8]) -> AppResult<()> {
    Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into())
}

fn read_only(_path: &Path, _bytes: &[u8]) -> AppResult<()> {
    Err(std::io::Error::from_raw_os_error(libc::EROFS).into())
}

fn profile_write_fails(path: &Path, bytes: &[u8]) -> AppResult<()> {
    if path.file_name().unwrap() == "mouse-remapping.json" {
        no_space(path, bytes)
    } else {
        profiles::atomic_write(path, bytes)
    }
}

fn recovery_write_fails(path: &Path, bytes: &[u8]) -> AppResult<()> {
    if path.file_name().unwrap() == "remapping-recovery.json" {
        read_only(path, bytes)
    } else {
        profiles::atomic_write(path, bytes)
    }
}

fn rearm_is_renamed_before_io_error(path: &Path, bytes: &[u8]) -> AppResult<()> {
    profiles::atomic_write(path, bytes)?;
    if path.file_name().unwrap() == "remapping-recovery.json" {
        // Model a successful rename followed by a failed parent directory fsync.
        return read_only(path, bytes);
    }
    Ok(())
}

fn unlocked_write_is_a_bug(_path: &Path, _bytes: &[u8]) -> AppResult<()> {
    panic!("A failed lock must never lead to a document write");
}

#[tokio::test]
async fn full_and_read_only_stores_still_stop_every_owned_preset_after_quiescing() {
    for writer in [no_space as fn(&Path, &[u8]) -> AppResult<()>, read_only] {
        let (_temp, mut controller, backend) = fixture(3);
        controller.store.write_document = writer;
        let before = fs::read(controller.store.path()).unwrap();
        let late = preset_name(current_uid());
        backend.inner.lock().unwrap().pending_apply = Some(("mouse-0".into(), late.clone()));
        let connected = backend.clone();
        let stopped = backend.clone();
        let error = controller
            .disable_with(
                None,
                || async move { Ok(connected) },
                || async move { stopped.quiesce().await },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Io);
        assert!(error.message.contains("non salvata"));
        assert!(error
            .detail
            .as_ref()
            .unwrap()
            .contains("Salvataggio profili"));
        assert_eq!(fs::read(controller.store.path()).unwrap(), before);
        assert!(controller
            .store
            .load()
            .unwrap()
            .profiles
            .iter()
            .all(|p| p.enabled && p.auto_apply));
        assert!(controller.store.recovery().unwrap().disarmed.is_empty());
        assert!(controller.recovery_failure().is_some());
        let inner = backend.inner.lock().unwrap();
        assert_eq!(inner.stops.len(), 3);
        assert_eq!(inner.stops[0], ("mouse-0".into(), late));
        assert!(inner.states.values().all(|s| s.preset.is_none()));
        assert_eq!(inner.events[0], "quiesce-user-agent");
        assert!(
            inner
                .events
                .iter()
                .position(|e| e == "late-apply-completed")
                .unwrap()
                < inner
                    .events
                    .iter()
                    .position(|e| e.starts_with("stop:"))
                    .unwrap()
        );
    }
}

#[tokio::test]
async fn disarm_survives_profile_write_failure_and_flags_survive_recovery_write_failure() {
    for (writer, profiles_fail) in [
        (
            profile_write_fails as fn(&Path, &[u8]) -> AppResult<()>,
            true,
        ),
        (recovery_write_fails, false),
    ] {
        let (_temp, mut controller, backend) = fixture(2);
        controller.store.write_document = writer;
        let connected = backend.clone();
        let stopped = backend.clone();
        assert!(controller
            .disable_with(
                None,
                || async move { Ok(connected) },
                || async move { stopped.quiesce().await }
            )
            .await
            .is_err());
        let document = controller.store.load().unwrap();
        let recovery = controller.store.recovery().unwrap();
        if profiles_fail {
            assert!(document.profiles.iter().all(|p| p.enabled && p.auto_apply));
            assert_eq!(recovery.disarmed.len(), 2);
        } else {
            assert!(document
                .profiles
                .iter()
                .all(|p| !p.enabled && !p.auto_apply));
            assert!(recovery.disarmed.is_empty());
        }
        assert_eq!(backend.inner.lock().unwrap().stops.len(), 2);
    }
}

#[tokio::test]
async fn stop_errors_are_aggregated_without_skipping_remaining_presets() {
    let (_temp, controller, backend) = fixture(3);
    backend
        .inner
        .lock()
        .unwrap()
        .stop_failures
        .extend(["mouse-0".into(), "mouse-1".into()]);
    let connected = backend.clone();
    let error = controller
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async { panic!("Healthy persistence must not stop the user unit") },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotConfirmed);
    let detail = error.detail.unwrap();
    assert!(detail.contains("2 errori"));
    assert!(detail.contains("Stop mouse-0"));
    assert!(detail.contains("Stop mouse-1"));
    let inner = backend.inner.lock().unwrap();
    assert_eq!(inner.stops.len(), 3);
    assert!(inner.states["mouse-2"].preset.is_none());
}

#[tokio::test]
async fn persistence_error_is_not_hidden_by_quiesce_or_stop_errors() {
    let (_temp, mut controller, backend) = fixture(3);
    controller.store.write_document = no_space;
    backend
        .inner
        .lock()
        .unwrap()
        .stop_failures
        .insert("mouse-0".into());
    let connected = backend.clone();
    let error = controller
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async {
                Err(AppError::new(
                    ErrorCode::Timeout,
                    "Fake user unit non quiescente.",
                ))
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Io);
    let detail = error.detail.unwrap();
    assert!(detail.contains("Salvataggio profili"));
    assert!(detail.contains("Arresto agente utente"));
    assert!(detail.contains("Stop mouse-0"));
    assert_eq!(backend.inner.lock().unwrap().stops.len(), 3);
    assert!(backend.inner.lock().unwrap().states["mouse-2"]
        .preset
        .is_none());
}

#[tokio::test]
async fn relief_preserves_foreign_uid_and_foreign_preset_even_on_io_failure() {
    let (_temp, mut controller, backend) = fixture(3);
    controller.store.write_document = read_only;
    {
        let mut inner = backend.inner.lock().unwrap();
        inner.states.get_mut("mouse-1").unwrap().owner_uid = Some(current_uid().wrapping_add(1));
        inner.states.get_mut("mouse-2").unwrap().preset = Some("stock-or-foreign-preset".into());
    }
    let connected = backend.clone();
    let stopped = backend.clone();
    assert!(controller
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async move { stopped.quiesce().await }
        )
        .await
        .is_err());
    let inner = backend.inner.lock().unwrap();
    assert_eq!(inner.stops.len(), 1);
    assert_eq!(inner.stops[0].0, "mouse-0");
    assert!(inner.states["mouse-1"].preset.is_some());
    assert_eq!(
        inner.states["mouse-2"].preset.as_deref(),
        Some("stock-or-foreign-preset")
    );
}

#[tokio::test]
async fn failed_recovery_values_keep_persisted_flags_active_preset_and_foreign_busy() {
    let (_temp, mut controller, backend) = fixture(2);
    controller.store.write_document = profile_write_fails;
    {
        let mut inner = backend.inner.lock().unwrap();
        inner.stop_failures.insert("mouse-0".into());
        inner.states.get_mut("mouse-1").unwrap().owner_uid = Some(current_uid().wrapping_add(1));
    }
    let connected = backend.clone();
    let stopped = backend.clone();
    assert!(controller
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async move { stopped.quiesce().await }
        )
        .await
        .is_err());
    let saved = controller.store.load().unwrap();
    let recovery = controller.store.recovery().unwrap();
    let failure = controller.recovery_failure();
    let inner = backend.inner.lock().unwrap();
    let owned = inner.states["mouse-0"].values(
        Some(&saved.profiles[0]),
        recovery.disarmed.contains(&"mouse-0".into()),
        failure.as_ref(),
    );
    assert!(owned.enabled && owned.auto_apply);
    assert!(owned.active_preset.is_some());
    assert_eq!(owned.state, RemappingRuntimeState::Error);
    assert!(owned.message.unwrap().contains("non salvata"));
    let foreign = inner.states["mouse-1"].values(
        Some(&saved.profiles[1]),
        recovery.disarmed.contains(&"mouse-1".into()),
        failure.as_ref(),
    );
    assert_eq!(foreign.state, RemappingRuntimeState::Busy);
    assert!(foreign.active_preset.is_some());
}

#[tokio::test]
async fn relief_keeps_expected_preset_cas_when_owner_changes_during_stop() {
    let (_temp, controller, backend) = fixture(2);
    let replacement = preset_name(current_uid().wrapping_add(1));
    backend.inner.lock().unwrap().replacement_on_stop.insert(
        "mouse-0".into(),
        state(
            "mouse-0",
            Some(replacement.clone()),
            Some(current_uid().wrapping_add(1)),
        ),
    );
    let connected = backend.clone();
    let error = controller
        .disable_with(None, || async move { Ok(connected) }, || async { Ok(()) })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Busy);
    let inner = backend.inner.lock().unwrap();
    assert_eq!(inner.stops.len(), 2);
    assert_ne!(inner.stops[0].1, replacement);
    assert_eq!(inner.states["mouse-0"].preset.as_ref(), Some(&replacement));
    assert!(inner.states["mouse-1"].preset.is_none());
}

#[tokio::test]
async fn inventory_failure_does_not_prevent_stop_of_saved_identities() {
    let (_temp, controller, backend) = fixture(2);
    backend.inner.lock().unwrap().inventory_failure = Some(AppError::new(
        ErrorCode::Timeout,
        "Fake inventory indisponibile.",
    ));
    let connected = backend.clone();
    let error = controller
        .disable_with(None, || async move { Ok(connected) }, || async { Ok(()) })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Timeout);
    assert_eq!(backend.inner.lock().unwrap().stops.len(), 2);
}

#[tokio::test]
async fn lock_busy_never_overwrites_another_writers_newer_document() {
    let (_temp, mut controller, backend) = fixture(2);
    controller.store.write_document = unlocked_write_is_a_bug;
    let writer = Store::at(controller.store.paths.clone());
    let lock = writer.lock().unwrap();
    let mut document = writer.load().unwrap();
    let revision = document.revision;
    document.profiles[0].bindings[0].action = MouseAction::Key { code: 30 };
    let connected = backend.clone();
    let stopped = backend.clone();
    let error = controller
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async move {
                writer.save(&mut document).unwrap();
                drop(lock);
                stopped.quiesce().await
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Busy);
    let saved = controller.store.load().unwrap();
    assert_eq!(saved.revision, revision + 1);
    assert_eq!(
        saved.profiles[0].bindings[0].action,
        MouseAction::Key { code: 30 }
    );
    assert!(saved.profiles.iter().all(|p| p.enabled && p.auto_apply));
    assert_eq!(backend.inner.lock().unwrap().stops.len(), 2);
}

#[tokio::test]
async fn filesystem_lock_failure_still_quiesces_and_releases_without_writing() {
    let (_temp, mut controller, backend) = fixture(2);
    controller.store.write_document = unlocked_write_is_a_bug;
    let before = fs::read(controller.store.path()).unwrap();
    let lock = controller
        .store
        .paths
        .config_dir
        .join("mouse-remapping.lock");
    fs::remove_file(&lock).unwrap();
    fs::create_dir(&lock).unwrap();
    let connected = backend.clone();
    let stopped = backend.clone();
    let error = controller
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async move { stopped.quiesce().await },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(fs::read(controller.store.path()).unwrap(), before);
    assert_eq!(backend.inner.lock().unwrap().stops.len(), 2);
    assert_eq!(
        backend.inner.lock().unwrap().events[0],
        "quiesce-user-agent"
    );
}

#[tokio::test]
async fn unreadable_agent_location_does_not_overwrite_fallback_data_or_block_relief() {
    let (_temp, controller, backend) = fixture(2);
    let before = fs::read(controller.store.path()).unwrap();
    backend
        .inner
        .lock()
        .unwrap()
        .states
        .get_mut("mouse-1")
        .unwrap()
        .owner_uid = Some(current_uid().wrapping_add(1));
    let connected = backend.clone();
    let stopped = backend.clone();
    let error = recover_with_paths(
        Err(AppError::new(ErrorCode::Io, "Fake locator non leggibile.")),
        || async move { Ok(connected) },
        || async move { stopped.quiesce().await },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Io);
    assert!(error.message.contains("nessuna disattivazione persistente"));
    assert_eq!(fs::read(controller.store.path()).unwrap(), before);
    let inner = backend.inner.lock().unwrap();
    assert_eq!(inner.events[0], "quiesce-user-agent");
    assert_eq!(inner.stops.len(), 1);
    assert!(inner.states["mouse-0"].preset.is_none());
    assert!(inner.states["mouse-1"].preset.is_some());
}

#[tokio::test]
async fn shutdown_relief_needs_no_writer_lock_and_preserves_desired_flags() {
    let (_temp, mut controller, backend) = fixture(2);
    controller.store.write_document = unlocked_write_is_a_bug;
    let lock = controller.store.lock().unwrap();
    let before = fs::read(controller.store.path()).unwrap();
    let connected = backend.clone();
    tokio::time::timeout(
        Duration::from_millis(300),
        controller.release_with(|| async move { Ok(connected) }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(fs::read(controller.store.path()).unwrap(), before);
    assert!(controller
        .store
        .load()
        .unwrap()
        .profiles
        .iter()
        .all(|p| p.enabled && p.auto_apply));
    assert_eq!(backend.inner.lock().unwrap().stops.len(), 2);
    drop(lock);
}

#[tokio::test]
async fn successful_disable_retains_disarm_and_fresh_agent_does_not_autoapply() {
    let (_temp, controller, backend) = fixture(2);
    let connected = backend.clone();
    controller
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async { panic!("No emergency quiesce on healthy storage") },
        )
        .await
        .unwrap();
    let saved = controller.store.load().unwrap();
    assert!(saved.profiles.iter().all(|p| !p.enabled && !p.auto_apply));
    assert_eq!(
        controller.store.recovery().unwrap().disarmed,
        vec!["mouse-0", "mouse-1"]
    );
    let connected = backend.clone();
    AgentRuntime::default()
        .step(
            &Store::at(controller.store.paths.clone()),
            || async move { Ok(connected) },
            || async { Ok(true) },
        )
        .await
        .unwrap();
    assert!(backend.inner.lock().unwrap().applies.is_empty());
    assert_eq!(controller.store.recovery().unwrap().disarmed.len(), 2);
}

#[tokio::test]
async fn disarm_alone_blocks_restart_when_profile_write_failed() {
    let (_temp, mut controller, backend) = fixture(1);
    controller.store.write_document = profile_write_fails;
    let connected = backend.clone();
    let stopped = backend.clone();
    assert!(controller
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async move { stopped.quiesce().await }
        )
        .await
        .is_err());
    let connected = backend.clone();
    AgentRuntime::default()
        .step(
            &Store::at(controller.store.paths.clone()),
            || async move { Ok(connected) },
            || async { Ok(true) },
        )
        .await
        .unwrap();
    assert!(controller.store.load().unwrap().profiles[0].enabled);
    assert!(backend.inner.lock().unwrap().applies.is_empty());
}

#[tokio::test]
async fn failed_apply_finalization_quiesces_even_if_rearm_was_already_renamed() {
    let (_temp, mut controller, backend) = fixture(2);
    let lock = controller.store.lock().unwrap();
    controller.store.disarm(&["mouse-0".into()], true).unwrap();
    controller.store.write_document = rearm_is_renamed_before_io_error;
    let mut document = controller.store.load().unwrap();
    let late = preset_name(current_uid());
    backend.inner.lock().unwrap().pending_apply = Some(("mouse-0".into(), late.clone()));
    let stopped = backend.clone();
    let error = controller
        .finish_apply_with(&lock, &mut document, "mouse-0", &backend, || async move {
            stopped.quiesce().await
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Io);
    assert!(controller.store.recovery().unwrap().disarmed.is_empty());
    assert!(controller
        .store
        .load()
        .unwrap()
        .profiles
        .iter()
        .all(|p| p.enabled && p.auto_apply));
    assert!(controller
        .recovery_failure()
        .unwrap()
        .message
        .contains("non salvata"));
    let inner = backend.inner.lock().unwrap();
    assert_eq!(inner.stops.len(), 2);
    assert_eq!(inner.stops[0], ("mouse-0".into(), late));
    assert!(inner.pending_apply.is_none());
    assert!(inner.states.values().all(|s| s.preset.is_none()));
    assert_eq!(inner.events[0], "quiesce-user-agent");
}

#[tokio::test]
async fn corrupt_profiles_remain_intact_but_recovery_still_stops_owned_presets() {
    let (_temp, controller, backend) = fixture(2);
    {
        let _lock = controller.store.lock().unwrap();
        profiles::atomic_write(&controller.store.path(), b"corrupt original").unwrap();
    }
    let connected = backend.clone();
    let stopped = backend.clone();
    assert!(controller
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async move { stopped.quiesce().await }
        )
        .await
        .is_err());
    assert_eq!(
        fs::read(controller.store.path()).unwrap(),
        b"corrupt original"
    );
    assert_eq!(backend.inner.lock().unwrap().stops.len(), 2);
    assert_eq!(controller.store.recovery().unwrap().disarmed.len(), 2);
}

#[tokio::test]
async fn emergency_quiesce_cancels_in_flight_agent_and_stops_its_late_preset() {
    let (_temp, mut recovery, backend) = fixture(1);
    let agent = Arc::new(Controller::new(recovery.store.paths.clone()));
    recovery.store.write_document = unlocked_write_is_a_bug;
    {
        let mut inner = backend.inner.lock().unwrap();
        inner
            .states
            .insert("mouse-0".into(), state("mouse-0", None, None));
        inner.block_apply = true;
    }
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let agent_backend = backend.clone();
    let task = tokio::spawn(async move {
        let mut runtime = AgentRuntime::default();
        let connected = agent_backend.clone();
        assert!(
            !runtime
                .cycle(
                    &agent,
                    || async { Ok(connected.clone()) },
                    || async { Ok(true) },
                    async {
                        let _ = shutdown_rx.await;
                    }
                )
                .await
        );
        let connected = agent_backend.clone();
        agent
            .release_with(|| async move { Ok(connected) })
            .await
            .unwrap();
    });
    backend.apply_entered.notified().await;
    let connected = backend.clone();
    let stopped = backend.clone();
    let error = recovery
        .disable_with(
            None,
            || async move { Ok(connected) },
            || async move {
                stopped.event("quiesce-request");
                shutdown_tx.send(()).unwrap();
                task.await.unwrap();
                // A root RPC may complete after the agent client has already exited.
                stopped.complete_pending_apply();
                stopped.event("quiesce-confirmed");
                Ok(())
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Busy);
    let inner = backend.inner.lock().unwrap();
    assert_eq!(inner.applies.len(), 1);
    assert_eq!(inner.stops, inner.applies);
    assert!(inner.states["mouse-0"].preset.is_none());
    assert!(
        inner
            .events
            .iter()
            .position(|e| e == "quiesce-confirmed")
            .unwrap()
            < inner
                .events
                .iter()
                .position(|e| e.starts_with("stop:"))
                .unwrap()
    );
}

#[tokio::test]
async fn agent_storage_failure_ends_cycle_instead_of_rearming_after_fs_recovery() {
    let (_temp, mut controller, backend) = fixture(1);
    backend
        .inner
        .lock()
        .unwrap()
        .states
        .insert("mouse-0".into(), state("mouse-0", None, None));
    controller.store.write_document = read_only;
    let connected = backend.clone();
    let keep_running = AgentRuntime::default()
        .cycle(
            &controller,
            || async { Ok(connected.clone()) },
            || async { Ok(true) },
            std::future::pending(),
        )
        .await;
    assert!(!keep_running);
    controller.store.write_document = profiles::atomic_write;
    let connected = backend.clone();
    controller
        .release_with(|| async move { Ok(connected) })
        .await
        .unwrap();
    assert!(backend.inner.lock().unwrap().applies.is_empty());
    assert!(controller.store.load().unwrap().profiles[0].enabled);
}

#[tokio::test]
async fn healthy_agent_autoapply_still_works_and_transient_backend_failure_keeps_supervision() {
    let (_temp, controller, backend) = fixture(1);
    backend
        .inner
        .lock()
        .unwrap()
        .states
        .insert("mouse-0".into(), state("mouse-0", None, None));
    let connected = backend.clone();
    let mut runtime = AgentRuntime::default();
    assert!(
        runtime
            .cycle(
                &controller,
                || async { Ok(connected.clone()) },
                || async { Ok(true) },
                std::future::pending()
            )
            .await
    );
    assert_eq!(backend.inner.lock().unwrap().applies.len(), 1);
    assert!(controller.store.recovery().unwrap().disarmed.is_empty());
    backend.inner.lock().unwrap().inventory_failure = Some(AppError::new(
        ErrorCode::ProviderMissing,
        "Fake backend offline.",
    ));
    let connected = backend.clone();
    assert!(
        runtime
            .cycle(
                &controller,
                || async { Ok(connected.clone()) },
                || async { Ok(true) },
                std::future::pending()
            )
            .await
    );
    assert!(controller.store.load().unwrap().profiles[0].enabled);
}

#[test]
fn shared_output_keys_and_chord_modifiers_remain_valid_profiles() {
    let mut input = mouse();
    input.buttons.push(InputButton {
        id: "right".into(),
        label: "Destro".into(),
        code: 273,
        node_id: "mouse".into(),
        origin_hash: "hash".into(),
    });
    for actions in [
        [MouseAction::Key { code: 46 }, MouseAction::Key { code: 46 }],
        [
            MouseAction::Key { code: 29 },
            MouseAction::Chord {
                modifiers: vec![29],
                key: 46,
            },
        ],
        [
            MouseAction::Chord {
                modifiers: vec![29],
                key: 30,
            },
            MouseAction::Chord {
                modifiers: vec![29],
                key: 46,
            },
        ],
    ] {
        let bindings = vec![
            MouseBinding {
                button_id: "side".into(),
                action: actions[0].clone(),
            },
            MouseBinding {
                button_id: "right".into(),
                action: actions[1].clone(),
            },
        ];
        assert!(validate_bindings(&bindings, &input, &[29, 30, 46, 272, 273]).is_ok());
    }
}
