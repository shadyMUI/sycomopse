use crate::models::{AppError, AppResult, ErrorCode, RemappingIdentityStrength};
use crate::profiles::{self, AppPaths};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use super::bridge::linux;
use super::controller::{
    aggregate_errors, recover_with_paths, release_identities, stop_owned, Controller,
};
use super::store::{
    agent_paths, unchanged_observation, valid_preset_name, validate_bindings, FileLockExt, Store,
};
use super::types::{current_uid, RemappingBackend};

#[cfg(any(target_os = "linux", test))]
#[derive(Default)]
pub struct AgentRuntime {
    pub seen: HashMap<String, (String, Option<String>, u64)>,
    pub present: HashSet<String>,
    pub unplug: HashMap<String, VecDeque<Instant>>,
    pub failures: HashMap<String, u8>,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug)]
pub struct AgentFailure {
    pub error: AppError,
    pub persistence_failed: bool,
}

#[cfg(any(target_os = "linux", test))]
impl AgentFailure {
    pub fn storage(error: AppError) -> Self {
        Self {
            error,
            persistence_failed: true,
        }
    }

    pub fn runtime(error: AppError) -> Self {
        Self {
            error,
            persistence_failed: false,
        }
    }
}

pub fn log_error(context: &str, error: &AppError) {
    eprintln!("{context}: {}", error.message);
    if let Some(detail) = &error.detail {
        eprintln!("{detail}");
    }
}

#[cfg(any(target_os = "linux", test))]
pub async fn until_shutdown<T>(
    work: impl Future<Output = T>,
    shutdown: impl Future<Output = ()>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = shutdown => None,
        result = work => Some(result),
    }
}

#[cfg(any(target_os = "linux", test))]
impl AgentRuntime {
    pub async fn cycle<B, C, CF, S, SF>(
        &mut self,
        controller: &Controller,
        connect: C,
        active_session: S,
        shutdown: impl Future<Output = ()>,
    ) -> bool
    where
        B: RemappingBackend,
        C: Fn() -> CF,
        CF: Future<Output = AppResult<B>>,
        S: FnOnce() -> SF,
        SF: Future<Output = AppResult<bool>>,
    {
        let Some(result) = until_shutdown(
            self.step(&controller.store, &connect, active_session),
            shutdown,
        )
        .await
        else {
            return false;
        };
        if let Err(failure) = result {
            if failure.persistence_failed {
                log_error(
                    "Agente rimappatura; salvataggio non confermato, autoapply arrestato",
                    &failure.error,
                );
                // Clean exit prevents Restart=on-failure from rearming unchanged data
                // when a full/read-only filesystem later becomes writable again.
                return false;
            }
            log_error("Agente rimappatura", &failure.error);
            if let Err(error) = controller.release_with(connect).await {
                log_error("Rilascio agente", &error);
            }
        }
        true
    }

