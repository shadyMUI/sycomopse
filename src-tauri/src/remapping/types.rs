use crate::models::{
    AppError, AppResult, AvailabilityCode, DeviceIdentity, Diagnostic, ErrorCode, MouseAction,
    MouseBinding, ProviderKind, RemappingIdentityStrength, RemappingRuntimeState, RemappingValues,
    Severity,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::future::Future;
use std::path::Path;

pub const AGENT_UNIT: &str = "peripheral-hub-remapping-agent.service";
pub const MAX_BYTES: usize = 1024 * 1024;
pub const MAX_PROFILES: usize = 128;
pub const MAX_RECOVERY: usize = 512;
pub const MAX_BUTTONS: usize = 256;
pub const MODIFIERS: &[u16] = &[29, 42, 54, 56, 97, 100, 125, 126];

pub fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCode::Validation, message)
}

pub fn current_uid() -> u32 {
    #[cfg(target_os = "linux")]
    {
        unsafe { libc::geteuid() }
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

pub fn bounded_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

pub fn diagnostic(error: &AppError) -> Diagnostic {
    let mut item = Diagnostic::new(
        "remapping-runtime",
        ProviderKind::Inputremapper,
        match error.code {
            ErrorCode::PermissionDenied => AvailabilityCode::PermissionDenied,
            ErrorCode::Unsupported | ErrorCode::UnsupportedPlatform => {
                AvailabilityCode::Unsupported
            }
            ErrorCode::ProviderMissing => AvailabilityCode::ProviderMissing,
            _ => AvailabilityCode::Unknown,
        },
        Severity::Warning,
        error.message.clone(),
    );
    item.detail = error.detail.clone();
    item
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemappingProfile {
    pub id: String,
    pub fingerprint: String,
    pub identity_strength: RemappingIdentityStrength,
    pub identity: DeviceIdentity,
    pub bindings: Vec<MouseBinding>,
    pub enabled: bool,
    pub auto_apply: bool,
    pub allow_weak_identity: bool,
    pub paused_for_safety: bool,
    pub compiled_preset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Document {
    pub schema_version: u32,
    pub revision: u64,
    pub profiles: Vec<RemappingProfile>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            profiles: Vec::new(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryDocument {
    pub schema_version: u32,
    pub identities: Vec<String>,
    pub disarmed: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputButton {
    pub id: String,
    pub label: String,
    pub code: u16,
    pub node_id: String,
    pub origin_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputNode {
    pub id: String,
    pub origin_hash: String,
    pub name: String,
    pub path: String,
    pub phys: String,
    pub key_codes: Vec<u16>,
    pub relative_codes: Vec<u16>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub id: String,
    pub fingerprint: String,
    pub identity_strength: RemappingIdentityStrength,
    pub name: String,
    pub serial: Option<String>,
    pub physical_path: Option<String>,
    pub vendor_id: u16,
    pub product_id: u16,
    pub transport: String,
    pub nodes: Vec<InputNode>,
    pub buttons: Vec<InputButton>,
}

impl InputDevice {
    pub fn identity(&self) -> DeviceIdentity {
        DeviceIdentity {
            serial: self.serial.clone(),
            physical_path: self.physical_path.clone(),
            vendor_id: Some(self.vendor_id),
            product_id: Some(self.product_id),
            transport: Some(self.transport.clone()),
            ..DeviceIdentity::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub version: u32,
    pub generation: u64,
    pub keyboard_output_codes: Vec<u16>,
    pub devices: Vec<InputDevice>,
}

impl Inventory {
    pub fn validate(&self) -> AppResult<()> {
        if self.version != 1
            || self.devices.len() > MAX_PROFILES
            || self.keyboard_output_codes.len() > 768
        {
            return Err(invalid("Inventory Input Remapper non valida."));
        }
        let mut identities = HashSet::new();
        for device in &self.devices {
            if !bounded_identifier(&device.fingerprint)
                || !identities.insert(device.fingerprint.as_str())
                || device.nodes.is_empty()
                || device.nodes.len() > 32
                || device.buttons.len() > MAX_BUTTONS
                || device.name.len() > 512
                || device
                    .physical_path
                    .as_ref()
                    .is_some_and(|path| !path.starts_with("/sys/") || path.len() > 4096)
            {
                return Err(invalid("Identita input non valida o duplicata."));
            }
            let mut buttons = HashSet::new();
            for button in &device.buttons {
                if !bounded_identifier(&button.id)
                    || !buttons.insert(button.id.as_str())
                    || button.label.len() > 256
                    || !device.nodes.iter().any(|node| {
                        node.id == button.node_id
                            && node.origin_hash == button.origin_hash
                            && node.key_codes.contains(&button.code)
                    })
                {
                    return Err(invalid("Pulsante input senza sorgente verificata."));
                }
            }
            for node in &device.nodes {
                if !node
                    .path
                    .strip_prefix("/dev/input/event")
                    .is_some_and(|suffix| {
                        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
                    })
                    || node.path.len() > 128
                    || node.phys.starts_with("input-remapper")
                    || !bounded_identifier(&node.origin_hash)
                    || node.key_codes.len() > 768
                    || node.relative_codes.len() > 128
                {
                    return Err(invalid("Nodo input virtuale o non valido."));
                }
            }
        }
        Ok(())
    }

    pub fn output_codes(&self) -> Vec<u16> {
        let mut codes = self.keyboard_output_codes.clone();
        // Verified against the pinned engine's DEFAULT_UINPUTS["mouse"].
        codes.extend(272..=279);
        codes.sort_unstable();
        codes.dedup();
        codes
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeState {
    pub version: u32,
    pub state: String,
    pub preset: Option<String>,
    pub owner_uid: Option<u32>,
    pub generation: u64,
    pub fingerprint: Option<String>,
    pub message: Option<String>,
}

impl BridgeState {
    pub fn values(
        &self,
        profile: Option<&RemappingProfile>,
        disarmed: bool,
        recovery_failure: Option<&AppError>,
    ) -> RemappingValues {
        let foreign = self.owner_uid.is_some_and(|uid| uid != current_uid());
        let mut values = RemappingValues {
            state: if profile.is_some_and(|profile| profile.paused_for_safety) {
                RemappingRuntimeState::Paused
            } else {
                match self.state.as_str() {
                    "RUNNING" => RemappingRuntimeState::Active,
                    "STARTING" => RemappingRuntimeState::Starting,
                    "UNKNOWN" | "STOPPED" => RemappingRuntimeState::Inactive,
                    "NO_GRAB" => RemappingRuntimeState::Busy,
                    _ => RemappingRuntimeState::Error,
                }
            },
            enabled: profile.is_some_and(|profile| profile.enabled),
            auto_apply: profile.is_some_and(|profile| profile.auto_apply),
            paused_for_safety: profile.is_some_and(|profile| profile.paused_for_safety),
            profile_id: profile.map(|profile| profile.id.clone()),
            bindings: profile
                .map(|profile| profile.bindings.clone())
                .unwrap_or_default(),
            active_preset: self.preset.clone(),
            message: self.message.clone(),
        };
        if foreign {
            values.state = RemappingRuntimeState::Busy;
        } else {
            if profile.is_some_and(|profile| profile.enabled) && disarmed {
                values.state = RemappingRuntimeState::Paused;
                values.paused_for_safety = true;
                values.message =
                    Some("Recovery attiva: riapplicare esplicitamente per riattivare.".to_owned());
            }
            if let Some(error) = recovery_failure {
                values.state = RemappingRuntimeState::Error;
                values.message = Some(error.message.clone());
            }
        }
        values
    }
}

// The real bridge and regression fakes share the ownership/CAS relief path.
#[cfg(any(target_os = "linux", test))]
pub trait RemappingBackend: Sync {
    fn owner(&self) -> &str;
    fn inventory(&self) -> impl Future<Output = AppResult<Inventory>> + Send;
    fn state(&self, fingerprint: &str) -> impl Future<Output = AppResult<BridgeState>> + Send;
    fn stop(&self, fingerprint: &str, expected: &str)
        -> impl Future<Output = AppResult<()>> + Send;
    fn apply(
        &self,
        input: &InputDevice,
        generation: u64,
        preset: &str,
        expected: &str,
        directory: &Path,
    ) -> impl Future<Output = AppResult<()>> + Send;
}
