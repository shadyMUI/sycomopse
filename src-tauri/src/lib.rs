pub mod audio;
pub mod audio_worker;
#[cfg(feature = "desktop")]
mod commands;
pub mod devices;
pub mod models;
pub mod process;
pub mod profiles;
pub mod providers;
pub mod remapping;
pub mod surround;

#[cfg(feature = "desktop")]
use models::*;
#[cfg(feature = "desktop")]
use std::path::PathBuf;
#[cfg(feature = "desktop")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "desktop")]
use std::sync::{Arc, Mutex as StdMutex};
#[cfg(feature = "desktop")]
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
#[cfg(feature = "desktop")]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
#[cfg(all(feature = "desktop", target_os = "linux"))]
use tauri::Emitter;
#[cfg(feature = "desktop")]
use tauri::{AppHandle, Manager};
#[cfg(all(feature = "desktop", target_os = "linux"))]
use tokio::sync::Mutex;
#[cfg(feature = "desktop")]
use tokio::sync::RwLock;

#[cfg(feature = "desktop")]
pub(crate) enum AudioMutation {
    DefaultOutput(String),
    Volume(String, f64),
    Mute(String, bool),
    Surround(bool),
    Import(PathBuf),
    Apply(AudioProfile),
}

#[cfg(feature = "desktop")]
pub(crate) struct AppState {
    snapshot: RwLock<Snapshot>,
    tasks: StdMutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
    monitor_task: StdMutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    shutdown_started: AtomicBool,
    exit_ready: AtomicBool,
    tray_created: AtomicBool,
    #[cfg(target_os = "linux")]
    hardware: Mutex<()>,
    #[cfg(target_os = "linux")]
    remapping_lock: Mutex<()>,
    #[cfg(target_os = "linux")]
    audio: Mutex<audio::AudioController>,
    #[cfg(target_os = "linux")]
    profiles: Mutex<profiles::ProfileStore>,
    #[cfg(target_os = "linux")]
    remapping: remapping::Controller,
}

#[cfg(feature = "desktop")]
impl AppState {
    fn new() -> AppResult<Self> {
        #[cfg(target_os = "linux")]
        let profiles = profiles::ProfileStore::new()?;
        #[cfg(target_os = "linux")]
        let audio = audio::AudioController::new(profiles.paths().clone());
        #[cfg(target_os = "linux")]
        let remapping = remapping::Controller::new(profiles.paths().clone());
        Ok(Self {
            snapshot: RwLock::new(Snapshot::default()),
            tasks: StdMutex::new(Vec::new()),
            monitor_task: StdMutex::new(None),
            shutdown_started: AtomicBool::new(false),
            exit_ready: AtomicBool::new(false),
            tray_created: AtomicBool::new(false),
            #[cfg(target_os = "linux")]
            hardware: Mutex::new(()),
            #[cfg(target_os = "linux")]
            remapping_lock: Mutex::new(()),
            #[cfg(target_os = "linux")]
            audio: Mutex::new(audio),
            #[cfg(target_os = "linux")]
            profiles: Mutex::new(profiles),
            #[cfg(target_os = "linux")]
            remapping,
        })
    }

    pub(crate) fn require_platform(&self) -> AppResult<()> {
        if !cfg!(target_os = "linux") {
            return Err(AppError::unsupported_platform());
        }
        if self.shutdown_started.load(Ordering::SeqCst) {
            return Err(AppError::new(
                ErrorCode::Busy,
                "Chiusura in corso: nessuna nuova operazione consentita.",
            ));
        }
        Ok(())
    }

    pub(crate) async fn get_snapshot(&self) -> AppResult<Snapshot> {
        self.require_platform()?;
        Ok(self.snapshot.read().await.clone())
    }

    #[cfg(target_os = "linux")]
    async fn publish(&self, app: &AppHandle, update: impl FnOnce(&mut Snapshot)) -> Snapshot {
        let mut snapshot = self.snapshot.write().await;
        let mut next = snapshot.clone();
        update(&mut next);
        let changed = match (
            serde_json::to_vec(&(
                &next.devices,
                &next.audio,
                &next.profiles,
                &next.diagnostics,
            )),
            serde_json::to_vec(&(
                &snapshot.devices,
                &snapshot.audio,
                &snapshot.profiles,
                &snapshot.diagnostics,
            )),
        ) {
            (Ok(next_bytes), Ok(cur_bytes)) => next_bytes != cur_bytes,
            _ => true,
        };
        if changed {
            next.revision = snapshot.revision.saturating_add(1);
            *snapshot = next.clone();
            drop(snapshot);
            let _ = app.emit("snapshot-changed", &next);
            next
        } else {
            snapshot.clone()
        }
    }

