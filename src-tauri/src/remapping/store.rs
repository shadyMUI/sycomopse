use crate::models::{
    AppError, AppResult, ErrorCode, MouseAction, MouseBinding, RemappingIdentityStrength,
};
use crate::profiles::{self, AppPaths};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

use super::types::*;

pub trait FileLockExt {
    fn try_lock_exclusive(&self) -> std::io::Result<()>;
    fn unlock(&self) -> std::io::Result<()>;
}

#[cfg(target_os = "linux")]
impl FileLockExt for File {
    fn try_lock_exclusive(&self) -> std::io::Result<()> {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn unlock(&self) -> std::io::Result<()> {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::flock(self.as_raw_fd(), libc::LOCK_UN) };
        if ret == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl FileLockExt for File {
    fn try_lock_exclusive(&self) -> std::io::Result<()> {
        Ok(())
    }
    fn unlock(&self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct StoreLock(pub File);

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub fn private_file(path: &Path) -> AppResult<File> {
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentLocation {
    pub schema_version: u32,
    pub config_dir: PathBuf,
}

pub fn location_path() -> AppResult<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| AppError::new(ErrorCode::Io, "Home dell'agente non disponibile."))?;
    Ok(base
        .home_dir()
        .join(".local/state/peripheral-hub/remapping-agent.json"))
}

pub fn save_agent_location(paths: &AppPaths) -> AppResult<()> {
    let bytes = serde_json::to_vec(&AgentLocation {
        schema_version: 1,
        config_dir: paths.config_dir.clone(),
    })
    .map_err(|error| {
        invalid("Configurazione agente non serializzabile.").detail(error.to_string())
    })?;
    profiles::atomic_write(&location_path()?, &bytes)
}

pub fn agent_paths() -> AppResult<AppPaths> {
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

pub struct Store {
    pub paths: AppPaths,
    // Keep document I/O injectable without replacing locking or read validation.
    pub write_document: fn(&Path, &[u8]) -> AppResult<()>,
}

impl Store {
    pub fn at(paths: AppPaths) -> Self {
        Self {
            paths,
            write_document: profiles::atomic_write,
        }
    }

    pub fn directory(&self) -> PathBuf {
        self.paths.config_dir.join("remapping")
    }

    pub fn path(&self) -> PathBuf {
        self.paths.config_dir.join("mouse-remapping.json")
    }

    pub fn recovery(&self) -> AppResult<RecoveryDocument> {
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

    pub fn disarm(&self, identities: &[String], disarmed: bool) -> AppResult<()> {
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
        (self.write_document)(
            &self.paths.config_dir.join("remapping-recovery.json"),
            &bytes,
        )
    }

    pub fn lock(&self) -> AppResult<StoreLock> {
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

    pub fn load(&self) -> AppResult<Document> {
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

    pub fn save(&self, document: &mut Document) -> AppResult<()> {
        document.revision = document.revision.saturating_add(1);
        let bytes = serde_json::to_vec_pretty(document).map_err(|error| {
            invalid("Serializzazione profili fallita.").detail(error.to_string())
        })?;
        if bytes.len() > MAX_BYTES || document.profiles.len() > MAX_PROFILES {
            return Err(invalid("Limite profili rimappatura raggiunto."));
        }
        (self.write_document)(&self.path(), &bytes)
    }

    pub async fn acquire_lock(&self) -> AppResult<StoreLock> {
        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || Store::at(paths).lock())
            .await
            .map_err(|error| {
                AppError::new(ErrorCode::Io, "Lock rimappatura non disponibile.")
                    .detail(error.to_string())
            })?
    }

    pub fn preset(&self, profile: &RemappingProfile, device: &InputDevice) -> AppResult<String> {
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

pub fn valid_preset_name(name: &str, uid: u32) -> bool {
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

pub fn unchanged_observation(
    previous: &(String, Option<String>, u64),
    owner: &str,
    preset: &Option<String>,
    generation: u64,
) -> bool {
    previous.0 == owner && previous.1 == *preset && previous.2 == generation
}

pub fn validate_action(action: &MouseAction, codes: Option<&[u16]>) -> AppResult<()> {
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
        MouseAction::Autoclicker { cps, button }
            if (1..=100).contains(cps)
                && (272..=279).contains(button)
                && codes.is_none_or(|values| values.contains(button)) =>
        {
            Ok(())
        }
        _ => Err(invalid(
            "Azione non consentita: usare clic, tasto o scorciatoia senza macro.",
        )),
    }
}

pub fn validate_bindings(
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
