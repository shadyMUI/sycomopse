#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::models::*;
use crate::profiles::{self, AppPaths};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const AGENT_UNIT: &str = "peripheral-hub-remapping-agent.service";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_PROFILES: usize = 128;
const MAX_RECOVERY: usize = 512;
const MAX_BUTTONS: usize = 256;
const MODIFIERS: &[u16] = &[29, 42, 54, 56, 97, 100, 125, 126];

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCode::Validation, message)
}

fn current_uid() -> u32 {
    #[cfg(target_os = "linux")]
    {
        unsafe { libc::geteuid() }
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentLocation {
    schema_version: u32,
    config_dir: PathBuf,
}

fn location_path() -> AppResult<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| AppError::new(ErrorCode::Io, "Home dell'agente non disponibile."))?;
    Ok(base
        .home_dir()
        .join(".local/state/peripheral-hub/remapping-agent.json"))
}

fn save_agent_location(paths: &AppPaths) -> AppResult<()> {
    let bytes = serde_json::to_vec(&AgentLocation {
        schema_version: 1,
        config_dir: paths.config_dir.clone(),
    })
    .map_err(|error| {
        invalid("Configurazione agente non serializzabile.").detail(error.to_string())
    })?;
    profiles::atomic_write(&location_path()?, &bytes)
}

fn agent_paths() -> AppResult<AppPaths> {
    let mut paths = profiles::ProfileStore::new()?.paths().clone();
    let location = location_path()?;
    match fs::symlink_metadata(&location) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let mut bytes = Vec::new();
    private_file(&location)?
        .take(4097)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid("Configurazione agente troppo grande."));
    }
    let location: AgentLocation = serde_json::from_slice(&bytes)
        .map_err(|error| invalid("Configurazione agente non valida.").detail(error.to_string()))?;
    if location.schema_version != 1
        || !location.config_dir.is_absolute()
        || location
            .config_dir
            .as_os_str()
            .to_string_lossy()
            .chars()
            .any(char::is_control)
    {
        return Err(invalid("Percorso o versione dell'agente non validi."));
    }
    paths.config_dir = location.config_dir;
    Ok(paths)
}

