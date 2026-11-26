use crate::models::{AppError, AppResult, AudioProfile, ErrorCode};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_PROFILE_BYTES: usize = 1024 * 1024;
const MAX_PROFILES: usize = 128;
pub(crate) const SYSTEM_HRTF: &str = "system";
pub(crate) const APP_NODE_PREFIX: &str = "peripheral-hub.surround.";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

impl AppPaths {
    fn discover() -> AppResult<Self> {
        let base = BaseDirs::new()
            .ok_or_else(|| AppError::new(ErrorCode::Io, "Cartella personale non disponibile."))?;
        let xdg = |key: &str, fallback: PathBuf| -> AppResult<PathBuf> {
            match std::env::var_os(key) {
                Some(value) if !value.is_empty() => {
                    let path = PathBuf::from(value);
                    if !path.is_absolute() {
                        return Err(AppError::new(
                            ErrorCode::Validation,
                            format!("{key} deve essere un percorso assoluto."),
                        ));
                    }
                    Ok(path)
                }
                _ => Ok(fallback),
            }
        };
        let config_dir =
            xdg("XDG_CONFIG_HOME", base.home_dir().join(".config"))?.join("peripheral-hub");
        let data_dir =
            xdg("XDG_DATA_HOME", base.home_dir().join(".local/share"))?.join("peripheral-hub");
        let state_dir =
            xdg("XDG_STATE_HOME", base.home_dir().join(".local/state"))?.join("peripheral-hub");
        let runtime_dir = match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(value) if !value.is_empty() => {
                let root = PathBuf::from(value);
                if !root.is_absolute() {
                    return Err(AppError::new(
                        ErrorCode::Validation,
                        "XDG_RUNTIME_DIR deve essere un percorso assoluto.",
                    ));
                }
                root.join("peripheral-hub")
            }
            _ => state_dir.join("runtime"),
        };
        Ok(Self {
            config_dir,
            data_dir,
            state_dir,
            runtime_dir,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileDocument {
    schema_version: u32,
    profiles: Vec<AudioProfile>,
}

pub struct ProfileStore {
    paths: AppPaths,
    blocked: Option<AppError>,
}

impl ProfileStore {
    pub fn new() -> AppResult<Self> {
        Ok(Self::at(AppPaths::discover()?))
    }

    pub fn at(paths: AppPaths) -> Self {
        Self {
            paths,
            blocked: None,
        }
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn load(&mut self) -> AppResult<Vec<AudioProfile>> {
        let path = self.paths.config_dir.join("profiles.json");
        let result = match read_limited(&path, MAX_PROFILE_BYTES) {
            Ok(bytes) => parse_profiles(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A failed load must not silently turn into an empty writable store.
                if let Some(error) = &self.blocked {
                    return Err(error.clone());
                }
                return Ok(Vec::new());
            }
            Err(error) => Err(AppError::from(error)),
        };
        match result {
            Ok(profiles) => {
                self.blocked = None;
                Ok(profiles)
            }
            Err(error) => {
                self.blocked = Some(error.clone());
                Err(error)
            }
        }
    }

    pub fn save(&mut self, profile: AudioProfile) -> AppResult<Vec<AudioProfile>> {
        validate_profile(&profile)?;
        // Reload before every write, including the first write, to preserve foreign data.
        let mut profiles = self.load()?;
        match profiles.iter().position(|saved| saved.id == profile.id) {
            Some(index) => profiles[index] = profile,
            None if profiles.len() < MAX_PROFILES => profiles.push(profile),
            None => {
                return Err(AppError::new(
                    ErrorCode::Validation,
                    "Numero massimo di profili raggiunto (128).",
                ));
            }
        }
        let bytes = serde_json::to_vec_pretty(&ProfileDocument {
            schema_version: 1,
            profiles: profiles.clone(),
        })
        .map_err(|error| invalid_profiles().detail(error.to_string()))?;
        if bytes.len() > MAX_PROFILE_BYTES {
            return Err(invalid_profiles());
        }
        atomic_write(&self.paths.config_dir.join("profiles.json"), &bytes)?;
        Ok(profiles)
    }
}

fn invalid_profiles() -> AppError {
    AppError::new(
        ErrorCode::InvalidData,
        "Profili non validi: il file originale viene conservato e non sovrascritto.",
    )
}

fn parse_profiles(bytes: &[u8]) -> AppResult<Vec<AudioProfile>> {
    if bytes.len() > MAX_PROFILE_BYTES {
        return Err(invalid_profiles());
    }
    let document: ProfileDocument = serde_json::from_slice(bytes)
        .map_err(|error| invalid_profiles().detail(error.to_string()))?;
    if document.schema_version != 1 {
        return Err(invalid_profiles().detail(format!(
            "schemaVersion {} non supportata; nessuna migrazione automatica.",
            document.schema_version
        )));
    }
    if document.profiles.len() > MAX_PROFILES {
        return Err(invalid_profiles());
    }
    let mut ids = HashSet::new();
    for profile in &document.profiles {
        validate_profile(profile).map_err(|error| invalid_profiles().detail(error.message))?;
        if !ids.insert(&profile.id) {
            return Err(invalid_profiles().detail("UUID duplicato."));
        }
    }
    Ok(document.profiles)
}

pub(crate) fn validate_node_name(name: &str) -> AppResult<()> {
    if name.is_empty() || name.len() > 512 || name.chars().any(char::is_control) {
        return Err(AppError::new(
            ErrorCode::Validation,
            "Nome del nodo audio non valido.",
        ));
    }
    Ok(())
}

pub(crate) fn validate_hrtf_reference(reference: &str) -> AppResult<()> {
    if reference == SYSTEM_HRTF {
        return Ok(());
    }
    let valid = reference
        .strip_suffix(".sofa")
        .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id));
    if !valid {
        return Err(AppError::new(
            ErrorCode::Validation,
            "Riferimento HRTF non valido: sono ammessi solo system o UUID.sofa importati.",
        ));
    }
    Ok(())
}

pub(crate) fn validate_profile(profile: &AudioProfile) -> AppResult<()> {
    let valid_id = Uuid::parse_str(&profile.id).is_ok_and(|uuid| uuid.to_string() == profile.id);
    if profile.schema_version != 1
        || !valid_id
        || profile.name.is_empty()
        || profile.name.len() > 120
        || profile.name.trim() != profile.name
        || profile.name.chars().any(char::is_control)
        || !profile.volume.is_finite()
        || !(0.0..=1.0).contains(&profile.volume)
        || profile.output_node_name.starts_with(APP_NODE_PREFIX)
    {
        return Err(AppError::new(
            ErrorCode::Validation,
            "Profilo acustico non valido.",
        ));
    }
    validate_node_name(&profile.output_node_name)?;
    if let Some(eq) = &profile.equalizer {
        eq.validate()?;
    }
    if let Some(reference) = &profile.hrtf {
        validate_hrtf_reference(reference)?;
    } else if profile.surround_requested {
        return Err(AppError::new(
            ErrorCode::Validation,
            "Un profilo 7.1 deve indicare un HRTF disponibile.",
        ));
    }
    Ok(())
}

pub(crate) fn read_limited(path: &Path, maximum: usize) -> std::io::Result<Vec<u8>> {
    if !fs::metadata(path)?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Il percorso non e' un file regolare.",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "File non regolare o troppo grande.",
        ));
    }
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(maximum));
    file.take(maximum as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "File troppo grande.",
        ));
    }
    Ok(bytes)
}