    #[cfg(target_os = "linux")]
    async fn publish_audio(
        &self,
        app: &AppHandle,
        audio: AudioSnapshot,
        error: Option<&AppError>,
    ) -> Snapshot {
        self.publish(app, |snapshot| {
            snapshot.audio = audio;
            snapshot
                .diagnostics
                .retain(|diagnostic| diagnostic.id != "audio-runtime");
            let diagnostic = match error {
                Some(error) => {
                    diagnostic_from_error("audio-runtime", ProviderKind::Pipewire, error)
                }
                None => Diagnostic::new(
                    "audio-runtime",
                    ProviderKind::Pipewire,
                    if snapshot.audio.available {
                        AvailabilityCode::Available
                    } else {
                        AvailabilityCode::Absent
                    },
                    Severity::Info,
                    if snapshot.audio.available {
                        "PipeWire connesso. Routing per nuovi stream auto-collegati."
                    } else {
                        "Nessuna uscita PipeWire disponibile."
                    },
                ),
            };
            snapshot.diagnostics.push(diagnostic);
        })
        .await
    }

    #[cfg(target_os = "linux")]
    async fn refresh_audio(&self, app: &AppHandle) {
        let mut audio = self.audio.lock().await;
        let result = audio.refresh().await;
        self.publish_audio(app, audio.snapshot(), result.as_ref().err())
            .await;
    }

    #[cfg(target_os = "linux")]
    async fn refresh_hardware_locked(
        &self,
        app: &AppHandle,
        include_batteries: bool,
        preserve: Option<(Device, bool)>,
    ) -> Snapshot {
        let previous = self.snapshot.read().await.clone();
        let nodes: Vec<_> = previous
            .audio
            .outputs
            .iter()
            .chain(&previous.audio.inputs)
            .cloned()
            .collect();
        let mut discovery = devices::discover(&nodes, include_batteries).await;
        discovery.devices.extend(
            previous
                .devices
                .iter()
                .filter(|device| device.provider == ProviderKind::Inputremapper)
                .cloned(),
        );
        self.remapping.decorate(&mut discovery).await;
        for device in &mut discovery.devices {
            let old = previous.devices.iter().find(|old| {
                old.id == device.id
                    || crate::providers::identities_match(&old.identity, &device.identity)
            });
            if device.values.staged {
                device.values.dpi = old.and_then(|old| old.values.dpi);
                device.values.polling_rate = old.and_then(|old| old.values.polling_rate);
            }
            if device.values.dpi.is_none() {
                if let Some(old_dpi) = old.and_then(|old| old.values.dpi) {
                    device.values.dpi = Some(old_dpi);
                }
            }
            if device.values.polling_rate.is_none() {
                if let Some(old_rate) = old.and_then(|old| old.values.polling_rate) {
                    device.values.polling_rate = Some(old_rate);
                }
            }
            if !include_batteries {
                if let Some(old) = old {
                    if device.values.battery.is_none() {
                        device.values.battery = old.values.battery.clone().map(|mut battery| {
                            battery.stale |= now_secs().saturating_sub(battery.observed_at) > 90;
                            battery
                        });
                    }
                }
            }
            // An unconfirmed ratbag staging value must not replace the last confirmed UI value.
            if let Some((old_dev, is_dpi)) = &preserve {
                if old_dev.id == device.id
                    || crate::providers::identities_match(&old_dev.identity, &device.identity)
                {
                    if *is_dpi {
                        device.values.dpi = old_dev.values.dpi;
                    } else {
                        device.values.polling_rate = old_dev.values.polling_rate;
                    }
                }
            }
        }
        self.publish(app, |snapshot| {
            snapshot.devices = discovery.devices;
            snapshot.diagnostics.retain(|diagnostic| {
                matches!(
                    diagnostic.provider,
                    ProviderKind::Pipewire | ProviderKind::App
                )
            });
            snapshot.diagnostics.extend(discovery.diagnostics);
        })
        .await
    }

