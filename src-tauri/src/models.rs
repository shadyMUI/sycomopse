use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    UnsupportedPlatform,
    Validation,
    DeviceNotFound,
    ProviderMissing,
    PermissionDenied,
    Unsupported,
    Busy,
    Timeout,
    CommandFailed,
    NotConfirmed,
    InvalidData,
    Io,
    AudioUnavailable,
    HrtfUnavailable,
    TargetUnavailable,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub detail: Option<String>,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn detail(mut self, detail: impl AsRef<str>) -> Self {
        self.detail = Some(
            detail
                .as_ref()
                .chars()
                .filter(|c| !c.is_control() || *c == '\n')
                .take(2048)
                .collect(),
        );
        self
    }

    pub fn unsupported_platform() -> Self {
        Self::new(
            ErrorCode::UnsupportedPlatform,
            "Il backend hardware e audio richiede Linux. La demo si avvia solo con npm run demo.",
        )
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorCode::Io, "Operazione su file o processo non riuscita.")
            .detail(error.to_string())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Platform {
    Linux,
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    Openrazer,
    Ratbag,
    Upower,
    Headsetcontrol,
    Inputremapper,
    Pipewire,
    Usb,
    App,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Brand {
    Razer,
    Logitech,
    Generic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DeviceKind {
    Mouse,
    Headset,
    Receiver,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AvailabilityCode {
    Available,
    Absent,
    Unsupported,
    ProviderMissing,
    PermissionDenied,
    Unknown,
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub id: String,
    pub provider: ProviderKind,
    pub code: AvailabilityCode,
    pub severity: Severity,
    pub message: String,
    pub detail: Option<String>,
}

impl Diagnostic {
    pub fn new(
        id: impl Into<String>,
        provider: ProviderKind,
        code: AvailabilityCode,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            provider,
            code,
            severity,
            message: message.into(),
            detail: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub serial: Option<String>,
    pub physical_path: Option<String>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub transport: Option<String>,
    pub firmware: Option<String>,
    pub backend_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DpiValues {
    Range { min: u32, max: u32, step: u32 },
    Discrete { values: Vec<u32> },
    Unknown,
}

impl DpiValues {
    pub fn contains(&self, value: u32) -> bool {
        match self {
            Self::Range { min, max, step } => {
                *step > 0 && value >= *min && value <= *max && (value - min).is_multiple_of(*step)
            }
            Self::Discrete { values } => values.contains(&value),
            Self::Unknown => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiCapability {
    pub values: DpiValues,
    pub writable: bool,
    pub separate_axes: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollingCapability {
    pub values: Vec<u32>,
    pub writable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCapabilities {
    pub dpi: Option<DpiCapability>,
    pub polling: Option<PollingCapability>,
    pub remapping: Option<RemappingCapability>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RemappingIdentityStrength {
    Serial,
    Port,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MouseButtonDescriptor {
    pub id: String,
    pub label: String,
    pub code: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemappingCapability {
    pub provider: ProviderKind,
    pub availability: AvailabilityCode,
    pub writable: bool,
    pub fingerprint: Option<String>,
    pub identity_strength: Option<RemappingIdentityStrength>,
    pub buttons: Vec<MouseButtonDescriptor>,
    pub key_codes: Vec<u16>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MouseAction {
    Default,
    MouseButton {
        code: u16,
    },
    Key {
        code: u16,
    },
    Chord {
        modifiers: Vec<u16>,
        key: u16,
    },
    Autoclicker {
        cps: u32,
        #[serde(default = "default_autoclick_button")]
        button: u16,
    },
    Disabled,
}

fn default_autoclick_button() -> u16 {
    272
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MouseBinding {
    pub button_id: String,
    pub action: MouseAction,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum RemappingRuntimeState {
    #[default]
    Unavailable,
    Inactive,
    Starting,
    Active,
    Busy,
    Error,
    Paused,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemappingValues {
    pub state: RemappingRuntimeState,
    pub enabled: bool,
    pub auto_apply: bool,
    pub paused_for_safety: bool,
    pub profile_id: Option<String>,
    pub bindings: Vec<MouseBinding>,
    pub active_preset: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DpiValue {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BatteryLevel {
    Unknown,
    Critical,
    Low,
    Normal,
    High,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
    pub percentage: Option<f64>,
    pub level: Option<BatteryLevel>,
    pub charging: Option<bool>,
    pub source: ProviderKind,
    pub observed_at: u64,
    pub source_updated_at: Option<u64>,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceValues {
    pub dpi: Option<DpiValue>,
    pub polling_rate: Option<u32>,
    pub battery: Option<Battery>,
    pub remapping: Option<RemappingValues>,
    #[serde(skip)]
    pub staged: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SupportLevel {
    Supported,
    Partial,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub brand: Brand,
    pub kind: DeviceKind,
    pub provider: ProviderKind,
    pub identity: DeviceIdentity,
    pub availability: AvailabilityCode,
    pub support: SupportLevel,
    pub capabilities: DeviceCapabilities,
    pub values: DeviceValues,
    pub output_node_names: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Discovery {
    pub devices: Vec<Device>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioNode {
    pub id: u32,
    pub object_serial: Option<u64>,
    pub node_name: String,
    pub description: String,
    pub media_class: String,
    pub channels: Vec<String>,
    pub volume: Option<f64>,
    pub muted: Option<bool>,
    pub is_virtual: bool,
    pub device_serial: Option<String>,
    pub device_bus_path: Option<String>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub transport: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SurroundState {
    #[default]
    Off,
    Starting,
    On,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HrtfStatus {
    pub reference: String,
    pub display_name: String,
    pub readable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioSnapshot {
    pub available: bool,
    pub outputs: Vec<AudioNode>,
    pub inputs: Vec<AudioNode>,
    pub default_output: Option<String>,
    pub configured_default_output: Option<String>,
    pub selected_output: Option<String>,
    pub default_input: Option<String>,
    pub surround_state: SurroundState,
    pub can_enable_surround: bool,
    pub surround_reason: Option<String>,
    pub hrtf: Option<HrtfStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioProfile {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub output_node_name: String,
    pub hrtf: Option<String>,
    pub volume: f64,
    pub mute: bool,
    pub surround_requested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u64,
    pub platform: Platform,
    pub demo: bool,
    pub devices: Vec<Device>,
    pub audio: AudioSnapshot,
    pub profiles: Vec<AudioProfile>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            platform: if cfg!(target_os = "linux") {
                Platform::Linux
            } else {
                Platform::UnsupportedPlatform
            },
            demo: false,
            devices: Vec::new(),
            audio: AudioSnapshot::default(),
            profiles: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_nonuniform_dpi_and_each_axis() {
        let discrete = DpiValues::Discrete {
            values: vec![400, 800, 1200, 3200],
        };
        assert!(discrete.contains(1200));
        assert!(!discrete.contains(1600));
        assert!(!DpiValues::Unknown.contains(800));
        assert!(!DpiValues::Range {
            min: 100,
            max: 1000,
            step: 0
        }
        .contains(100));
        assert!(DpiValues::Range {
            min: 100,
            max: 1000,
            step: 50
        }
        .contains(150));
        assert!(!DpiValues::Range {
            min: 100,
            max: 1000,
            step: 50
        }
        .contains(151));
    }

    #[test]
    fn error_details_are_bounded_and_serialized() {
        let error =
            AppError::new(ErrorCode::NotConfirmed, "Non confermato").detail("x".repeat(5000));
        assert_eq!(error.detail.as_ref().unwrap().len(), 2048);
        assert_eq!(serde_json::to_value(error).unwrap()["code"], "notConfirmed");
    }

    #[test]
    fn staging_metadata_is_internal_not_an_ipc_capability() {
        let values = DeviceValues {
            staged: true,
            ..DeviceValues::default()
        };
        assert!(serde_json::to_value(values)
            .unwrap()
            .get("staged")
            .is_none());
    }
}
