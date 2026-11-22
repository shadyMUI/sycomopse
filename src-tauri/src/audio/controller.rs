use crate::models::{
    AppError, AppResult, AudioNode, AudioProfile, AudioSnapshot, ErrorCode, SurroundState,
};
use crate::profiles::{validate_node_name, validate_profile, AppPaths, APP_NODE_PREFIX, SYSTEM_HRTF};
#[cfg(any(target_os = "linux", test))]
use crate::surround;
use crate::surround::HrtfSelection;
use serde_json::Value;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use crate::process;
#[cfg(target_os = "linux")]
use crate::surround::{
    runtime::{ActiveFilter, OwnedChild},
    RestoreAction, RestoreIntent, RestoreRecord,
};
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(any(feature = "desktop", test))]
pub const MONITOR_CAPACITY: usize = 4;
#[cfg(target_os = "linux")]
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub enum AudioEvent {
    Snapshot(Vec<Value>),
    Changed(Vec<Value>),
    Disconnected,
}

use super::registry::{is_type, text_property, Registry};
use super::graph::{same_channels, stereo_target, CHANNELS_71};
use super::parser::{invalid_dump, JsonArrayParser};
#[cfg(any(target_os = "linux", test))]
use super::graph::verify_graph;
#[cfg(all(test, target_os = "linux"))]
use super::lifecycle_tests;

pub struct AudioController {    pub paths: AppPaths,
    pub registry: Registry,
    pub available: bool,
    pub selected_output: Option<String>,
    pub(crate) hrtf: Option<HrtfSelection>,
    pub state: SurroundState,
    pub reason: Option<String>,
    pub recovery_pending: bool,
    #[cfg(target_os = "linux")]
    pub(crate) filter: Option<ActiveFilter>,
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) test_backend: Option<lifecycle_tests::Backend>,
}