    #[cfg(target_os = "linux")]
    async fn refresh_hardware(&self, app: &AppHandle, include_batteries: bool) -> Snapshot {
        let _guard = self.hardware.lock().await;
        self.refresh_hardware_locked(app, include_batteries, None)
            .await
    }

    #[cfg(target_os = "linux")]
    async fn refresh_remapping_locked(&self, app: &AppHandle) -> Snapshot {
        let previous = self.snapshot.read().await.clone();
        let mut discovery = Discovery {
            devices: previous.devices,
            diagnostics: Vec::new(),
        };
        self.remapping.decorate(&mut discovery).await;
        self.publish(app, |snapshot| {
            snapshot.devices = discovery.devices;
            snapshot
                .diagnostics
                .retain(|item| item.provider != ProviderKind::Inputremapper);
            snapshot.diagnostics.extend(discovery.diagnostics);
        })
        .await
    }

    pub(crate) async fn mutate_remapping(
        &self,
        app: &AppHandle,
        device_id: &str,
        bindings: Option<Vec<MouseBinding>>,
        auto_apply: bool,
        allow_weak_identity: bool,
    ) -> AppResult<Snapshot> {
        self.require_platform()?;
        #[cfg(target_os = "linux")]
        {
            let _guard = self.remapping_lock.lock().await;
            let device = self
                .snapshot
                .read()
                .await
                .devices
                .iter()
                .find(|device| device.id == device_id)
                .cloned()
                .ok_or_else(|| {
                    AppError::new(
                        ErrorCode::DeviceNotFound,
                        "Mouse non presente nella snapshot.",
                    )
                })?;
            let result = match bindings {
                Some(bindings) => {
                    self.remapping
                        .apply(&device, bindings, auto_apply, allow_weak_identity)
                        .await
                }
                None => self.remapping.disable(&device).await,
            };
            let snapshot = self.refresh_remapping_locked(app).await;
            if let Err(error) = result {
                self.publish(app, |snapshot| {
                    snapshot
                        .diagnostics
                        .retain(|item| item.id != "remapping-command");
                    snapshot.diagnostics.push(diagnostic_from_error(
                        "remapping-command",
                        ProviderKind::Inputremapper,
                        &error,
                    ));
                })
                .await;
                return Err(error);
            }
            Ok(snapshot)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (app, device_id, bindings, auto_apply, allow_weak_identity);
            Err(AppError::unsupported_platform())
        }
    }

    pub(crate) async fn refresh_all(&self, app: &AppHandle) -> AppResult<Snapshot> {
        self.require_platform()?;
        #[cfg(target_os = "linux")]
        {
            self.refresh_audio(app).await;
            Ok(self.refresh_hardware(app, true).await)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = app;
            Err(AppError::unsupported_platform())
        }
    }

