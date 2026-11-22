use super::bridge::*;
use super::store::*;
use super::types::*;
use crate::models::*;
use crate::profiles::AppPaths;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub struct Controller {
    pub store: Store,
    pub recovery_failure: Mutex<Option<AppError>>,
}

pub fn aggregate_errors(message: &str, failures: Vec<(String, AppError)>) -> AppResult<()> {
    let Some((_, first)) = failures.first() else {
        return Ok(());
    };
    let detail = format!(
        "{} errori:\n{}",
        failures.len(),
        failures
            .iter()
            .map(|(context, error)| format!(
                "{context}: {}{}",
                error.message,
                error
                    .detail
                    .as_ref()
                    .map(|detail| format!(" ({detail})"))
                    .unwrap_or_default()
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
    Err(AppError::new(first.code.clone(), message).detail(detail))
}

#[cfg(any(target_os = "linux", test))]
pub async fn stop_owned(bridge: &impl RemappingBackend, fingerprint: &str) -> AppResult<()> {
    let state = bridge.state(fingerprint).await?;
    if state.owner_uid == Some(current_uid()) {
        if let Some(preset) = state
            .preset
            .filter(|preset| valid_preset_name(preset, current_uid()))
        {
            bridge.stop(fingerprint, &preset).await?;
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
pub async fn release_identities(
    bridge: &impl RemappingBackend,
    identities: &[String],
) -> Vec<(String, AppError)> {
    let mut failures = Vec::new();
    for identity in identities {
        if let Err(error) = stop_owned(bridge, identity).await {
            failures.push((format!("Stop {identity}"), error));
        }
    }
    failures
}

#[cfg(any(target_os = "linux", test))]
pub async fn release_saved(store: &Store, bridge: &impl RemappingBackend) -> AppResult<()> {
    // Shutdown/relief must not depend on a writable config or its writer lock.
    let mut identities = store
        .recovery()
        .map(|data| data.identities)
        .unwrap_or_default();
    if let Ok(document) = store.load() {
        identities.extend(document.profiles.into_iter().map(|p| p.fingerprint));
    }
    let mut failures = Vec::new();
    match bridge.inventory().await {
        Ok(inventory) => identities.extend(inventory.devices.into_iter().map(|d| d.fingerprint)),
        Err(error) => failures.push(("Inventory per rilascio".to_owned(), error)),
    }
    identities.sort();
    identities.dedup();
    failures.extend(release_identities(bridge, &identities).await);
    aggregate_errors("Rilascio delle rimappature non confermato.", failures)
}

#[cfg(any(target_os = "linux", test))]
pub async fn recover_with_paths<B, C, CF, Q, QF>(
    paths: AppResult<AppPaths>,
    connect: C,
    quiesce: Q,
) -> AppResult<()>
where
    B: RemappingBackend,
    C: FnOnce() -> CF,
    CF: Future<Output = AppResult<B>>,
    Q: FnOnce() -> QF,
    QF: Future<Output = AppResult<()>>,
{
    match paths {
        Ok(paths) => {
            Controller::new(paths)
                .disable_with(None, connect, quiesce)
                .await
        }
        Err(error) => {
            // An unreadable locator is not permission to overwrite a fallback store.
            let mut failures = vec![("Percorso configurazione agente".to_owned(), error)];
            if let Err(error) = quiesce().await {
                failures.push(("Arresto agente utente".to_owned(), error));
            }
            match connect().await {
                Ok(bridge) => match bridge.inventory().await {
                    Ok(inventory) => {
                        let identities: Vec<_> = inventory
                            .devices
                            .into_iter()
                            .map(|d| d.fingerprint)
                            .collect();
                        failures.extend(release_identities(&bridge, &identities).await);
                    }
                    Err(error) => failures.push(("Inventory per recovery".to_owned(), error)),
                },
                Err(error) => failures.push(("Connessione per recovery".to_owned(), error)),
            }
            aggregate_errors("Configurazione non disponibile; nessuna disattivazione persistente salvata, rilascio tentato.", failures)
        }
    }
}

impl Controller {
    pub fn new(paths: AppPaths) -> Self {
        Self {
            store: Store::at(paths),
            recovery_failure: Mutex::new(None),
        }
    }

    pub fn recovery_failure(&self) -> Option<AppError> {
        self.recovery_failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn remember_recovery(&self, result: &AppResult<()>) {
        *self
            .recovery_failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = result.as_ref().err().cloned();
    }

    #[cfg(target_os = "linux")]
    pub async fn decorate(&self, discovery: &mut Discovery) {
        if let Err(error) = self.decorate_inner(discovery).await {
            discovery.diagnostics.push(diagnostic(&error));
            let saved = self.store.load().ok();
            for device in discovery
                .devices
                .iter_mut()
                .filter(|device| device.kind == DeviceKind::Mouse)
            {
                if device.provider == ProviderKind::Inputremapper {
                    device.availability = AvailabilityCode::Unknown;
                }
                let profile = saved.as_ref().and_then(|document| {
                    let matching: Vec<_> = document
                        .profiles
                        .iter()
                        .filter(|profile| {
                            crate::providers::identities_match(&device.identity, &profile.identity)
                        })
                        .collect();
                    if matching.len() == 1 {
                        Some(matching[0])
                    } else {
                        None
                    }
                });
                device.capabilities.remapping = Some(RemappingCapability {
                    provider: ProviderKind::Inputremapper,
                    availability: if error.code == ErrorCode::PermissionDenied {
                        AvailabilityCode::PermissionDenied
                    } else {
                        AvailabilityCode::ProviderMissing
                    },
                    writable: false,
                    fingerprint: profile.map(|profile| profile.fingerprint.clone()),
                    identity_strength: profile.map(|profile| profile.identity_strength),
                    buttons: Vec::new(),
                    key_codes: Vec::new(),
                    reason: Some(error.message.clone()),
                });
                device.values.remapping = Some(RemappingValues {
                    enabled: profile.is_some_and(|profile| profile.enabled),
                    auto_apply: profile.is_some_and(|profile| profile.auto_apply),
                    paused_for_safety: profile.is_some_and(|profile| profile.paused_for_safety),
                    profile_id: profile.map(|profile| profile.id.clone()),
                    bindings: profile
                        .map(|profile| profile.bindings.clone())
                        .unwrap_or_default(),
                    message: Some(error.message.clone()),
                    ..RemappingValues::default()
                });
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn decorate_inner(&self, discovery: &mut Discovery) -> AppResult<()> {
        let bridge = linux::Bridge::connect().await?;
        let inventory = bridge.inventory().await?;
        let document = self.store.load();
        let recovery = self.store.recovery();
        let storage_error = document.as_ref().err().or(recovery.as_ref().err()).cloned();
        let recovery_failure = self.recovery_failure();
        if let Some(error) = &storage_error {
            discovery.diagnostics.push(diagnostic(error));
        }
        let codes = inventory.output_codes();
        discovery
            .devices
            .retain(|device| device.provider != ProviderKind::Inputremapper);
        for input in &inventory.devices {
            let identity = input.identity();
            let indices: Vec<_> = discovery
                .devices
                .iter()
                .enumerate()
                .filter(|(_, device)| {
                    matches!(device.kind, DeviceKind::Mouse | DeviceKind::Unknown)
                        && crate::providers::identities_match(&device.identity, &identity)
                })
                .map(|(index, _)| index)
                .collect();
            if indices.len() > 1 {
                discovery.diagnostics.push(Diagnostic::new(
                    format!("remapping-ambiguous-{}", input.fingerprint),
                    ProviderKind::Inputremapper,
                    AvailabilityCode::Unknown,
                    Severity::Warning,
                    "Associazione mouse ambigua: nessuna rimappatura esposta.",
                ));
                continue;
            }
            let index = if indices.len() == 1 {
                indices[0]
            } else {
                discovery.devices.push(Device {
                    id: format!("input:{}", input.fingerprint), name: input.name.clone(),
                    brand: match input.vendor_id { 0x1532 => Brand::Razer, 0x046d => Brand::Logitech, _ => Brand::Generic },
                    kind: DeviceKind::Mouse, provider: ProviderKind::Inputremapper, identity: identity.clone(), availability: AvailabilityCode::Available,
                    support: SupportLevel::Partial, capabilities: DeviceCapabilities::default(), values: DeviceValues::default(), output_node_names: Vec::new(),
                    limitations: vec!["Rimappatura software dei soli pulsanti esposti; nessun DPI, polling o HID proprietario presunto.".to_owned()],
                });
                discovery.devices.len() - 1
            };
            let profile = document.as_ref().ok().and_then(|document| {
                document
                    .profiles
                    .iter()
                    .find(|profile| profile.fingerprint == input.fingerprint)
            });
            let state = bridge.state(&input.fingerprint).await?;
            let foreign = state.owner_uid.is_some_and(|uid| uid != current_uid());
            let device = &mut discovery.devices[index];
            device.kind = DeviceKind::Mouse;
            if device.support == SupportLevel::Unsupported {
                device.support = SupportLevel::Partial;
            }
            device.capabilities.remapping = Some(RemappingCapability {
                provider: ProviderKind::Inputremapper,
                availability: AvailabilityCode::Available,
                writable: !input.buttons.is_empty() && storage_error.is_none() && !foreign,
                fingerprint: Some(input.fingerprint.clone()),
                identity_strength: Some(input.identity_strength),
                buttons: input
                    .buttons
                    .iter()
                    .map(|button| MouseButtonDescriptor {
                        id: button.id.clone(),
                        label: button.label.clone(),
                        code: button.code,
                    })
                    .collect(),
                key_codes: codes.clone(),
                reason: storage_error
                    .as_ref()
                    .map(|error| error.message.clone())
                    .or_else(|| foreign.then(|| "Mouse gestito da un altro utente.".to_owned()))
                    .or_else(|| recovery_failure.as_ref().map(|error| error.message.clone())),
            });
            device.values.remapping = Some(
                state.values(
                    profile,
                    recovery
                        .as_ref()
                        .ok()
                        .is_some_and(|data| data.disarmed.contains(&input.fingerprint)),
                    recovery_failure.as_ref(),
                ),
            );
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub async fn apply(
        &self,
        device: &Device,
        bindings: Vec<MouseBinding>,
        auto_apply: bool,
        allow_weak_identity: bool,
    ) -> AppResult<()> {
        let capability = device
            .capabilities
            .remapping
            .as_ref()
            .filter(|capability| capability.writable)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Unsupported,
                    "Rimappatura non disponibile per questo mouse.",
                )
            })?;
        let fingerprint = capability
            .fingerprint
            .as_deref()
            .ok_or_else(|| invalid("Identita input mancante."))?;
        let bridge = linux::Bridge::connect().await?;
        let inventory = bridge.inventory().await?;
        let input = inventory
            .devices
            .iter()
            .find(|input| input.fingerprint == fingerprint)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::DeviceNotFound,
                    "Mouse scollegato o identita cambiata.",
                )
            })?;
        validate_bindings(&bindings, input, &inventory.output_codes())?;
        if auto_apply
            && input.identity_strength == RemappingIdentityStrength::Port
            && !allow_weak_identity
        {
            return Err(invalid(
                "Confermare esplicitamente l'automatismo per modello/porta senza seriale univoco.",
            ));
        }
        if bindings
            .iter()
            .all(|binding| binding.action == MouseAction::Default)
        {
            return self.disable(device).await;
        }
        let _lock = self.store.acquire_lock().await?;
        let mut document = self.store.load()?;
        let existing = document
            .profiles
            .iter()
            .position(|profile| profile.fingerprint == fingerprint);
        let mut profile = existing
            .map(|index| document.profiles[index].clone())
            .unwrap_or(RemappingProfile {
                id: Uuid::new_v4().to_string(),
                fingerprint: fingerprint.to_owned(),
                identity_strength: input.identity_strength,
                identity: input.identity(),
                bindings: Vec::new(),
                enabled: false,
                auto_apply: false,
                allow_weak_identity: false,
                paused_for_safety: false,
                compiled_preset: None,
            });
        profile.bindings = bindings;
        profile.identity_strength = input.identity_strength;
        profile.identity = input.identity();
        profile.enabled = true;
        profile.auto_apply = auto_apply;
        profile.allow_weak_identity = allow_weak_identity;
        profile.paused_for_safety = false;
        let state = bridge.state(fingerprint).await?;
        if state.preset.as_ref().is_some_and(|preset| {
            state.owner_uid != Some(current_uid()) || !valid_preset_name(preset, current_uid())
        }) {
            return Err(AppError::new(
                ErrorCode::Busy,
                "Il mouse ha una configurazione di un altro proprietario.",
            ));
        }
        let name = self.store.preset(&profile, input)?;
        // Persist recovery intent before an RPC that may complete after a timeout.
        save_agent_location(&self.store.paths)?;
        let agent_arguments: &[&str] = if auto_apply {
            &["--user", "enable", "--now", AGENT_UNIT]
        } else {
            &["--user", "start", AGENT_UNIT]
        };
        crate::process::run("systemctl", agent_arguments, Duration::from_secs(5))
            .await
            .map_err(|error| {
                AppError::new(
                    error.code,
                    "Agente rimappatura non pronto; nessuna modifica applicata.",
                )
                .detail(error.message)
            })?;
        self.store.disarm(&[fingerprint.to_owned()], true)?;
        profile.compiled_preset = Some(name.clone());
        profile.paused_for_safety = true;
        let index = match existing {
            Some(index) => {
                document.profiles[index] = profile.clone();
                index
            }
            None => {
                document.profiles.push(profile.clone());
                document.profiles.len() - 1
            }
        };
        self.store.save(&mut document)?;
        let result = bridge
            .apply(
                input,
                inventory.generation,
                &name,
                state.preset.as_deref().unwrap_or(""),
                &self.store.directory(),
            )
            .await;
        if let Err(error) = result {
            let mut failures = vec![("Apply".to_owned(), error)];
            if let Err(error) = bridge.stop_owned(fingerprint).await {
                failures.push(("Stop dopo Apply".to_owned(), error));
            }
            return aggregate_errors("Applicazione non confermata; rilascio tentato.", failures);
        }
        profile.paused_for_safety = false;
        document.profiles[index] = profile;
        self.finish_apply_with(
            &_lock,
            &mut document,
            fingerprint,
            &bridge,
            linux::quiesce_agent,
        )
        .await
    }

    #[cfg(any(target_os = "linux", test))]
    pub async fn finish_apply_with<Q, QF>(
        &self,
        _lock: &StoreLock,
        document: &mut Document,
        fingerprint: &str,
        bridge: &impl RemappingBackend,
        quiesce: Q,
    ) -> AppResult<()>
    where
        Q: FnOnce() -> QF,
        QF: Future<Output = AppResult<()>>,
    {
        let persisted = self
            .store
            .save(document)
            .and_then(|_| self.store.disarm(&[fingerprint.to_owned()], false));
        let result = match persisted {
            Ok(()) => Ok(()),
            Err(error) => {
                let mut failures = vec![("Salvataggio dopo Apply".to_owned(), error)];
                // Even a failed directory fsync may have already renamed a rearm
                // record into place. Fence the agent before emergency release.
                if let Err(error) = quiesce().await {
                    failures.push(("Arresto agente utente".to_owned(), error));
                }
                if let Err(error) = release_saved(&self.store, bridge).await {
                    failures.push(("Rilascio dopo Apply".to_owned(), error));
                }
                aggregate_errors(
                    "Applicazione non salvata interamente; quiescenza agente e rilascio tentati.",
                    failures,
                )
            }
        };
        self.remember_recovery(&result);
        result
    }

    #[cfg(target_os = "linux")]
    pub async fn disable(&self, device: &Device) -> AppResult<()> {
        let fingerprint = device
            .capabilities
            .remapping
            .as_ref()
            .and_then(|capability| capability.fingerprint.as_deref())
            .ok_or_else(|| invalid("Identita input non disponibile."))?;
        self.disable_identity(fingerprint).await
    }

    #[cfg(target_os = "linux")]
    async fn disable_identity(&self, fingerprint: &str) -> AppResult<()> {
        self.disable_with(
            Some(fingerprint),
            linux::Bridge::connect,
            linux::quiesce_agent,
        )
        .await
    }

    #[cfg(target_os = "linux")]
    pub async fn disable_all(&self) -> AppResult<()> {
        self.disable_with(None, linux::Bridge::connect, linux::quiesce_agent)
            .await
    }

    #[cfg(any(target_os = "linux", test))]
    pub async fn disable_with<B, C, CF, Q, QF>(
        &self,
        only: Option<&str>,
        connect: C,
        quiesce: Q,
    ) -> AppResult<()>
    where
        B: RemappingBackend,
        C: FnOnce() -> CF,
        CF: Future<Output = AppResult<B>>,
        Q: FnOnce() -> QF,
        QF: Future<Output = AppResult<()>>,
    {
        let mut failures = Vec::new();
        // Retain a successful lock through Stop; never save an unlocked snapshot.
        let lock = match self.store.acquire_lock().await {
            Ok(lock) => Some(lock),
            Err(error) => {
                failures.push(("Lock configurazione".to_owned(), error));
                None
            }
        };
        let mut identities = Vec::new();
        let recovery = self.store.recovery();
        match &recovery {
            Ok(data) => identities.extend(data.identities.iter().cloned()),
            Err(error) => failures.push(("Lettura recovery".to_owned(), error.clone())),
        }
        let mut document = self.store.load();
        match &document {
            Ok(data) => identities.extend(data.profiles.iter().map(|p| p.fingerprint.clone())),
            Err(error) => failures.push(("Lettura profili".to_owned(), error.clone())),
        }
        if let Some(identity) = only {
            identities = vec![identity.to_owned()];
        }
        identities.sort();
        identities.dedup();
        if lock.is_some() {
            // Disarm and desired flags are independent best-effort writes.
            if recovery.is_ok() {
                if let Err(error) = self.store.disarm(&identities, true) {
                    failures.push(("Salvataggio recovery".to_owned(), error));
                }
            }
            if let Ok(document) = &mut document {
                for profile in &mut document.profiles {
                    if only.is_none_or(|identity| profile.fingerprint == identity) {
                        profile.enabled = false;
                        profile.auto_apply = false;
                        profile.paused_for_safety = false;
                    }
                }
                if let Err(error) = self.store.save(document) {
                    failures.push(("Salvataggio profili".to_owned(), error));
                }
            }
        }
        let mut persistence_failed = !failures.is_empty();
        let mut quiesce = Some(quiesce);
        if persistence_failed {
            // Fence an in-flight auto-apply before querying its final owned preset.
            if let Err(error) = quiesce.take().unwrap()().await {
                failures.push(("Arresto agente utente".to_owned(), error));
            }
        }
        match connect().await {
            Ok(bridge) => {
                if only.is_none() {
                    match bridge.inventory().await {
                        Ok(inventory) => {
                            let additional: Vec<_> = inventory
                                .devices
                                .into_iter()
                                .map(|device| device.fingerprint)
                                .filter(|identity| !identities.contains(identity))
                                .collect();
                            if lock.is_some() && !additional.is_empty() {
                                if let Err(error) = self.store.disarm(&additional, true) {
                                    failures
                                        .push(("Salvataggio recovery inventory".to_owned(), error));
                                    persistence_failed = true;
                                }
                            }
                            identities.extend(additional);
                        }
                        Err(error) => failures.push(("Inventory per recovery".to_owned(), error)),
                    }
                }
                if persistence_failed {
                    if let Some(quiesce) = quiesce.take() {
                        if let Err(error) = quiesce().await {
                            failures.push(("Arresto agente utente".to_owned(), error));
                        }
                    }
                }
                identities.sort();
                identities.dedup();
                failures.extend(release_identities(&bridge, &identities).await);
            }
            Err(error) => failures.push(("Connessione per recovery".to_owned(), error)),
        }
        let result = aggregate_errors(
            if persistence_failed {
                "Disattivazione non salvata interamente; rilascio tentato. I profili possono restare abilitati al riavvio."
            } else {
                "Disattivazione salvata, ma rilascio delle rimappature non confermato."
            },
            failures,
        );
        self.remember_recovery(&result);
        result
    }

    #[cfg(target_os = "linux")]
    pub async fn release_owned(&self) -> AppResult<()> {
        self.release_with(linux::Bridge::connect).await
    }

    #[cfg(any(target_os = "linux", test))]
    pub async fn release_with<B, C, CF>(&self, connect: C) -> AppResult<()>
    where
        B: RemappingBackend,
        C: FnOnce() -> CF,
        CF: Future<Output = AppResult<B>>,
    {
        let bridge = connect().await?;
        release_saved(&self.store, &bridge).await
    }
}