pub(crate) fn private_dir(path: &Path) -> AppResult<()> {
    if !path.is_absolute() {
        return Err(AppError::new(
            ErrorCode::Validation,
            "Cartella privata non assoluta.",
        ));
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(AppError::new(
            ErrorCode::PermissionDenied,
            "La cartella privata non deve essere un collegamento simbolico.",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // Only app-owned subdirectories are passed here, never an XDG root.
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(AppError::new(
                ErrorCode::PermissionDenied,
                "Cartella di un altro utente.",
            ));
        }
        if metadata.mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

pub(crate) fn write_new_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new(ErrorCode::Validation, "Destinazione del file non valida."))?;
    private_dir(parent)?;
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(AppError::new(
            ErrorCode::PermissionDenied,
            "Il file non deve essere un link.",
        ));
    }
    let temporary = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| {
        write_new_private(&temporary, bytes)?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> AudioProfile {
        AudioProfile {
            schema_version: 1,
            id: "1f8bfe67-6807-4be7-a4e3-54c1d3bbf002".into(),
            name: "Cuffie".into(),
            output_node_name: "alsa_output.usb-headset.stereo".into(),
            hrtf: Some(SYSTEM_HRTF.into()),
            volume: 0.4,
            mute: false,
            surround_requested: true,
            equalizer: None,
        }
    }

    fn paths(root: &Path) -> AppPaths {
        AppPaths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            state_dir: root.join("state"),
            runtime_dir: root.join("runtime"),
        }
    }

    #[test]
    fn validates_schema_volume_and_controlled_references() {
        let mut value = profile();
        assert!(validate_profile(&value).is_ok());
        for reference in ["../hrtf.sofa", "/tmp/arbitrary.sofa", "test.conf", "SYSTEM"] {
            value.hrtf = Some(reference.into());
            assert!(validate_profile(&value).is_err());
        }
        value.hrtf = Some(format!("{}.sofa", Uuid::new_v4()));
        assert!(validate_profile(&value).is_ok());
        for volume in [f64::NAN, f64::INFINITY, -0.1, 1.001] {
            value.volume = volume;
            assert!(validate_profile(&value).is_err());
        }
        value.volume = 0.5;
        value.schema_version = 2;
        assert!(validate_profile(&value).is_err());
    }

    #[test]
    fn initial_load_is_read_only_and_save_is_atomic() {
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let mut store = ProfileStore::at(app_paths.clone());
        assert!(store.load().unwrap().is_empty());
        assert!(!app_paths.config_dir.exists());
        assert_eq!(store.save(profile()).unwrap().len(), 1);
        let mut changed = profile();
        changed.volume = 0.2;
        assert_eq!(store.save(changed.clone()).unwrap(), vec![changed.clone()]);
        assert_eq!(store.load().unwrap(), vec![changed]);
        assert_eq!(fs::read_dir(&app_paths.config_dir).unwrap().count(), 1);
        assert!(!app_paths.state_dir.exists());
    }

    #[test]
    fn corrupt_and_unknown_documents_are_not_overwritten() {
        for bytes in [
            include_bytes!("../tests/profiles_corrupt.json").as_slice(),
            include_bytes!("../tests/profiles_unknown_version.json").as_slice(),
            b"{\"schemaVersion\":1,\"profiles\":[],\"foreign\":true}".as_slice(),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let app_paths = paths(directory.path());
            fs::create_dir(&app_paths.config_dir).unwrap();
            let file = app_paths.config_dir.join("profiles.json");
            fs::write(&file, bytes).unwrap();
            let mut store = ProfileStore::at(app_paths);
            assert!(store.load().is_err());
            assert!(store.save(profile()).is_err());
            assert_eq!(fs::read(file).unwrap(), bytes);
        }
    }

    #[test]
    fn duplicate_ids_and_unknown_profile_fields_are_rejected() {
        let value = profile();
        let bytes = serde_json::to_vec(&ProfileDocument {
            schema_version: 1,
            profiles: vec![value.clone(), value],
        })
        .unwrap();
        assert!(parse_profiles(&bytes).is_err());
        let mut json = serde_json::to_value(profile()).unwrap();
        json["runtimeId"] = serde_json::json!(42);
        assert!(serde_json::from_value::<AudioProfile>(json).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn config_state_and_runtime_are_private_and_symlinks_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        for path in [
            &app_paths.config_dir,
            &app_paths.state_dir,
            &app_paths.runtime_dir,
        ] {
            private_dir(path).unwrap();
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let file = app_paths.config_dir.join("profiles.json");
        symlink(directory.path().join("foreign"), &file).unwrap();
        assert!(atomic_write(&file, b"{}").is_err());
    }
}