    #[cfg(target_os = "linux")]
    async fn device(&self, id: &str) -> AppResult<Device> {
        self.snapshot
            .read()
            .await
            .devices
            .iter()
            .find(|device| device.id == id)
            .cloned()
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::DeviceNotFound,
                    "Dispositivo non piu' presente. Aggiorna la discovery.",
                )
            })
    }

    pub(crate) async fn set_dpi(
        &self,
        app: &AppHandle,
        id: &str,
        x: u32,
        y: u32,
    ) -> AppResult<Snapshot> {
        self.require_platform()?;
        #[cfg(target_os = "linux")]
        {
            let _guard = self.hardware.lock().await;
            self.require_platform()?;
            let device = self.device(id).await?;
            let capability = device.capabilities.dpi.as_ref().ok_or_else(|| {
                AppError::new(
                    ErrorCode::Unsupported,
                    "DPI hardware non disponibili per questo dispositivo.",
                )
            })?;
            if !capability.writable
                || !capability.values.contains(x)
                || !capability.values.contains(y)
                || (!capability.separate_axes && x != y)
            {
                return Err(AppError::new(
                    ErrorCode::Validation,
                    capability
                        .reason
                        .as_deref()
                        .unwrap_or("DPI o assi non ammessi dal provider proprietario."),
                ));
            }
            let result = devices::set_dpi(&device, x, y).await;
            let confirmed = self
                .refresh_hardware_locked(app, false, result.as_ref().err().map(|_| (device, true)))
                .await;
            result?;
            Ok(confirmed)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (app, id, x, y);
            Err(AppError::unsupported_platform())
        }
    }

    pub(crate) async fn set_polling_rate(
        &self,
        app: &AppHandle,
        id: &str,
        rate: u32,
    ) -> AppResult<Snapshot> {
        self.require_platform()?;
        #[cfg(target_os = "linux")]
        {
            let _guard = self.hardware.lock().await;
            self.require_platform()?;
            let device = self.device(id).await?;
            let capability = device.capabilities.polling.as_ref().ok_or_else(|| {
                AppError::new(
                    ErrorCode::Unsupported,
                    "Polling hardware non disponibile per questo dispositivo.",
                )
            })?;
            if !capability.writable || !capability.values.contains(&rate) {
                return Err(AppError::new(
                    ErrorCode::Validation,
                    capability
                        .reason
                        .as_deref()
                        .unwrap_or("Polling non ammesso dal provider proprietario."),
                ));
            }
            let result = devices::set_polling_rate(&device, rate).await;
            let confirmed = self
                .refresh_hardware_locked(app, false, result.as_ref().err().map(|_| (device, false)))
                .await;
            result?;
            Ok(confirmed)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (app, id, rate);
            Err(AppError::unsupported_platform())
        }
    }

    pub(crate) async fn mutate_audio(
        &self,
        app: &AppHandle,
        mutation: AudioMutation,
    ) -> AppResult<Snapshot> {
        self.require_platform()?;
        #[cfg(target_os = "linux")]
        {
            let mut audio = self.audio.lock().await;
            self.require_platform()?;
            let requested = match &mutation {
                AudioMutation::Surround(enabled) => Some(*enabled),
                AudioMutation::Apply(profile) => Some(profile.surround_requested),
                AudioMutation::DefaultOutput(_)
                    if audio.snapshot().surround_state == SurroundState::On =>
                {
                    Some(true)
                }
                _ => None,
            };
            if let Some(enabled) = requested {
                let mut pending = audio.snapshot();
                if (enabled && pending.surround_state != SurroundState::On)
                    || (!enabled && pending.surround_state != SurroundState::Off)
                {
                    pending.surround_state = if enabled {
                        SurroundState::Starting
                    } else {
                        SurroundState::Stopping
                    };
                    self.publish(app, |snapshot| snapshot.audio = pending).await;
                }
            }
            let result = match mutation {
                AudioMutation::DefaultOutput(name) => audio.set_default_output(&name).await,
                AudioMutation::Volume(name, volume) => audio.set_volume(&name, volume).await,
                AudioMutation::Mute(name, muted) => audio.set_mute(&name, muted).await,
                AudioMutation::Surround(enabled) => audio.set_surround(enabled).await,
                AudioMutation::Import(path) => audio.import_hrtf(path).await,
                AudioMutation::Apply(profile) => audio.apply_profile(&profile).await,
            };
            let confirmed = self
                .publish_audio(app, audio.snapshot(), result.as_ref().err())
                .await;
            result?;
            Ok(confirmed)
        }
        #[cfg(not(target_os = "linux"))]
        {
            // Consume typed fields too: they never reach a CLI or a DBus bus on other OSes.
            match mutation {
                AudioMutation::DefaultOutput(name) => drop(name),
                AudioMutation::Volume(name, volume) => {
                    let _ = (name, volume);
                }
                AudioMutation::Mute(name, muted) => {
                    let _ = (name, muted);
                }
                AudioMutation::Surround(enabled) => {
                    let _ = enabled;
                }
                AudioMutation::Import(path) => drop(path),
                AudioMutation::Apply(profile) => drop(profile),
            }
            let _ = app;
            Err(AppError::unsupported_platform())
        }
    }

    pub(crate) async fn save_profile(
        &self,
        app: &AppHandle,
        profile: AudioProfile,
    ) -> AppResult<Snapshot> {
        self.require_platform()?;
        #[cfg(target_os = "linux")]
        {
            let mut store = self.profiles.lock().await;
            self.require_platform()?;
            let result = store.save(profile);
            drop(store);
            let confirmed = self
                .publish(app, |snapshot| {
                    snapshot
                        .diagnostics
                        .retain(|diagnostic| diagnostic.id != "profiles-write");
                    match &result {
                        Ok(profiles) => snapshot.profiles = profiles.clone(),
                        Err(error) => snapshot.diagnostics.push(diagnostic_from_error(
                            "profiles-write",
                            ProviderKind::App,
                            error,
                        )),
                    }
                })
                .await;
            result?;
            Ok(confirmed)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (app, profile);
            Err(AppError::unsupported_platform())
        }
    }

    pub(crate) async fn apply_profile(&self, app: &AppHandle, id: &str) -> AppResult<Snapshot> {
        self.require_platform()?;
        let profile = self
            .snapshot
            .read()
            .await
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .cloned()
            .ok_or_else(|| {
                AppError::new(ErrorCode::Validation, "Profilo non trovato o non valido.")
            })?;
        self.mutate_audio(app, AudioMutation::Apply(profile)).await
    }

    #[cfg(target_os = "linux")]
    async fn handle_audio_event(&self, app: &AppHandle, event: audio::AudioEvent) {
        let mut audio = self.audio.lock().await;
        let topology = |snapshot: AudioSnapshot| -> Vec<_> {
            snapshot
                .outputs
                .into_iter()
                .chain(snapshot.inputs)
                .map(|node| (node.node_name, node.id, node.object_serial))
                .collect()
        };
        let before = topology(audio.snapshot());
        let result = audio.handle_event(event).await;
        let after = audio.snapshot();
        let changed = before != topology(after.clone());
        self.publish_audio(app, after, result.as_ref().err()).await;
        drop(audio);
        if changed {
            self.refresh_hardware(app, false).await;
        }
    }

    #[cfg(target_os = "linux")]
    fn track(&self, task: tauri::async_runtime::JoinHandle<()>) {
        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(task);
    }
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
fn diagnostic_from_error(id: &str, provider: ProviderKind, error: &AppError) -> Diagnostic {
    let code = match error.code {
        ErrorCode::ProviderMissing => AvailabilityCode::ProviderMissing,
        ErrorCode::PermissionDenied => AvailabilityCode::PermissionDenied,
        ErrorCode::Unsupported | ErrorCode::UnsupportedPlatform => AvailabilityCode::Unsupported,
        _ => AvailabilityCode::Unknown,
    };
    let mut diagnostic =
        Diagnostic::new(id, provider, code, Severity::Warning, error.message.clone());
    diagnostic.detail = error.detail.clone();
    diagnostic
}