fn bounded_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn diagnostic(error: &AppError) -> Diagnostic {
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
struct Document {
    schema_version: u32,
    revision: u64,
    profiles: Vec<RemappingProfile>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryDocument {
    schema_version: u32,
    identities: Vec<String>,
    disarmed: Vec<String>,
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

pub struct Store {
    paths: AppPaths,
}

struct StoreLock(File);

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn private_file(path: &Path) -> AppResult<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_BYTES as u64
    {
        return Err(invalid("File rimappatura non regolare o troppo grande."));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        if metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 || metadata.nlink() != 1
        {
            return Err(AppError::new(
                ErrorCode::PermissionDenied,
                "File rimappatura non privato.",
            ));
        }
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || opened.len() > MAX_BYTES as u64 {
        return Err(invalid("File rimappatura non regolare o troppo grande."));
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.uid() != current_uid() || opened.mode() & 0o077 != 0 || opened.nlink() != 1 {
            return Err(AppError::new(
                ErrorCode::PermissionDenied,
                "File rimappatura cambiato durante la lettura.",
            ));
        }
    }
    Ok(file)
}

impl Store {
    pub fn at(paths: AppPaths) -> Self {
        Self { paths }
    }

    fn directory(&self) -> PathBuf {
        self.paths.config_dir.join("remapping")
    }

    fn path(&self) -> PathBuf {
        self.paths.config_dir.join("mouse-remapping.json")
    }

    fn recovery(&self) -> AppResult<RecoveryDocument> {
        let path = self.paths.config_dir.join("remapping-recovery.json");
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RecoveryDocument {
                    schema_version: 1,
                    ..RecoveryDocument::default()
                })
            }
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        let mut bytes = Vec::new();
        private_file(&path)?
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        let data: RecoveryDocument = serde_json::from_slice(&bytes)
            .map_err(|error| invalid("Registro recovery non valido.").detail(error.to_string()))?;
        if data.schema_version != 1
            || data.identities.len() > MAX_RECOVERY
            || data.disarmed.len() > MAX_RECOVERY
            || data
                .identities
                .iter()
                .any(|identity| !bounded_identifier(identity))
            || data
                .disarmed
                .iter()
                .any(|identity| !data.identities.contains(identity))
        {
            return Err(invalid("Registro recovery non supportato."));
        }
        Ok(data)
    }

    fn disarm(&self, identities: &[String], disarmed: bool) -> AppResult<()> {
        let mut data = self.recovery()?;
        for identity in identities {
            if !bounded_identifier(identity) {
                return Err(invalid("Identita recovery non valida."));
            }
            if !data.identities.contains(identity) {
                data.identities.push(identity.clone());
            }
            if disarmed && !data.disarmed.contains(identity) {
                data.disarmed.push(identity.clone());
            }
            if !disarmed {
                data.disarmed.retain(|entry| entry != identity);
            }
        }
        if data.identities.len() > MAX_RECOVERY {
            return Err(invalid("Limite recovery raggiunto."));
        }
        let bytes = serde_json::to_vec(&data).map_err(|error| {
            invalid("Registro recovery non serializzabile.").detail(error.to_string())
        })?;
        profiles::atomic_write(
            &self.paths.config_dir.join("remapping-recovery.json"),
            &bytes,
        )
    }

    fn lock(&self) -> AppResult<StoreLock> {
        profiles::private_dir(&self.paths.config_dir)?;
        let path = self.paths.config_dir.join("mouse-remapping.lock");
        if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(AppError::new(
                ErrorCode::PermissionDenied,
                "Lock rimappatura non sicuro.",
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(path)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata()?;
            if metadata.uid() != current_uid()
                || metadata.mode() & 0o077 != 0
                || metadata.nlink() != 1
            {
                return Err(AppError::new(
                    ErrorCode::PermissionDenied,
                    "Lock rimappatura non privato.",
                ));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(StoreLock(file)),
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(AppError::new(
                        ErrorCode::Busy,
                        "Un'altra operazione modifica le rimappature.",
                    ));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn load(&self) -> AppResult<Document> {
        match fs::symlink_metadata(self.path()) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Document::default())
            }
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        let file = private_file(&self.path())?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("Profili rimappatura troppo grandi."));
        }
        let document: Document = serde_json::from_slice(&bytes).map_err(|error| {
            invalid("Profili rimappatura non validi; file originale conservato.")
                .detail(error.to_string())
        })?;
        if document.schema_version != 1 || document.profiles.len() > MAX_PROFILES {
            return Err(invalid(
                "Versione o numero di profili rimappatura non supportato.",
            ));
        }
        let mut ids = HashSet::new();
        let mut identities = HashSet::new();
        for profile in &document.profiles {
            if Uuid::parse_str(&profile.id)
                .ok()
                .is_none_or(|id| id.to_string() != profile.id)
                || !ids.insert(profile.id.as_str())
                || !bounded_identifier(&profile.fingerprint)
                || !identities.insert(profile.fingerprint.as_str())
                || profile.bindings.len() > MAX_BUTTONS
                || profile.auto_apply
                    && profile.identity_strength == RemappingIdentityStrength::Port
                    && !profile.allow_weak_identity
                || profile
                    .compiled_preset
                    .as_deref()
                    .is_some_and(|name| !valid_preset_name(name, current_uid()))
            {
                return Err(invalid("Profilo rimappatura non sicuro o duplicato."));
            }
            let mut buttons = HashSet::new();
            for binding in &profile.bindings {
                if !bounded_identifier(&binding.button_id)
                    || !buttons.insert(binding.button_id.as_str())
                {
                    return Err(invalid("Sorgenti duplicate nel profilo rimappatura."));
                }
                validate_action(&binding.action, None)?;
            }
        }
        Ok(document)
    }

    fn save(&self, document: &mut Document) -> AppResult<()> {
        document.revision = document.revision.saturating_add(1);
        let bytes = serde_json::to_vec_pretty(document).map_err(|error| {
            invalid("Serializzazione profili fallita.").detail(error.to_string())
        })?;
        if bytes.len() > MAX_BYTES || document.profiles.len() > MAX_PROFILES {
            return Err(invalid("Limite profili rimappatura raggiunto."));
        }
        profiles::atomic_write(&self.path(), &bytes)
    }

    async fn acquire_lock(&self) -> AppResult<StoreLock> {
        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || Store::at(paths).lock())
            .await
            .map_err(|error| {
                AppError::new(ErrorCode::Io, "Lock rimappatura non disponibile.")
                    .detail(error.to_string())
            })?
    }

    fn preset(&self, profile: &RemappingProfile, device: &InputDevice) -> AppResult<String> {
        let mut bindings = Vec::new();
        for binding in &profile.bindings {
            if binding.action == MouseAction::Default {
                continue;
            }
            let button = device
                .buttons
                .iter()
                .find(|button| button.id == binding.button_id)
                .ok_or_else(|| invalid("Il pulsante non e piu presente sul mouse."))?;
            bindings.push(CompiledBinding {
                input: CompiledInput {
                    r#type: 1,
                    code: button.code,
                    origin_hash: button.origin_hash.clone(),
                    node_id: button.node_id.clone(),
                },
                output: binding.action.clone(),
            });
        }
        let bytes = serde_json::to_vec(&CompiledPreset {
            schema_version: 1,
            owner_uid: current_uid(),
            profile_id: profile.id.clone(),
            fingerprint: profile.fingerprint.clone(),
            bindings,
        })
        .map_err(|error| invalid("Preset non serializzabile.").detail(error.to_string()))?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let name = format!("ph-{}-{}-{hash}", current_uid(), profile.id);
        let parent = self.directory().join("presets");
        profiles::private_dir(&self.directory())?;
        profiles::private_dir(&parent)?;
        let path = parent.join(format!("{name}.json"));
        if path.exists() {
            let mut existing = Vec::new();
            private_file(&path)?
                .take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut existing)?;
            if existing != bytes {
                return Err(invalid("Preset immutabile modificato esternamente."));
            }
        } else {
            profiles::write_new_private(&path, &bytes)?;
        }
        Ok(name)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompiledPreset {
    schema_version: u32,
    owner_uid: u32,
    profile_id: String,
    fingerprint: String,
    bindings: Vec<CompiledBinding>,
}

#[derive(Serialize)]
struct CompiledBinding {
    input: CompiledInput,
    output: MouseAction,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompiledInput {
    r#type: u8,
    code: u16,
    origin_hash: String,
    node_id: String,
}

fn valid_preset_name(name: &str, uid: u32) -> bool {
    let Some(rest) = name.strip_prefix(&format!("ph-{uid}-")) else {
        return false;
    };
    rest.len() == 101
        && rest
            .get(..36)
            .is_some_and(|value| Uuid::parse_str(value).is_ok())
        && rest.as_bytes()[36] == b'-'
        && rest[37..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn unchanged_observation(
    previous: &(String, Option<String>, u64),
    owner: &str,
    preset: &Option<String>,
    generation: u64,
) -> bool {
    previous.0 == owner && previous.1 == *preset && previous.2 == generation
}

fn validate_action(action: &MouseAction, codes: Option<&[u16]>) -> AppResult<()> {
    let valid_key = |code: u16| {
        code > 0
            && code <= 767
            && !(272..=351).contains(&code)
            && codes.is_none_or(|values| values.contains(&code))
    };
    match action {
        MouseAction::Default | MouseAction::Disabled => Ok(()),
        MouseAction::MouseButton { code }
            if (272..=279).contains(code) && codes.is_none_or(|values| values.contains(code)) =>
        {
            Ok(())
        }
        MouseAction::Key { code } if valid_key(*code) => Ok(()),
        MouseAction::Chord { modifiers, key }
            if !modifiers.is_empty()
                && modifiers.len() <= 4
                && valid_key(*key)
                && !MODIFIERS.contains(key)
                && modifiers
                    .iter()
                    .all(|code| MODIFIERS.contains(code) && valid_key(*code))
                && modifiers.iter().collect::<HashSet<_>>().len() == modifiers.len() =>
        {
            Ok(())
        }
        _ => Err(invalid(
            "Azione non consentita: usare clic, tasto o scorciatoia senza macro.",
        )),
    }
}

fn validate_bindings(
    bindings: &[MouseBinding],
    device: &InputDevice,
    codes: &[u16],
) -> AppResult<()> {
    if bindings.len() > MAX_BUTTONS {
        return Err(invalid("Troppi pulsanti nel profilo."));
    }
    let mut seen = HashSet::new();
    for binding in bindings {
        if !seen.insert(binding.button_id.as_str())
            || !device
                .buttons
                .iter()
                .any(|button| button.id == binding.button_id)
        {
            return Err(invalid("Pulsante non rilevato o duplicato."));
        }
        validate_action(&binding.action, Some(codes))?;
    }
    let has_left = device.buttons.iter().any(|button| {
        let action = bindings
            .iter()
            .find(|binding| binding.button_id == button.id)
            .map(|binding| &binding.action);
        matches!(action, Some(MouseAction::MouseButton { code: 272 }))
            || button.code == 272 && matches!(action, None | Some(MouseAction::Default))
    });
    if !has_left {
        return Err(invalid(
            "Conservare almeno un pulsante per il clic sinistro.",
        ));
    }
    Ok(())
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
    fn identity(&self) -> DeviceIdentity {
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
    fn validate(&self) -> AppResult<()> {
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

    fn output_codes(&self) -> Vec<u16> {
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
    fn values(&self, profile: Option<&RemappingProfile>) -> RemappingValues {
        RemappingValues {
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
        }
    }
}

pub struct Controller {
    store: Store,
}

impl Controller {
    pub fn new(paths: AppPaths) -> Self {
        Self {
            store: Store::at(paths),
        }
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
                    .or_else(|| foreign.then(|| "Mouse gestito da un altro utente.".to_owned())),
            });
            let mut values = state.values(profile);
            if foreign {
                values.state = RemappingRuntimeState::Busy;
            }
            if profile.is_some_and(|profile| profile.enabled)
                && recovery
                    .as_ref()
                    .ok()
                    .is_some_and(|data| data.disarmed.contains(&input.fingerprint))
            {
                values.state = RemappingRuntimeState::Paused;
                values.paused_for_safety = true;
                values.message =
                    Some("Recovery attiva: riapplicare esplicitamente per riattivare.".to_owned());
            }
            device.values.remapping = Some(values);
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
            let _ = bridge.stop_owned(fingerprint).await;
            return Err(error);
        }
        profile.paused_for_safety = false;
        document.profiles[index] = profile;
        if let Err(error) = self.store.save(&mut document) {
            let _ = bridge.stop_owned(fingerprint).await;
            return Err(error);
        }
        if let Err(error) = self.store.disarm(&[fingerprint.to_owned()], false) {
            let _ = bridge.stop_owned(fingerprint).await;
            return Err(error);
        }
        Ok(())
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
        let _lock = self.store.acquire_lock().await?;
        self.store.disarm(&[fingerprint.to_owned()], true)?;
        if let Ok(mut document) = self.store.load() {
            if let Some(profile) = document
                .profiles
                .iter_mut()
                .find(|profile| profile.fingerprint == fingerprint)
            {
                profile.enabled = false;
                profile.auto_apply = false;
                profile.paused_for_safety = false;
                self.store.save(&mut document)?;
            }
        }
        linux::Bridge::connect()
            .await?
            .stop_owned(fingerprint)
            .await
    }

    #[cfg(target_os = "linux")]
    pub async fn disable_all(&self) -> AppResult<()> {
        let _lock = self.store.acquire_lock().await?;
        let mut known = self
            .store
            .recovery()
            .map(|data| data.identities)
            .unwrap_or_default();
        if let Ok(mut document) = self.store.load() {
            for profile in &mut document.profiles {
                known.push(profile.fingerprint.clone());
                profile.enabled = false;
                profile.auto_apply = false;
                profile.paused_for_safety = false;
            }
            self.store.save(&mut document)?;
        }
        self.store.disarm(&known, true)?;
        let bridge = linux::Bridge::connect().await?;
        let identities = self.recovery_identities(&bridge).await?;
        self.store.disarm(&identities, true)?;
        let mut failure = None;
        for identity in identities {
            if let Err(error) = bridge.stop_owned(&identity).await {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    #[cfg(target_os = "linux")]
    async fn release_owned(&self) -> AppResult<()> {
        let _lock = self.store.acquire_lock().await?;
        let bridge = linux::Bridge::connect().await?;
        let mut failure = None;
        for identity in self.recovery_identities(&bridge).await? {
            if let Err(error) = bridge.stop_owned(&identity).await {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    #[cfg(target_os = "linux")]
    async fn recovery_identities(&self, bridge: &linux::Bridge) -> AppResult<Vec<String>> {
        let mut identities = self
            .store
            .recovery()
            .map(|data| data.identities)
            .unwrap_or_default();
        if let Ok(document) = self.store.load() {
            identities.extend(
                document
                    .profiles
                    .into_iter()
                    .map(|profile| profile.fingerprint),
            );
        }
        match bridge.inventory().await {
            Ok(inventory) => identities.extend(
                inventory
                    .devices
                    .into_iter()
                    .map(|device| device.fingerprint),
            ),
            Err(error) if identities.is_empty() => return Err(error),
            Err(_) => {}
        }
        identities.sort();
        identities.dedup();
        Ok(identities)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::providers::dbus::Bus;
    pub const SERVICE: &str = "io.peripheralhub.InputRemapper";
    const PATH: &str = "/io/peripheralhub/InputRemapper";
    const INTERFACE: &str = "io.peripheralhub.InputRemapper1";

    pub struct Bridge {
        pub(super) bus: Bus,
    }

    fn parse<T: serde::de::DeserializeOwned>(json: String) -> AppResult<T> {
        if json.len() > MAX_BYTES {
            return Err(invalid("Risposta bridge troppo grande."));
        }
        serde_json::from_str(&json)
            .map_err(|error| invalid("Risposta bridge non valida.").detail(error.to_string()))
    }

    fn bridge_error(mut error: AppError) -> AppError {
        let detail = error.detail.as_deref().unwrap_or("");
        if detail.contains(".Busy") || detail.contains(".Conflict") || detail.contains(".Stale") {
            error.code = ErrorCode::Busy;
        } else if detail.contains(".Validation") || detail.contains(".InvalidPreset") {
            error.code = ErrorCode::Validation;
        }
        error
    }

    impl Bridge {
        pub async fn connect() -> AppResult<Self> {
            let bus = Bus::active(SERVICE, false).await.map_err(|error| AppError::new(error.code, "Backend di rimappatura non disponibile: installare o riparare i servizi Peripheral Hub.").detail(error.message))?;
            let api = bus.introspect(PATH).await?;
            for (method, input, output) in [
                ("GetVersion", "", "u"),
                ("Inventory", "", "s"),
                ("State", "s", "s"),
                ("Apply", "stsss", "s"),
                ("Stop", "ss", "s"),
            ] {
                if !api.method(INTERFACE, method, input, output) {
                    return Err(AppError::new(
                        ErrorCode::Unsupported,
                        "Protocollo del bridge non compatibile; riparare l'installazione.",
                    ));
                }
            }
            let version: u32 = bus.call(PATH, INTERFACE, "GetVersion", &()).await?;
            if version != 1 {
                return Err(invalid("Versione bridge non supportata."));
            }
            Ok(Self { bus })
        }

        pub async fn inventory(&self) -> AppResult<Inventory> {
            self.bus.ensure_owner(SERVICE).await?;
            let inventory: Inventory =
                parse(self.bus.call(PATH, INTERFACE, "Inventory", &()).await?)?;
            inventory.validate()?;
            self.bus.ensure_owner(SERVICE).await?;
            Ok(inventory)
        }

        pub async fn state(&self, fingerprint: &str) -> AppResult<BridgeState> {
            let state: BridgeState = parse(
                self.bus
                    .call(PATH, INTERFACE, "State", &(fingerprint,))
                    .await
                    .map_err(bridge_error)?,
            )?;
            if state.version != 1
                || state
                    .fingerprint
                    .as_deref()
                    .is_some_and(|value| value != fingerprint)
                || !matches!(
                    state.state.as_str(),
                    "UNKNOWN"
                        | "STARTING"
                        | "RUNNING"
                        | "STOPPED"
                        | "FAILED"
                        | "NO_GRAB"
                        | "UPGRADE_EVDEV"
                )
            {
                return Err(invalid("Stato bridge non coerente."));
            }
            self.bus.ensure_owner(SERVICE).await?;
            Ok(state)
        }

        pub async fn apply(
            &self,
            input: &InputDevice,
            generation: u64,
            preset: &str,
            expected: &str,
            directory: &Path,
        ) -> AppResult<()> {
            self.bus.ensure_owner(SERVICE).await?;
            let directory = directory
                .to_str()
                .ok_or_else(|| invalid("Percorso preset non UTF-8."))?;
            let _: BridgeState = parse(
                self.bus
                    .call(
                        PATH,
                        INTERFACE,
                        "Apply",
                        &(&input.fingerprint, generation, preset, expected, directory),
                    )
                    .await
                    .map_err(bridge_error)?,
            )?;
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                let state = self.state(&input.fingerprint).await?;
                if state.state == "RUNNING"
                    && state.preset.as_deref() == Some(preset)
                    && state.owner_uid == Some(current_uid())
                {
                    return Ok(());
                }
                if state.state != "STARTING" || Instant::now() >= deadline {
                    return Err(AppError::new(
                        ErrorCode::NotConfirmed,
                        "Rimappatura non confermata: nessun successo simulato.",
                    )
                    .detail(state.message.as_deref().unwrap_or(&state.state)));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        pub async fn stop(&self, fingerprint: &str, expected: &str) -> AppResult<()> {
            self.bus.ensure_owner(SERVICE).await?;
            let _: BridgeState = parse(
                self.bus
                    .call(PATH, INTERFACE, "Stop", &(fingerprint, expected))
                    .await
                    .map_err(bridge_error)?,
            )?;
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let state = self.state(fingerprint).await?;
                if matches!(state.state.as_str(), "STOPPED" | "UNKNOWN") && state.preset.is_none() {
                    return Ok(());
                }
                if state
                    .preset
                    .as_deref()
                    .is_some_and(|value| value != expected)
                {
                    return Err(AppError::new(
                        ErrorCode::Busy,
                        "Preset cambiato durante la disattivazione.",
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(AppError::new(
                        ErrorCode::NotConfirmed,
                        "Arresto della rimappatura non confermato.",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        pub async fn stop_owned(&self, fingerprint: &str) -> AppResult<()> {
            let state = self.state(fingerprint).await?;
            if state.owner_uid == Some(current_uid()) {
                if let Some(preset) = state
                    .preset
                    .filter(|preset| valid_preset_name(preset, current_uid()))
                {
                    self.stop(fingerprint, &preset).await?;
                }
            }
            Ok(())
        }
    }

    pub async fn active_session() -> AppResult<bool> {
        let bus = Bus::active("org.freedesktop.login1", false).await?;
        let sessions: Vec<(String, u32, String, String, zbus::zvariant::OwnedObjectPath)> = bus
            .call(
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
                "ListSessions",
                &(),
            )
            .await?;
        let mut active = Vec::new();
        for (_, uid, _, seat, path) in sessions {
            if seat.is_empty() {
                continue;
            }
            let kind: String = bus
                .get(path.as_str(), "org.freedesktop.login1.Session", "Type")
                .await?;
            let remote: bool = bus
                .get(path.as_str(), "org.freedesktop.login1.Session", "Remote")
                .await?;
            let running: bool = bus
                .get(path.as_str(), "org.freedesktop.login1.Session", "Active")
                .await?;
            if matches!(kind.as_str(), "x11" | "wayland") && !remote && running {
                active.push(uid);
            }
        }
        Ok(active.len() == 1 && active[0] == current_uid())
    }
}

#[cfg(target_os = "linux")]
pub async fn run_agent(disable_all: bool) -> AppResult<()> {
    use std::collections::{HashMap, VecDeque};
    if current_uid() == 0 {
        return Err(AppError::new(
            ErrorCode::PermissionDenied,
            "L'agente deve essere eseguito dall'utente, non da root.",
        ));
    }
    let installed_binary = std::env::current_exe()
        .ok()
        .is_some_and(|path| path.parent() == Some(Path::new("/usr/bin")));
    let paths = agent_paths()?;
    profiles::private_dir(&paths.runtime_dir)?;
    let mut options = OpenOptions::new();
    use std::os::unix::fs::OpenOptionsExt;
    options
        .write(true)
        .read(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let instance = options.open(paths.runtime_dir.join("remapping-agent.lock"))?;
    let mut controller = Controller::new(paths);
    if disable_all {
        return controller.disable_all().await;
    }
    instance
        .try_lock_exclusive()
        .map_err(|_| AppError::new(ErrorCode::Busy, "Agente gia attivo."))?;
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut seen: HashMap<String, (String, Option<String>, u64)> = HashMap::new();
    let mut present = HashSet::new();
    let mut unplug: HashMap<String, VecDeque<Instant>> = HashMap::new();
    let mut failures: HashMap<String, u8> = HashMap::new();
    loop {
        tokio::select! {
            _ = term.recv() => break,
            _ = interrupt.recv() => break,
            _ = tick.tick() => {}
        }
        // Native package removal must not leave a resident, endlessly retrying agent.
        if installed_binary && !Path::new("/usr/bin/peripheral-hub-remapping-agent").is_file() {
            break;
        }
        match agent_paths() {
            Ok(paths) if paths.config_dir != controller.store.paths.config_dir => {
                let _ = controller.release_owned().await;
                controller = Controller::new(paths);
                seen.clear();
                present.clear();
                unplug.clear();
                failures.clear();
            }
            Err(error) => {
                eprintln!("Configurazione agente: {}", error.message);
                continue;
            }
            _ => {}
        }
        let result: AppResult<()> = async {
            let _lock = controller.store.acquire_lock().await?;
            let mut document = controller.store.load()?;
            let recovery = controller.store.recovery()?;
            if document.profiles.is_empty() && recovery.identities.is_empty() {
                return Ok(());
            }
            let bridge = linux::Bridge::connect().await?;
            for identity in &recovery.disarmed {
                bridge.stop_owned(identity).await?;
            }
            if document.profiles.is_empty() {
                for identity in &recovery.identities {
                    bridge.stop_owned(identity).await?;
                }
                return Ok(());
            }
            if !linux::active_session().await.unwrap_or(false) {
                for profile in &document.profiles {
                    bridge.stop_owned(&profile.fingerprint).await?;
                }
                seen.clear();
                return Ok(());
            }
            let inventory = bridge.inventory().await?;
            let now = Instant::now();
            let current: HashSet<_> = inventory
                .devices
                .iter()
                .map(|device| device.fingerprint.clone())
                .collect();
            let mut changed = false;
            for identity in present.difference(&current) {
                let events = unplug.entry(identity.clone()).or_default();
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
                        .find(|profile| profile.fingerprint == *identity)
                    {
                        profile.paused_for_safety = true;
                        changed = true;
                    }
                }
                seen.remove(identity);
            }
            present = current;
            for index in 0..document.profiles.len() {
                let mut profile = document.profiles[index].clone();
                if !profile.enabled || recovery.disarmed.contains(&profile.fingerprint) {
                    bridge.stop_owned(&profile.fingerprint).await?;
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
                let state = bridge.state(&profile.fingerprint).await?;
                if state.generation != inventory.generation {
                    // A reconnect can happen between the two DBus replies.
                    seen.remove(&profile.fingerprint);
                    continue;
                }
                if input.identity_strength != profile.identity_strength
                    || input.identity_strength == RemappingIdentityStrength::Port
                        && !profile.allow_weak_identity
                    || validate_bindings(&profile.bindings, input, &inventory.output_codes())
                        .is_err()
                {
                    if state.owner_uid == Some(current_uid())
                        && state.preset == profile.compiled_preset
                    {
                        if let Some(preset) = &state.preset {
                            bridge.stop(&profile.fingerprint, preset).await?;
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
                    seen.insert(
                        profile.fingerprint.clone(),
                        (
                            bridge.bus.owner.clone(),
                            state.preset.clone(),
                            inventory.generation,
                        ),
                    );
                    failures.remove(&profile.fingerprint);
                    continue;
                }
                if seen.get(&profile.fingerprint).is_some_and(|previous| {
                    unchanged_observation(
                        previous,
                        &bridge.bus.owner,
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
                let name = controller.store.preset(&profile, input)?;
                controller
                    .store
                    .disarm(&[profile.fingerprint.clone()], true)?;
                profile.compiled_preset = Some(name.clone());
                profile.paused_for_safety = true;
                document.profiles[index] = profile.clone();
                controller.store.save(&mut document)?;
                changed = false;
                match bridge
                    .apply(
                        input,
                        inventory.generation,
                        &name,
                        state.preset.as_deref().unwrap_or(""),
                        &controller.store.directory(),
                    )
                    .await
                {
                    Ok(()) => {
                        profile.paused_for_safety = false;
                        document.profiles[index] = profile.clone();
                        controller.store.save(&mut document)?;
                        controller
                            .store
                            .disarm(&[profile.fingerprint.clone()], false)?;
                        seen.insert(
                            profile.fingerprint.clone(),
                            (bridge.bus.owner.clone(), Some(name), inventory.generation),
                        );
                        failures.remove(&profile.fingerprint);
                    }
                    Err(error) => {
                        let _ = bridge.stop_owned(&profile.fingerprint).await;
                        let count = failures.entry(profile.fingerprint.clone()).or_default();
                        *count = count.saturating_add(1);
                        if *count >= 3 {
                            profile.paused_for_safety = true;
                            document.profiles[index] = profile;
                            changed = true;
                        }
                        eprintln!("Riattivazione rimappatura: {}", error.message);
                    }
                }
            }
            if changed {
                controller.store.save(&mut document)?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = controller.release_owned().await;
            eprintln!("Agente rimappatura: {}", error.message);
        }
    }
    // Logout/stop releases only this user's verified presets; desired flags survive.
    let _ = controller.release_owned().await;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub async fn run_agent(_disable_all: bool) -> AppResult<()> {
    Err(AppError::unsupported_platform())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &Path) -> AppPaths {
        AppPaths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            state_dir: root.join("state"),
            runtime_dir: root.join("runtime"),
        }
    }

    fn mouse() -> InputDevice {
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