impl AudioController {
    pub fn control_cookie(&self) -> AppResult<String> {
        self.registry
            .cookie()
            .map(|cookie| cookie.to_string())
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::NotConfirmed,
                    "Identita del server PipeWire non disponibile.",
                )
            })
    }

    pub fn new(paths: AppPaths) -> Self {
        Self {
            paths,
            registry: Registry::default(),
            available: false,
            selected_output: None,
            hrtf: None,
            state: SurroundState::Off,
            reason: if cfg!(target_os = "linux") {
                None
            } else {
                Some("Il backend audio richiede Linux.".into())
            },
            recovery_pending: true,
            #[cfg(target_os = "linux")]
            filter: None,
            #[cfg(all(test, target_os = "linux"))]
            test_backend: None,
        }
    }

    pub fn snapshot(&self) -> AudioSnapshot {
        let (outputs, inputs, effective, configured, default_input) = if self.available {
            let defaults = self.registry.defaults();
            (
                self.registry.nodes(true),
                self.registry.nodes(false),
                defaults.0,
                defaults.1,
                defaults.2,
            )
        } else {
            (Vec::new(), Vec::new(), None, None, None)
        };
        let target = self
            .selected_output
            .as_deref()
            .and_then(|name| outputs.iter().find(|node| node.node_name == name));
        let hrtf = self.hrtf.as_ref().map(|selection| {
            let mut status = selection.status.clone();
            status.readable = selection.readable();
            status
        });
        let target_error = target.map_or_else(
            || Some("Selezionare un'uscita fisica disponibile.".into()),
            |target| stereo_target(target).err().map(|error| error.message),
        );
        let reason = if !self.available {
            Some(if cfg!(target_os = "linux") {
                "PipeWire non disponibile.".into()
            } else {
                "Il backend audio richiede Linux.".into()
            })
        } else if self.reason.is_some() {
            self.reason.clone()
        } else if target_error.is_some() {
            target_error
        } else if !hrtf.as_ref().is_some_and(|hrtf| hrtf.readable) {
            Some("Importare un HRTF SOFA valido o installare libmysofa.".into())
        } else {
            None
        };
        let can_enable_surround = self.available
            && target.is_some_and(|target| stereo_target(target).is_ok())
            && hrtf.as_ref().is_some_and(|hrtf| hrtf.readable)
            && !self.recovery_pending
            && !matches!(
                self.state,
                SurroundState::Starting | SurroundState::Stopping
            );
        AudioSnapshot {
            available: self.available,
            outputs,
            inputs,
            default_output: effective,
            configured_default_output: configured,
            selected_output: self.selected_output.clone(),
            default_input,
            surround_state: self.state,
            can_enable_surround,
            surround_reason: reason,
            hrtf,
        }
    }

    pub async fn refresh(&mut self) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.read_registry().await;
            if result.is_ok() {
                if self.hrtf.is_none() {
                    match self.resolve_hrtf(SYSTEM_HRTF).await {
                        Ok(selection) => {
                            self.hrtf = Some(selection);
                            self.reason = None;
                        }
                        Err(error) => self.reason = Some(error.message),
                    }
                }
                if self.selected_output.is_none() {
                    let defaults = self.registry.defaults();
                    self.selected_output = defaults.0.into_iter().chain(defaults.1).find(|name| {
                        self.registry.node(name).is_ok_and(|node| {
                            !node.is_virtual && node.media_class.starts_with("Audio/Sink")
                        })
                    });
                }
                self.recover_pending().await?;
                self.enforce_safety().await?;
            } else if self.filter.is_some() {
                let _ = self.stop_filter(RestoreIntent::TargetLost).await;
                self.state = SurroundState::Error;
                self.reason = Some("PipeWire disconnesso: surround fermato, ripristino in attesa della riconnessione.".into());
            }
            result
        }
    }

    pub async fn set_default_output(&mut self, node_name: &str) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = node_name;
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.select_output(node_name).await;
            self.complete(result).await
        }
    }

    pub async fn set_volume(&mut self, node_name: &str, volume: f64) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (node_name, volume);
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.write_volume(node_name, volume).await;
            self.complete(result).await
        }
    }

    pub async fn set_mute(&mut self, node_name: &str, muted: bool) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (node_name, muted);
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.write_mute(node_name, muted).await;
            self.complete(result).await
        }
    }

    pub async fn set_surround(&mut self, enabled: bool) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = enabled;
            self.available = false;
            self.state = SurroundState::Error;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = if enabled {
                self.enable_surround(None).await
            } else {
                self.stop_filter(RestoreIntent::Bypass).await
            };
            if let Err(error) = &result {
                if self.filter.is_none() {
                    self.state = SurroundState::Error;
                }
                self.reason = Some(error.message.clone());
            }
            self.complete(result).await
        }
    }

    pub async fn import_hrtf(&mut self, path: PathBuf) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = match surround::import_hrtf(&self.paths, &path).await {
                Ok(selection) => {
                    // A new selection must not be shown as the HRTF of an old running filter.
                    let stopped = if self.filter.is_some() {
                        self.stop_filter(RestoreIntent::Bypass).await
                    } else {
                        Ok(())
                    };
                    if stopped.is_ok() {
                        self.hrtf = Some(selection);
                        self.reason = None;
                    }
                    stopped
                }
                Err(error) => Err(error),
            };
            self.complete(result).await
        }
    }

    pub async fn apply_profile(&mut self, profile: &AudioProfile) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = profile;
            self.available = false;
            Err(AppError::unsupported_platform())
        }
        #[cfg(target_os = "linux")]
        {
            let result = self.apply_validated_profile(profile).await;
            if let Err(error) = &result {
                self.reason = Some(error.message.clone());
            } else if !profile.surround_requested {
                self.state = SurroundState::Off;
                self.reason = None;
            }
            self.complete(result).await
        }
    }

    pub async fn shutdown(&mut self) -> AppResult<()> {
        #[cfg(not(target_os = "linux"))]
        {
            self.state = SurroundState::Off;
            Ok(())
        }
        #[cfg(target_os = "linux")]
        {
            self.state = SurroundState::Stopping;
            let result = self.stop_filter(RestoreIntent::Bypass).await;
            if result.is_err() {
                self.state = SurroundState::Error;
            }
            result
        }
    }

    pub async fn handle_event(&mut self, event: AudioEvent) -> AppResult<()> {
        match event {
            AudioEvent::Disconnected => {
                self.available = false;
                #[cfg(target_os = "linux")]
                if self.filter.is_some() {
                    let _ = self.stop_filter(RestoreIntent::TargetLost).await;
                    self.state = SurroundState::Error;
                    self.reason = Some("PipeWire disconnesso: filtro fermato senza cambiare un default non osservabile.".into());
                }
            }
            AudioEvent::Snapshot(_objects) => {
                #[cfg(not(target_os = "linux"))]
                self.registry.replace(_objects)?;
                #[cfg(target_os = "linux")]
                {
                    // Queued snapshots may predate a newer read; never tombstone
                    // that newer instance from the stale monitor payload.
                    self.read_registry().await?;
                    self.recover_pending().await?;
                    self.enforce_safety().await?;
                }
            }
            AudioEvent::Changed(objects) => {
                let resync = self.registry.merge(objects)?;
                #[cfg(not(target_os = "linux"))]
                let _ = resync;
                #[cfg(target_os = "linux")]
                {
                    if resync {
                        self.read_registry().await?;
                    }
                    self.enforce_safety().await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl AudioController {
    pub(crate) async fn resolve_hrtf(&self, reference: &str) -> AppResult<HrtfSelection> {
        #[cfg(test)]
        if let Some(backend) = &self.test_backend {
            return backend.resolve_hrtf(reference);
        }
        surround::resolve_hrtf(&self.paths, reference).await
    }

    pub async fn mutate_level(
        &mut self,
        node: &AudioNode,
        volume: Option<f64>,
        muted: Option<bool>,
    ) -> AppResult<()> {
        #[cfg(test)]
        if let Some(backend) = &mut self.test_backend {
            return backend.write_level(node, volume, muted);
        }
        let cookie = self.control_cookie()?;
        match (volume, muted) {
            (Some(volume), Some(muted)) => {
                crate::audio_worker::set_level(node, &cookie, volume, muted).await
            }
            (Some(volume), None) => crate::audio_worker::set_volume(node, &cookie, volume).await,
            (None, Some(muted)) => crate::audio_worker::set_mute(node, &cookie, muted).await,
            (None, None) => Err(AppError::new(
                ErrorCode::Validation,
                "Nessuna modifica del livello richiesta.",
            )),
        }
    }

    pub async fn mutate_default(
        &mut self,
        node: Option<&AudioNode>,
        expected: Option<&str>,
    ) -> AppResult<()> {
        #[cfg(test)]
        if let Some(backend) = &mut self.test_backend {
            return backend.write_default(node, expected);
        }
        crate::audio_worker::set_default(node, &self.control_cookie()?, expected).await
    }

    pub(crate) async fn spawn_filter(
        &mut self,
        target: &AudioNode,
        hrtf: &HrtfSelection,
        configured: Option<String>,
        effective: Option<String>,
    ) -> AppResult<ActiveFilter> {
        #[cfg(test)]
        if let Some(backend) = &mut self.test_backend {
            return backend.spawn(&self.paths, target, hrtf, configured, effective);
        }
        ActiveFilter::spawn(
            &self.paths,
            target,
            hrtf,
            configured,
            effective,
            self.registry.cookie(),
        )
        .await
    }

    pub async fn terminate_current_filter(&mut self) -> AppResult<()> {
        #[cfg(test)]
        if let Some(backend) = &mut self.test_backend {
            backend.before_stop().await?;
        }
        let Some(filter) = self.filter.as_mut() else {
            return Ok(());
        };
        #[cfg(test)]
        let record = filter.record.clone();
        let result = filter.stop().await;
        #[cfg(test)]
        if result.is_ok() {
            if let Some(backend) = &mut self.test_backend {
                backend.stopped(&record);
            }
        }
        result
    }

    pub async fn read_registry(&mut self) -> AppResult<()> {
        #[cfg(test)]
        if let Some(backend) = &self.test_backend {
            let result = backend.read_registry().and_then(|registry| {
                self.registry
                    .replace(registry.objects.into_values().collect())
            });
            self.available = result.is_ok();
            return result;
        }
        let result = async {
            let output = process::run("pw-dump", &["-N"], COMMAND_TIMEOUT).await?;
            let mut parser = JsonArrayParser::default();
            let mut arrays = parser.feed(&output.stdout)?;
            parser.finish()?;
            if arrays.len() != 1 {
                return Err(invalid_dump("Snapshot completa non ricevuta."));
            }
            self.registry.replace(arrays.remove(0))?;
            Ok(())
        }
        .await;
        self.available = result.is_ok();
        result
    }

    pub async fn complete(&mut self, result: AppResult<()>) -> AppResult<()> {
        let observation = self.read_registry().await;
        let safety = self.enforce_safety().await;
        match result {
            Err(error) => Err(error),
            Ok(()) => {
                observation?;
                safety
            }
        }
    }

    pub async fn resolve_current(&mut self, name: &str, output_only: bool) -> AppResult<AudioNode> {
        validate_node_name(name)?;
        self.read_registry().await?;
        let node = self.registry.node(name)?;
        if !(node.media_class.starts_with("Audio/Sink")
            || (!output_only && node.media_class.starts_with("Audio/Source")))
        {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Il nodo non e' un'uscita o ingresso audio controllabile.",
            ));
        }
        Ok(node)
    }

    pub async fn write_default(
        &mut self,
        name: &str,
        expected: Option<&Option<String>>,
    ) -> AppResult<()> {
        let node = self.resolve_current(name, true).await?;
        let previous = self.registry.defaults().1;
        if expected.is_some_and(|expected| *expected != previous) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Scelta esterna del default conservata; transazione annullata.",
            ));
        }
        if name.starts_with(APP_NODE_PREFIX) {
            let record = surround::read_restore(&self.paths)?.ok_or_else(|| {
                AppError::new(
                    ErrorCode::NotConfirmed,
                    "Il filtro non dispone di uno stato di ripristino valido.",
                )
            })?;
            if record.virtual_node_name != name
                || record.app_process_id != std::process::id()
                || !verify_graph(&self.registry, &record)?
            {
                return Err(AppError::new(
                    ErrorCode::NotConfirmed,
                    "Il filtro non e' verificato subito prima del cambio default.",
                ));
            }
        }
        self.mutate_default(Some(&node), previous.as_deref())
            .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            self.read_registry().await?;
            if self.registry.node(name)?.object_serial != node.object_serial {
                return Err(AppError::new(
                    ErrorCode::NotConfirmed,
                    "Identita del target cambiata durante il cambio default.",
                ));
            }
            let (effective, configured, _) = self.registry.defaults();
            if configured.as_deref() == Some(name)
                && (!name.starts_with(APP_NODE_PREFIX) || effective.as_deref() == Some(name))
            {
                return Ok(());
            }
            if (configured != previous && configured.as_deref() != Some(name))
                || tokio::time::Instant::now() >= deadline
            {
                return Err(AppError::new(
                    ErrorCode::NotConfirmed,
                    "Cambio del default non confermato o superato da una scelta esterna.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn write_volume(&mut self, name: &str, volume: f64) -> AppResult<()> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Volume ammesso da 0 a 1, senza amplificazione fisica.",
            ));
        }
        let node = self.resolve_current(name, false).await?;
        self.mutate_level(&node, Some(volume), None).await?;
        self.read_registry().await?;
        let observed = self.registry.node(name)?;
        if observed.object_serial != node.object_serial
            || !observed
                .volume
                .is_some_and(|actual| (actual - volume).abs() <= 0.002)
        {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Volume non confermato dal nodo o dalla route PipeWire.",
            ));
        }
        Ok(())
    }

    pub async fn write_mute(&mut self, name: &str, muted: bool) -> AppResult<()> {
        let node = self.resolve_current(name, false).await?;
        self.mutate_level(&node, None, Some(muted)).await?;
        self.read_registry().await?;
        let observed = self.registry.node(name)?;
        if observed.object_serial != node.object_serial || observed.muted != Some(muted) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Mute non confermato dal nodo PipeWire.",
            ));
        }
        Ok(())
    }

    pub async fn write_level(&mut self, name: &str, volume: f64, muted: bool) -> AppResult<()> {
        let node = self.resolve_current(name, false).await?;
        self.mutate_level(&node, Some(volume), Some(muted)).await?;
        self.read_registry().await?;
        let observed = self.registry.node(name)?;
        if observed.object_serial != node.object_serial
            || observed.muted != Some(muted)
            || !observed
                .volume
                .is_some_and(|actual| (actual - volume).abs() <= 0.002)
        {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Livello e mute non confermati dall'istanza audio.",
            ));
        }
        Ok(())
    }

    pub async fn clear_default(&mut self, expected: &Option<String>) -> AppResult<()> {
        self.read_registry().await?;
        if self.registry.defaults().1 != *expected {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Scelta esterna del default conservata.",
            ));
        }
        self.mutate_default(None, expected.as_deref()).await?;
        self.read_registry().await?;
        if self.registry.defaults().1.is_some() {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Ripristino del default automatico non confermato.",
            ));
        }
        Ok(())
    }

    pub async fn select_output(&mut self, name: &str) -> AppResult<()> {
        self.resolve_current(name, true).await?;
        if name.starts_with(APP_NODE_PREFIX) {
            return Err(AppError::new(
                ErrorCode::Validation,
                "Il nodo 7.1 si seleziona soltanto dopo la verifica del filtro.",
            ));
        }
        if self.filter.is_some() {
            self.stop_filter_to(RestoreIntent::Bypass, Some(name))
                .await?;
        }
        self.write_default(name, None).await?;
        self.selected_output = (!self.registry.node(name)?.is_virtual).then(|| name.into());
        self.reason = None;
        Ok(())
    }

    pub async fn recover_pending(&mut self) -> AppResult<()> {
        if !self.recovery_pending {
            return Ok(());
        }
        if self.filter.is_some() {
            self.stop_filter(RestoreIntent::Bypass).await
        } else {
            self.recover_stale().await
        }
    }

    pub async fn recover_stale(&mut self) -> AppResult<()> {
        if self.filter.is_some() {
            return Err(AppError::new(
                ErrorCode::Busy,
                "Terminare prima il DSP posseduto; non e' una sessione stale.",
            ));
        }
        let record = match surround::read_restore(&self.paths) {
            Ok(record) => record,
            Err(error) => {
                self.state = SurroundState::Error;
                self.reason = Some(error.message.clone());
                return Err(error);
            }
        };
        if let Some(record) = record {
            // A recovery record is never permission to signal a process by a persisted PID.
            self.restore_default(&record, RestoreIntent::Recovery)
                .await?;
            if self.registry.node(&record.virtual_node_name).is_ok()
                || self.registry.node(&record.playback_node_name).is_ok()
            {
                self.state = SurroundState::Error;
                self.reason = Some("Una vecchia sessione surround e' ancora visibile: nessun PID persistito viene terminato e nessun secondo filtro viene creato.".into());
                return Err(AppError::new(
                    ErrorCode::Busy,
                    self.reason.clone().unwrap_or_default(),
                ));
            }
            surround::clear_restore(&self.paths, &record.session_id)?;
            self.selected_output = Some(record.selected_physical_target.clone());
            let directory = self.paths.runtime_dir.join(&record.session_id);
            let _ = std::fs::remove_file(directory.join("spatializer.conf"));
            let _ = std::fs::remove_dir(directory);
            self.reason = None;
        }
        self.recovery_pending = false;
        self.state = SurroundState::Off;
        Ok(())
    }

    pub(crate) async fn restore_default(
        &mut self,
        record: &RestoreRecord,
        intent: RestoreIntent,
    ) -> AppResult<()> {
        self.read_registry().await?;
        let (_, configured, _) = self.registry.defaults();
        let outputs = self.registry.nodes(true);
        let target_present = outputs.iter().any(|node| {
            node.node_name == record.selected_physical_target
                && !node.is_virtual
                && (node.object_serial == Some(record.target_serial)
                    || (intent == RestoreIntent::Recovery
                        && self.registry.cookie() != record.core_cookie))
        });
        let previous_present = record
            .previous_configured_default
            .as_ref()
            .is_some_and(|name| {
                outputs.iter().any(|node| {
                    &node.node_name == name && !node.node_name.starts_with(APP_NODE_PREFIX)
                })
            });
        let fallback = record
            .previous_configured_default
            .as_ref()
            .and_then(|name| {
                outputs
                    .iter()
                    .find(|node| &node.node_name == name && !node.is_virtual)
            })
            .or_else(|| {
                record.previous_effective_default.as_ref().and_then(|name| {
                    outputs
                        .iter()
                        .find(|node| &node.node_name == name && !node.is_virtual)
                })
            })
            .or_else(|| outputs.iter().find(|node| !node.is_virtual));
        let action = surround::restore_decision(
            record,
            configured.as_deref(),
            target_present,
            previous_present,
            fallback.map(|node| node.node_name.as_str()),
            intent,
        );
        if action == RestoreAction::KeepExternal {
            return Ok(());
        }
        let target_name = match &action {
            RestoreAction::SetOutput(name) => Some(name.as_str()),
            _ => None,
        };
        let node = match target_name {
            Some(name) => Some(self.resolve_current(name, true).await?),
            None => {
                self.read_registry().await?;
                None
            }
        };
        // Observe ownership immediately before the write, not from a previous UI snapshot.
        if self.registry.defaults().1.as_deref() != Some(record.virtual_node_name.as_str()) {
            return Ok(());
        }
        self.mutate_default(node.as_ref(), Some(&record.virtual_node_name))
            .await?;
        self.read_registry().await?;
        if self.registry.defaults().1.as_deref() == Some(record.virtual_node_name.as_str()) {
            return Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Il ripristino del default audio non e' confermato.",
            ));
        }
        Ok(())
    }

    pub(crate) async fn stop_filter(&mut self, intent: RestoreIntent) -> AppResult<()> {
        self.stop_filter_to(intent, None).await
    }

    pub(crate) async fn stop_filter_to(
        &mut self,
        intent: RestoreIntent,
        replacement: Option<&str>,
    ) -> AppResult<()> {
        self.state = SurroundState::Stopping;
        let Some(record) = self.filter.as_ref().map(|filter| filter.record.clone()) else {
            let result = if self.recovery_pending && self.available {
                self.recover_stale().await
            } else {
                Ok(())
            };
            self.state = if result.is_ok() {
                SurroundState::Off
            } else {
                SurroundState::Error
            };
            if let Err(error) = &result {
                self.reason = Some(error.message.clone());
            }
            return result;
        };
        // Ownership and pending recovery survive cancellation as well as errors.
        self.recovery_pending = true;
        let replacement_baseline = self.registry.defaults().1;
        // Kill the owned DSP before any fallback. It must never follow a fallback to speakers.
        let stopped = self.terminate_current_filter().await;
        if stopped.is_ok() {
            self.filter.take();
        }
        let restored = if stopped.is_err() {
            Err(AppError::new(
                ErrorCode::NotConfirmed,
                "Terminazione del DSP non confermata; ripristino rinviato.",
            ))
        } else if let Some(name) = replacement {
            self.write_default(name, Some(&replacement_baseline)).await
        } else {
            self.restore_default(&record, intent).await
        };
        let cleared = if stopped.is_ok() && restored.is_ok() {
            surround::clear_restore(&self.paths, &record.session_id)
        } else {
            Ok(())
        };
        self.recovery_pending = stopped.is_err() || restored.is_err() || cleared.is_err();
        self.state = if !self.recovery_pending {
            SurroundState::Off
        } else {
            SurroundState::Error
        };
        if self.state == SurroundState::Off {
            self.reason = None;
        }
        if let Some(error) = stopped
            .as_ref()
            .err()
            .or(restored.as_ref().err())
            .or(cleared.as_ref().err())
        {
            self.reason = Some(error.message.clone());
        }
        stopped?;
        restored?;
        cleared
    }

    pub async fn enforce_safety(&mut self) -> AppResult<()> {
        let Some(filter) = self.filter.as_mut() else {
            return Ok(());
        };
        let error = if !self.available {
            Some(AppError::new(
                ErrorCode::AudioUnavailable,
                "PipeWire non disponibile; filtro fermato.",
            ))
        } else if filter.process.has_exited()? {
            Some(
                AppError::new(
                    ErrorCode::AudioUnavailable,
                    "Il client surround e' terminato.",
                )
                .detail(filter.process.stderr()),
            )
        } else if !self.hrtf.as_ref().is_some_and(HrtfSelection::readable) {
            Some(AppError::new(
                ErrorCode::HrtfUnavailable,
                "Il file SOFA selezionato non e' piu leggibile.",
            ))
        } else {
            match verify_graph(&self.registry, &filter.record) {
                Ok(true) => None,
                Ok(false) => Some(AppError::new(
                    ErrorCode::NotConfirmed,
                    "Il filtro ha perso i suoi nodi, le porte o i link stereo.",
                )),
                Err(error) => Some(error),
            }
        };
        if let Some(error) = error {
            let intent = if error.code == ErrorCode::TargetUnavailable {
                RestoreIntent::TargetLost
            } else {
                RestoreIntent::Bypass
            };
            let _ = self.stop_filter(intent).await;
            self.state = SurroundState::Error;
            self.reason = Some(error.message.clone());
            return Err(error);
        }
        Ok(())
    }

    pub async fn enable_surround(&mut self, level: Option<(f64, bool)>) -> AppResult<()> {
        let result = self.enable_surround_inner(level).await;
        if let Err(error) = &result {
            if self.filter.is_none() {
                self.state = SurroundState::Error;
                self.recovery_pending =
                    surround::read_restore(&self.paths).map_or(true, |record| record.is_some());
            }
            self.reason = Some(error.message.clone());
        }
        result
    }

    pub async fn enable_surround_inner(&mut self, level: Option<(f64, bool)>) -> AppResult<()> {
        self.read_registry().await?;
        self.recover_pending().await?;
        let name = self.selected_output.clone().ok_or_else(|| {
            AppError::new(
                ErrorCode::TargetUnavailable,
                "Selezionare un'uscita fisica.",
            )
        })?;
        let target = self.registry.node(&name)?;
        stereo_target(&target)?;
        if !same_channels(&self.registry.port_channels(target.id, "in"), &["FL", "FR"]) {
            return Err(AppError::new(
                ErrorCode::TargetUnavailable,
                "Le porte stereo del target non sono disponibili.",
            ));
        }
        let reference = self
            .hrtf
            .as_ref()
            .map(|selection| selection.status.reference.as_str())
            .unwrap_or(SYSTEM_HRTF);
        let hrtf = self.resolve_hrtf(reference).await?;
        if let Some(filter) = &self.filter {
            if filter.record.selected_physical_target == name
                && filter.hrtf_reference == hrtf.status.reference
                && self.registry.defaults().0.as_deref()
                    == Some(filter.record.virtual_node_name.as_str())
                && self.registry.defaults().1.as_deref()
                    == Some(filter.record.virtual_node_name.as_str())
            {
                self.enforce_safety().await?;
                if let Some((volume, mute)) = level {
                    let input = self
                        .filter
                        .as_ref()
                        .unwrap()
                        .record
                        .virtual_node_name
                        .clone();
                    let result = self.write_level(&input, volume, mute).await;
                    if result.is_err() {
                        let _ = self.stop_filter(RestoreIntent::Rollback).await;
                    }
                    result?;
                }
                self.state = SurroundState::On;
                self.reason = None;
                return Ok(());
            }
            self.stop_filter(RestoreIntent::Bypass).await?;
        }
        self.state = SurroundState::Starting;
        self.read_registry().await?;
        let (effective, configured, _) = self.registry.defaults();
        let current_target = self.registry.node(&name)?;
        if current_target.object_serial != target.object_serial {
            return Err(AppError::new(
                ErrorCode::TargetUnavailable,
                "Il target e' cambiato durante la preparazione del filtro.",
            ));
        }
        let target = current_target;
        stereo_target(&target)?;
        let filter = self
            .spawn_filter(&target, &hrtf, configured, effective)
            .await?;
        // The controller adopts the child before post-spawn persistence or any await.
        self.filter = Some(filter);
        self.recovery_pending = true;
        let mut record = self.filter.as_ref().unwrap().record.clone();
        let result = async {
            #[cfg(test)]
            if let Some(backend) = &mut self.test_backend {
                backend.prepare_post_spawn_store(&self.paths)?;
            }
            surround::write_restore(&self.paths, &record)?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
            loop {
                let filter = self.filter.as_mut().unwrap();
                if filter.process.has_exited()? {
                    return Err(AppError::new(ErrorCode::HrtfUnavailable, "Il filtro SOFA non si e' avviato: verificare il plugin PipeWire/libmysofa.").detail(filter.process.stderr()));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(AppError::new(ErrorCode::Timeout, "Nodi, otto porte di ingresso e link stereo del filtro non osservati in tempo.").detail(filter.process.stderr()));
                }
                self.read_registry().await?;
                if verify_graph(&self.registry, &record)? { break; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            record.virtual_object_serial = self.registry.node(&record.virtual_node_name)?.object_serial;
            record.playback_object_serial = self.registry.node(&record.playback_node_name)?.object_serial;
            surround::write_restore(&self.paths, &record)?;
            self.filter.as_mut().unwrap().record = record.clone();
            if let Some((volume, mute)) = level {
                self.write_level(&record.virtual_node_name, volume, mute).await?;
            }
            self.read_registry().await?;
            if !verify_graph(&self.registry, &record)? { return Err(AppError::new(ErrorCode::NotConfirmed, "Link del filtro persi prima del cambio default.")); }
            // Do not replace a choice made externally during the start transaction.
            if self.registry.defaults().1 != record.previous_configured_default {
                return Err(AppError::new(ErrorCode::NotConfirmed, "Default cambiato da un altro client durante l'attivazione; scelta esterna conservata."));
            }
            self.write_default(&record.virtual_node_name, Some(&record.previous_configured_default)).await?;
            self.read_registry().await?;
            if !verify_graph(&self.registry, &record)? { return Err(AppError::new(ErrorCode::NotConfirmed, "Il collegamento fisico non e' confermato dopo il cambio default.")); }
            if self.registry.defaults().0.as_deref() != Some(record.virtual_node_name.as_str()) {
                return Err(AppError::new(ErrorCode::NotConfirmed, "Il sink 7.1 non e' il default effettivo."));
            }
            Ok(())
        }.await;
        if let Err(error) = result {
            let _ = self.stop_filter(RestoreIntent::Rollback).await;
            self.state = SurroundState::Error;
            self.reason = Some(error.message.clone());
            return Err(error);
        }
        self.hrtf = Some(hrtf);
        self.recovery_pending = false;
        self.state = SurroundState::On;
        self.reason = None;
        Ok(())
    }

    pub async fn apply_validated_profile(&mut self, profile: &AudioProfile) -> AppResult<()> {
        validate_profile(profile)?;
        self.read_registry().await?;
        self.recover_pending().await?;
        let target = self.registry.node(&profile.output_node_name)?;
        if target.is_virtual || !target.media_class.starts_with("Audio/Sink") {
            return Err(AppError::new(
                ErrorCode::TargetUnavailable,
                "Il profilo deve indicare un'uscita fisica disponibile.",
            ));
        }
        let hrtf = match profile.hrtf.as_deref() {
            Some(reference) => Some(self.resolve_hrtf(reference).await?),
            None => None,
        };
        if profile.surround_requested {
            stereo_target(&target)?;
            if !same_channels(&self.registry.port_channels(target.id, "in"), &["FL", "FR"]) {
                return Err(AppError::new(
                    ErrorCode::TargetUnavailable,
                    "Porte stereo del target non disponibili.",
                ));
            }
        }
        // All target/file/schema checks precede the first audio mutation.
        let previous_selected = self.selected_output.clone();
        let previous_hrtf = self.hrtf.clone();
        let previous_default = self.registry.defaults().1;
        self.selected_output = Some(profile.output_node_name.clone());
        if let Some(hrtf) = hrtf {
            self.hrtf = Some(hrtf);
        }
        let result = if profile.surround_requested {
            self.enable_surround(Some((profile.volume, profile.mute)))
                .await
        } else {
            async {
                // Prepare the requested physical level before stop/fallback can
                // expose this output, and before selecting it as the default.
                self.write_level(&profile.output_node_name, profile.volume, profile.mute)
                    .await?;
                if self.filter.is_some() {
                    self.stop_filter_to(RestoreIntent::Bypass, Some(&profile.output_node_name))
                        .await?;
                }
                self.write_default(&profile.output_node_name, None).await
            }
            .await
        };
        if result.is_err() {
            self.selected_output = previous_selected;
            self.hrtf = previous_hrtf;
            if !profile.surround_requested
                && self.read_registry().await.is_ok()
                && self.registry.defaults().1.as_deref() == Some(profile.output_node_name.as_str())
            {
                match previous_default.filter(|name| !name.starts_with(APP_NODE_PREFIX)) {
                    Some(name) if self.registry.node(&name).is_ok() => {
                        let expected = Some(profile.output_node_name.clone());
                        let _ = self.write_default(&name, Some(&expected)).await;
                    }
                    _ => {
                        let expected = Some(profile.output_node_name.clone());
                        let _ = self.clear_default(&expected).await;
                    }
                }
                // Do not restore a higher physical volume after a partial application.
            }
        }
        result
    }
}