#[cfg(feature = "desktop")]
fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window.show() {
            eprintln!("Mostra finestra: {error}");
        }
        if let Err(error) = window.unminimize() {
            eprintln!("Ripristino finestra: {error}");
        }
        if let Err(error) = window.set_focus() {
            eprintln!("Focus finestra: {error}");
        }
    }
}

#[cfg(feature = "desktop")]
fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match (window.is_visible(), window.is_minimized()) {
        (Ok(true), Ok(false)) => {
            if let Err(error) = window.hide() {
                eprintln!("Nascondi finestra: {error}");
            }
        }
        (Ok(_), Ok(_)) => show_window(app),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("Stato finestra: {error}");
            show_window(app);
        }
    }
}

#[cfg(feature = "desktop")]
pub(crate) fn request_exit(app: AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if state.shutdown_started.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let tasks = std::mem::take(
            &mut *state
                .tasks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        let monitor = state
            .monitor_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(mut monitor) = monitor {
            if tokio::time::timeout(std::time::Duration::from_secs(3), &mut monitor)
                .await
                .is_err()
            {
                monitor.abort();
                let _ = monitor.await;
            }
        }
        #[cfg(target_os = "linux")]
        if let Err(error) = state.audio.lock().await.shutdown().await {
            eprintln!("Ripristino audio alla chiusura: {}", error.message);
        }
        state.exit_ready.store(true, Ordering::SeqCst);
        app.exit(0);
    });
}

#[cfg(all(feature = "desktop", test, not(target_os = "linux")))]
mod platform_tests {
    use super::*;

    #[tokio::test]
    async fn native_non_linux_snapshot_is_never_a_demo() {
        let state = AppState::new().unwrap();
        assert!(!state.snapshot.read().await.demo);
        assert_eq!(
            state.get_snapshot().await.unwrap_err().code,
            ErrorCode::UnsupportedPlatform
        );
    }
}