    pub async fn step<B, C, CF, S, SF>(
        &mut self,
        store: &Store,
        connect: C,
        active_session: S,
    ) -> Result<(), AgentFailure>
    where
        B: RemappingBackend,
        C: FnOnce() -> CF,
        CF: Future<Output = AppResult<B>>,
        S: FnOnce() -> SF,
        SF: Future<Output = AppResult<bool>>,
    {
        let _lock = match store.acquire_lock().await {
            Ok(lock) => lock,
            // A foreground apply may hold the lock longer than one tick.
            Err(error) if error.code == ErrorCode::Busy => return Ok(()),
            Err(error) => return Err(AgentFailure::storage(error)),
        };
        let mut document = store.load().map_err(AgentFailure::storage)?;
        let recovery = store.recovery().map_err(AgentFailure::storage)?;
        if document.profiles.is_empty() && recovery.identities.is_empty() {
            return Ok(());
        }
        let bridge = connect().await.map_err(AgentFailure::runtime)?;
        aggregate_errors(
            "Rilascio recovery non confermato.",
            release_identities(&bridge, &recovery.disarmed).await,
        )
        .map_err(AgentFailure::runtime)?;
        if document.profiles.is_empty() {
            return aggregate_errors(
                "Rilascio recovery non confermato.",
                release_identities(&bridge, &recovery.identities).await,
            )
            .map_err(AgentFailure::runtime);
        }
        if !active_session().await.unwrap_or(false) {
            self.seen.clear();
            return aggregate_errors(
                "Rilascio fuori sessione non confermato.",
                release_identities(
                    &bridge,
                    &document
                        .profiles
                        .iter()
                        .map(|p| p.fingerprint.clone())
                        .collect::<Vec<_>>(),
                )
                .await,
            )
            .map_err(AgentFailure::runtime);
        }
        let inventory = bridge.inventory().await.map_err(AgentFailure::runtime)?;
        let now = Instant::now();
        let current: HashSet<_> = inventory
            .devices
            .iter()
            .map(|d| d.fingerprint.clone())
            .collect();
        let mut changed = false;
        for identity in self.present.difference(&current) {
            let events = self.unplug.entry(identity.clone()).or_default();
            events.push_back(now);
            while events
                .front()
                .is_some_and(|time| now.duration_since(*time) > Duration::from_secs(15))
            {
                events.pop_front();
            }
            if events.len() >= 2 {
                if let Some(profile) = document
                    .profiles
                    .iter_mut()
                    .find(|p| p.fingerprint == *identity)
                {
                    profile.paused_for_safety = true;
                    changed = true;
                }
            }
            self.seen.remove(identity);
        }
        self.present = current;
        for index in 0..document.profiles.len() {
            let mut profile = document.profiles[index].clone();
            if !profile.enabled || recovery.disarmed.contains(&profile.fingerprint) {
                stop_owned(&bridge, &profile.fingerprint)
                    .await
                    .map_err(AgentFailure::runtime)?;
                continue;
            }
            if !profile.auto_apply || profile.paused_for_safety {
                continue;
            }
            let Some(input) = inventory
                .devices
                .iter()
                .find(|input| input.fingerprint == profile.fingerprint)
            else {
                continue;
            };
            let state = bridge
                .state(&profile.fingerprint)
                .await
                .map_err(AgentFailure::runtime)?;
            if state.generation != inventory.generation {
                // A reconnect can happen between the two DBus replies.
                self.seen.remove(&profile.fingerprint);
                continue;
            }
            if input.identity_strength != profile.identity_strength
                || input.identity_strength == RemappingIdentityStrength::Port
                    && !profile.allow_weak_identity
                || validate_bindings(&profile.bindings, input, &inventory.output_codes()).is_err()
            {
                if state.owner_uid == Some(current_uid()) && state.preset == profile.compiled_preset
                {
                    if let Some(preset) = &state.preset {
                        bridge
                            .stop(&profile.fingerprint, preset)
                            .await
                            .map_err(AgentFailure::runtime)?;
                    }
                }
                profile.paused_for_safety = true;
                document.profiles[index] = profile;
                changed = true;
                continue;
            }
            if state.owner_uid.is_some_and(|uid| uid != current_uid())
                || state
                    .preset
                    .as_deref()
                    .is_some_and(|preset| !valid_preset_name(preset, current_uid()))
            {
                continue;
            }
            if state.state == "RUNNING" && state.preset == profile.compiled_preset {
                self.seen.insert(
                    profile.fingerprint.clone(),
                    (
                        bridge.owner().to_owned(),
                        state.preset.clone(),
                        inventory.generation,
                    ),
                );
                self.failures.remove(&profile.fingerprint);
                continue;
            }
            if self.seen.get(&profile.fingerprint).is_some_and(|previous| {
                unchanged_observation(
                    previous,
                    bridge.owner(),
                    &profile.compiled_preset,
                    inventory.generation,
                )
            }) && state.state == "STOPPED"
            {
                profile.paused_for_safety = true;
                document.profiles[index] = profile;
                changed = true;
                continue;
            }
            let name = store
                .preset(&profile, input)
                .map_err(AgentFailure::storage)?;
            store
                .disarm(&[profile.fingerprint.clone()], true)
                .map_err(AgentFailure::storage)?;
            profile.compiled_preset = Some(name.clone());
            profile.paused_for_safety = true;
            document.profiles[index] = profile.clone();
            store.save(&mut document).map_err(AgentFailure::storage)?;
            changed = false;
            match bridge
                .apply(
                    input,
                    inventory.generation,
                    &name,
                    state.preset.as_deref().unwrap_or(""),
                    &store.directory(),
                )
                .await
            {
                Ok(()) => {
                    profile.paused_for_safety = false;
                    document.profiles[index] = profile.clone();
                    store.save(&mut document).map_err(AgentFailure::storage)?;
                    store
                        .disarm(&[profile.fingerprint.clone()], false)
                        .map_err(AgentFailure::storage)?;
                    self.seen.insert(
                        profile.fingerprint.clone(),
                        (bridge.owner().to_owned(), Some(name), inventory.generation),
                    );
                    self.failures.remove(&profile.fingerprint);
                }
                Err(error) => {
                    let release = stop_owned(&bridge, &profile.fingerprint).await;
                    let count = self
                        .failures
                        .entry(profile.fingerprint.clone())
                        .or_default();
                    *count = count.saturating_add(1);
                    if *count >= 3 {
                        profile.paused_for_safety = true;
                        document.profiles[index] = profile;
                        changed = true;
                    }
                    log_error("Riattivazione rimappatura", &error);
                    if let Err(stop_error) = release {
                        return aggregate_errors(
                            "Riattivazione fallita e rilascio non confermato.",
                            vec![("Apply".to_owned(), error), ("Stop".to_owned(), stop_error)],
                        )
                        .map_err(AgentFailure::runtime);
                    }
                }
            }
        }
        if changed {
            store.save(&mut document).map_err(AgentFailure::storage)?;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub async fn run_agent(disable_all: bool) -> AppResult<()> {
    if current_uid() == 0 {
        return Err(AppError::new(
            ErrorCode::PermissionDenied,
            "L'agente deve essere eseguito dall'utente, non da root.",
        ));
    }
    let paths = agent_paths();
    // Recovery must not require creating the runtime directory/instance lock.
    if disable_all {
        return recover_with_paths(paths, linux::Bridge::connect, linux::quiesce_agent).await;
    }
    let mut controller = Controller::new(paths?);
    let installed_binary = std::env::current_exe().ok().is_some_and(|path| {
        path.parent().is_some_and(|p| {
            p == Path::new("/usr/bin") || p == Path::new("/usr/local/bin") || p == Path::new("/bin")
        })
    });
    profiles::private_dir(&controller.store.paths.runtime_dir)?;
    let mut options = OpenOptions::new();
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    options
        .write(true)
        .read(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let instance = options.open(
        controller
            .store
            .paths
            .runtime_dir
            .join("remapping-agent.lock"),
    )?;
    let metadata = instance.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(AppError::new(
            ErrorCode::PermissionDenied,
            "Lock agente non privato.",
        ));
    }
    instance
        .try_lock_exclusive()
        .map_err(|_| AppError::new(ErrorCode::Busy, "Agente gia attivo."))?;
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut runtime = AgentRuntime::default();
    loop {
        tokio::select! {
            biased;
            _ = term.recv() => break,
            _ = interrupt.recv() => break,
            _ = tick.tick() => {}
        }
        // Native package removal must not leave a resident, endlessly retrying agent.
        let agent_present = std::env::var_os("PERIPHERAL_HUB_REMAPPING_AGENT_PATH")
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(|| std::env::current_exe().ok().filter(|p| p.is_file()))
            .or_else(|| {
                for dir in &["/usr/bin", "/usr/local/bin", "/bin"] {
                    let p = Path::new(dir).join("peripheral-hub-remapping-agent");
                    if p.is_file() {
                        return Some(p);
                    }
                }
                None
            })
            .is_some();
        if installed_binary && !agent_present {
            break;
        }
        match agent_paths() {
            Ok(paths) if paths.config_dir != controller.store.paths.config_dir => {
                if let Err(error) = controller.release_owned().await {
                    log_error("Cambio configurazione agente", &error);
                    continue;
                }
                controller = Controller::new(paths);
                runtime = AgentRuntime::default();
            }
            Err(error) => {
                log_error("Configurazione agente; autoapply arrestato", &error);
                break;
            }
            _ => {}
        }
        if !runtime
            .cycle(
                &controller,
                linux::Bridge::connect,
                linux::active_session,
                async {
                    tokio::select! {
                        _ = term.recv() => {},
                        _ = interrupt.recv() => {},
                    }
                },
            )
            .await
        {
            break;
        }
    }
    // Logout/stop releases only this user's verified presets; desired flags survive.
    if let Err(error) = controller.release_owned().await {
        log_error("Arresto agente; rilascio non confermato", &error);
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub async fn run_agent(_disable_all: bool) -> AppResult<()> {
    Err(AppError::unsupported_platform())
}