#[cfg(feature = "desktop")]
fn menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "open" | "tray-open" => show_window(app),
        "surround-off" => {
            let app = app.clone();
            let state = app.state::<Arc<AppState>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let _ = state
                    .mutate_audio(&app, AudioMutation::Surround(false))
                    .await;
            });
        }
        "exit" | "tray-exit" => request_exit(app.clone()),
        _ => {}
    }
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
async fn tray_host_available() -> bool {
    let check = async {
        let connection = zbus::Connection::session().await.ok()?;
        let bus = zbus::fdo::DBusProxy::new(&connection).await.ok()?;
        let name = zbus::names::BusName::try_from("org.kde.StatusNotifierWatcher").ok()?;
        if !bus.name_has_owner(name).await.ok()? {
            return Some(false);
        }
        let watcher = zbus::Proxy::new(
            &connection,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
        )
        .await
        .ok()?;
        Some(
            watcher
                .get_property::<bool>("IsStatusNotifierHostRegistered")
                .await
                .unwrap_or(false),
        )
    };
    tokio::time::timeout(std::time::Duration::from_secs(1), check)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
fn start_workers(app: &AppHandle, state: &Arc<AppState>) {
    let (audio_sender, mut audio_receiver) = tokio::sync::mpsc::channel(audio::MONITOR_CAPACITY);
    let (device_sender, mut device_receiver) = tokio::sync::mpsc::channel(1);
    *state
        .monitor_task
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(tauri::async_runtime::spawn(audio::monitor(audio_sender)));
    state.track(tauri::async_runtime::spawn(devices::watch_changes(
        device_sender,
    )));
    let worker_app = app.clone();
    let worker_state = state.clone();
    state.track(tauri::async_runtime::spawn(async move {
        let profiles = worker_state.profiles.lock().await.load();
        worker_state.publish(&worker_app, |snapshot| match profiles {
            Ok(profiles) => snapshot.profiles = profiles,
            Err(error) => snapshot.diagnostics.push(diagnostic_from_error("profiles-load", ProviderKind::App, &error)),
        }).await;
        worker_state.refresh_audio(&worker_app).await;
        worker_state.refresh_hardware(&worker_app, true).await;
        let mut battery_tick = tokio::time::interval(std::time::Duration::from_secs(30));
        battery_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        battery_tick.tick().await;
        loop {
            tokio::select! {
                Some(event) = audio_receiver.recv() => worker_state.handle_audio_event(&worker_app, event).await,
                Some(()) = device_receiver.recv() => {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    while device_receiver.try_recv().is_ok() {}
                    worker_state.refresh_hardware(&worker_app, false).await;
                }
                _ = battery_tick.tick() => { worker_state.refresh_hardware(&worker_app, true).await; }
            }
        }
    }));
    let remapping_app = app.clone();
    let remapping_state = state.clone();
    state.track(tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let _guard = remapping_state.remapping_lock.lock().await;
            remapping_state
                .refresh_remapping_locked(&remapping_app)
                .await;
        }
    }));
    let sync_app = app.clone();
    let sync_state = state.clone();
    state.track(tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(1500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let devices = sync_state.snapshot.read().await.devices.clone();
            let mut hardware_changed = false;
            for device in &devices {
                if device.provider == ProviderKind::Openrazer && device.kind == DeviceKind::Mouse {
                    if let Some((new_dpi, new_rate)) = crate::providers::openrazer::check_hardware_change(device).await {
                        sync_state.publish(&sync_app, |snapshot| {
                            for d in &mut snapshot.devices {
                                if d.id == device.id || crate::providers::identities_match(&d.identity, &device.identity) {
                                    if let Some(dpi) = new_dpi {
                                        d.values.dpi = Some(dpi);
                                    }
                                    if let Some(rate) = new_rate {
                                        d.values.polling_rate = Some(rate);
                                    }
                                }
                            }
                        }).await;
                        hardware_changed = true;
                    }
                }
            }
            if hardware_changed {
                sync_state.refresh_hardware(&sync_app, false).await;
            }
        }
    }));
    let signal_app = app.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            return;
        };
        let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
            return;
        };
        tokio::select! { _ = term.recv() => {}, _ = interrupt.recv() => {} }
        request_exit(signal_app);
    });
}

#[cfg(feature = "desktop")]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_window(app)))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot, commands::refresh_devices, commands::set_dpi,
            commands::set_polling_rate, commands::set_default_output, commands::set_volume,
            commands::set_mute, commands::set_surround, commands::import_hrtf,
            commands::save_audio_profile, commands::apply_audio_profile, commands::exit_app,
            commands::apply_mouse_remapping, commands::disable_mouse_remapping,
        ])
        .on_menu_event(menu_event)
        .setup(|app| {
            let state = Arc::new(AppState::new()?);
            app.manage(state.clone());
            let open = MenuItem::with_id(app, "open", "Apri", true, None::<&str>)?;
            let off = MenuItem::with_id(
                app,
                "surround-off",
                "Disattiva 7.1",
                cfg!(target_os = "linux"),
                None::<&str>,
            )?;
            let separator = PredefinedMenuItem::separator(app)?;
            let exit = MenuItem::with_id(app, "exit", "Esci", true, None::<&str>)?;
            let window_menu = Menu::with_items(app, &[&open, &off, &separator, &exit])?;
            app.set_menu(window_menu)?;

            let tray_open =
                MenuItem::with_id(app, "tray-open", "Mostra / Apri", true, None::<&str>)?;
            let tray_separator = PredefinedMenuItem::separator(app)?;
            let tray_exit = MenuItem::with_id(app, "tray-exit", "Esci", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&tray_open, &tray_separator, &tray_exit])?;
            let last_click = StdMutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
            let tray = TrayIconBuilder::with_id("peripheral-hub")
                .icon(tauri::include_image!("icons/32x32.png"))
                .tooltip("Peripheral Hub")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(move |tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let mut last = last_click.lock().unwrap_or_else(|error| error.into_inner());
                        if last.elapsed() >= std::time::Duration::from_millis(250) {
                            *last = std::time::Instant::now();
                            toggle_window(tray.app_handle());
                        }
                    }
                })
                .build(app);
            match tray {
                Ok(_) => state.tray_created.store(true, Ordering::SeqCst),
                Err(error) => eprintln!("Impossibile creare la tray: {error}"),
            }
            #[cfg(target_os = "linux")]
            start_workers(app.handle(), &state);
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(target_os = "linux")]
            {
                if window.label() != "main" {
                    return;
                }
                let tauri::WindowEvent::CloseRequested { api, .. } = event else {
                    return;
                };
                api.prevent_close();
                let window = window.clone();
                let app = window.app_handle().clone();
                let state = app.state::<Arc<AppState>>().inner().clone();
                if state.shutdown_started.load(Ordering::SeqCst) {
                    return;
                }
                tauri::async_runtime::spawn(async move {
                    let available = state.tray_created.load(Ordering::SeqCst)
                        && tray_host_available().await;
                    if state.shutdown_started.load(Ordering::SeqCst) {
                        return;
                    }
                    let hide_error = if available {
                        match window.hide() {
                            Ok(()) => {
                                state.publish(&app, |snapshot| {
                                    snapshot.diagnostics.retain(|item| item.id != "tray-unavailable");
                                }).await;
                                return;
                            }
                            Err(error) => Some(error.to_string()),
                        }
                    } else {
                        None
                    };
                    show_window(&app);
                    state.publish(&app, |snapshot| {
                        snapshot.diagnostics.retain(|item| item.id != "tray-unavailable");
                        let mut diagnostic = Diagnostic::new(
                            "tray-unavailable", ProviderKind::App,
                            if hide_error.is_some() { AvailabilityCode::Unknown } else { AvailabilityCode::Unsupported },
                            Severity::Warning,
                            if hide_error.is_some() {
                                "Impossibile nascondere la finestra: resta aperta. Usa Esci per ripristinare l'audio e chiudere."
                            } else {
                                "Tray non utilizzabile: la finestra resta aperta. Usa Esci per ripristinare l'audio e chiudere."
                            },
                        );
                        diagnostic.detail = hide_error;
                        snapshot.diagnostics.push(diagnostic);
                    }).await;
                });
            }
            #[cfg(not(target_os = "linux"))]
            let _ = (window, event);
        });
    let app = builder
        .build(tauri::generate_context!())
        .expect("Impossibile avviare Peripheral Hub");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            let state = app.state::<Arc<AppState>>();
            if !state.exit_ready.load(Ordering::SeqCst) {
                api.prevent_exit();
                request_exit(app.clone());
            }
        }
    });
}
