#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::models::{AppError, AppResult, ErrorCode, HrtfStatus};
use crate::profiles::{
    atomic_write, private_dir, read_limited, validate_hrtf_reference, validate_node_name,
    write_new_private, AppPaths, APP_NODE_PREFIX, SYSTEM_HRTF,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(crate) const MAX_SOFA_BYTES: usize = 64 * 1024 * 1024;
const RESTORE_FILE: &str = "audio-restore.json";
const SYSTEM_SOFA_PATH: &str = "/usr/share/libmysofa/default.sofa";

#[derive(Debug, Clone)]
pub(crate) struct HrtfSelection {
    pub path: PathBuf,
    pub status: HrtfStatus,
}

impl HrtfSelection {
    pub fn readable(&self) -> bool {
        if !fs::metadata(&self.path).is_ok_and(|metadata| metadata.is_file()) {
            return false;
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        options.open(&self.path).is_ok_and(|file| {
            file.metadata().is_ok_and(|metadata| {
                metadata.is_file() && (128..=MAX_SOFA_BYTES as u64).contains(&metadata.len())
            })
        })
    }
}

fn hrtf_error() -> AppError {
    AppError::new(
        ErrorCode::HrtfUnavailable,
        "HRTF SOFA non disponibile o non valido. Installare libmysofa o importare un file .sofa valido.",
    )
}

fn validate_sofa_container(bytes: &[u8]) -> AppResult<()> {
    let signature = b"\x89HDF\r\n\x1a\n";
    let valid_signature = bytes.starts_with(signature)
        || (9..=20).any(|power| {
            bytes
                .get((1 << power)..)
                .is_some_and(|data| data.starts_with(signature))
        });
    // This is an envelope check, not a substitute for libmysofa's semantic check.
    if bytes.len() < 128
        || bytes.len() > MAX_SOFA_BYTES
        || !valid_signature
        || !bytes
            .windows(b"SOFAConventions".len())
            .any(|word| word == b"SOFAConventions")
        || !bytes.windows(b"SOFA".len()).any(|word| word == b"SOFA")
    {
        return Err(
            hrtf_error().detail("Firma HDF5/SOFA mancante o file troppo grande (massimo 64 MiB).")
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_native_sofa(path: &Path) -> AppResult<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    struct Library(*mut libc::c_void);
    impl Drop for Library {
        fn drop(&mut self) {
            unsafe { libc::dlclose(self.0) };
        }
    }

    let filename = CString::new(path.as_os_str().as_bytes()).map_err(|_| hrtf_error())?;
    // Use the system dependency already required by PipeWire's SOFA plugin.
    let handle = unsafe {
        let first = libc::dlopen(
            c"libmysofa.so.1".as_ptr(),
            libc::RTLD_NOW | libc::RTLD_LOCAL,
        );
        if first.is_null() {
            libc::dlopen(c"libmysofa.so".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL)
        } else {
            first
        }
    };
    if handle.is_null() {
        return Err(hrtf_error().detail("La libreria di sistema libmysofa non e' installata."));
    }
    let library = Library(handle);
    type Load = unsafe extern "C" fn(*const libc::c_char, *mut libc::c_int) -> *mut libc::c_void;
    type Check = unsafe extern "C" fn(*mut libc::c_void) -> libc::c_int;
    type Free = unsafe extern "C" fn(*mut libc::c_void);
    let (load, check, free) = unsafe {
        (
            libc::dlsym(library.0, c"mysofa_load".as_ptr()),
            libc::dlsym(library.0, c"mysofa_check".as_ptr()),
            libc::dlsym(library.0, c"mysofa_free".as_ptr()),
        )
    };
    if load.is_null() || check.is_null() || free.is_null() {
        return Err(hrtf_error().detail("API libmysofa non disponibile."));
    }
    let load: Load = unsafe { std::mem::transmute(load) };
    let check: Check = unsafe { std::mem::transmute(check) };
    let free: Free = unsafe { std::mem::transmute(free) };
    let mut error = 0;
    let hrtf = unsafe { load(filename.as_ptr(), &mut error) };
    if hrtf.is_null() {
        return Err(hrtf_error().detail(format!("mysofa_load: {error}")));
    }
    let validation = unsafe { check(hrtf) };
    unsafe { free(hrtf) };
    if validation != 0 {
        return Err(hrtf_error().detail(format!("mysofa_check: {validation}")));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn validate_native_sofa(_path: &Path) -> AppResult<()> {
    Err(AppError::unsupported_platform())
}

pub(crate) fn resolve_hrtf(paths: &AppPaths, reference: &str) -> AppResult<HrtfSelection> {
    validate_hrtf_reference(reference)?;
    let path = if reference == SYSTEM_HRTF {
        PathBuf::from(SYSTEM_SOFA_PATH)
    } else {
        let root = paths.data_dir.join("hrtf");
        let path = root.join(reference);
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| hrtf_error().detail(error.to_string()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(hrtf_error().detail("Il riferimento importato non deve essere un link."));
        }
        let canonical_root =
            fs::canonicalize(root).map_err(|error| hrtf_error().detail(error.to_string()))?;
        let canonical_path =
            fs::canonicalize(&path).map_err(|error| hrtf_error().detail(error.to_string()))?;
        if canonical_path.parent() != Some(canonical_root.as_path()) {
            return Err(
                hrtf_error().detail("Il file HRTF deve restare nella cartella dati dell'app.")
            );
        }
        canonical_path
    };
    let bytes = read_limited(&path, MAX_SOFA_BYTES)
        .map_err(|error| hrtf_error().detail(error.to_string()))?;
    validate_sofa_container(&bytes)?;
    validate_native_sofa(&path)?;
    Ok(HrtfSelection {
        path,
        status: HrtfStatus {
            reference: reference.into(),
            display_name: if reference == SYSTEM_HRTF {
                "HRTF di sistema".into()
            } else {
                reference.into()
            },
            readable: true,
        },
    })
}

pub(crate) fn import_hrtf(paths: &AppPaths, source: &Path) -> AppResult<HrtfSelection> {
    if !source.is_absolute()
        || !source
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("sofa"))
    {
        return Err(hrtf_error()
            .detail("Selezionare un file .sofa dal dialogo, non una configurazione PipeWire."));
    }
    let bytes = read_limited(source, MAX_SOFA_BYTES)
        .map_err(|error| hrtf_error().detail(error.to_string()))?;
    validate_sofa_container(&bytes)?;
    let root = paths.data_dir.join("hrtf");
    private_dir(&root)?;
    let reference = format!("{}.sofa", Uuid::new_v4());
    let destination = root.join(&reference);
    let result = (|| {
        write_new_private(&destination, &bytes)?;
        // Validate the controlled copy, so a changed source cannot race the import.
        let mut selection = resolve_hrtf(paths, &reference)?;
        selection.status.display_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("HRTF importato")
            .chars()
            .filter(|character| !character.is_control())
            .take(120)
            .collect();
        Ok(selection)
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

pub(crate) fn render_config(
    target: &str,
    hrtf_path: &Path,
    input_name: &str,
    playback_name: &str,
    session: &str,
) -> AppResult<String> {
    validate_node_name(target)?;
    validate_node_name(input_name)?;
    validate_node_name(playback_name)?;
    let path = hrtf_path.to_str().ok_or_else(hrtf_error)?;
    let encoded = |value: &str| {
        serde_json::to_string(value).map_err(|error| {
            AppError::new(ErrorCode::InvalidData, "Configurazione non valida.")
                .detail(error.to_string())
        })
    };
    let values = [
        ("TARGET_OBJECT", encoded(target)?),
        ("SOFA_PATH", encoded(path)?),
        ("INPUT_NAME", encoded(input_name)?),
        ("PLAYBACK_NAME", encoded(playback_name)?),
        ("SESSION", encoded(session)?),
    ];
    let template = include_str!("../resources/spatializer-7.1.conf.template");
    let mut sections = template.split("{{");
    let mut config = sections.next().unwrap_or_default().to_owned();
    // Single-pass expansion: inserted user data cannot become another placeholder.
    for section in sections {
        let (key, remainder) = section.split_once("}}").ok_or_else(|| {
            AppError::new(ErrorCode::InvalidData, "Template surround incompleto.")
        })?;
        let value = values
            .iter()
            .find(|(name, _)| *name == key)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::InvalidData,
                    "Campo del template surround sconosciuto.",
                )
            })?;
        config.push_str(&value.1);
        config.push_str(remainder);
    }
    Ok(config)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoreRecord {
    pub schema_version: u32,
    pub session_id: String,
    pub previous_configured_default: Option<String>,
    pub previous_effective_default: Option<String>,
    pub selected_physical_target: String,
    pub target_serial: u64,
    pub virtual_node_name: String,
    pub playback_node_name: String,
    pub app_process_id: u32,
    pub filter_process_id: Option<u32>,
    pub virtual_object_serial: Option<u64>,
    pub playback_object_serial: Option<u64>,
    pub core_cookie: Option<u64>,
}

impl RestoreRecord {
    pub fn validate(&self) -> AppResult<()> {
        let uuid = Uuid::parse_str(&self.session_id).map_err(|_| invalid_restore())?;
        if self.schema_version != 1
            || uuid.to_string() != self.session_id
            || self.virtual_node_name != format!("{APP_NODE_PREFIX}input.{}", self.session_id)
            || self.playback_node_name != format!("{APP_NODE_PREFIX}playback.{}", self.session_id)
            || self.selected_physical_target.starts_with(APP_NODE_PREFIX)
            || self.app_process_id == 0
        {
            return Err(invalid_restore());
        }
        validate_node_name(&self.selected_physical_target)?;
        for name in [
            &self.previous_configured_default,
            &self.previous_effective_default,
        ]
        .into_iter()
        .flatten()
        {
            validate_node_name(name)?;
            if name.starts_with(APP_NODE_PREFIX) {
                return Err(invalid_restore());
            }
        }
        Ok(())
    }
}

fn invalid_restore() -> AppError {
    AppError::new(
        ErrorCode::InvalidData,
        "Stato di ripristino non valido: conservato, attivazione surround bloccata.",
    )
}

pub(crate) fn read_restore(paths: &AppPaths) -> AppResult<Option<RestoreRecord>> {
    let bytes = match read_limited(&paths.state_dir.join(RESTORE_FILE), 16 * 1024) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(invalid_restore().detail(error.to_string())),
    };
    let record: RestoreRecord = serde_json::from_slice(&bytes)
        .map_err(|error| invalid_restore().detail(error.to_string()))?;
    record.validate()?;
    Ok(Some(record))
}

pub(crate) fn write_restore(paths: &AppPaths, record: &RestoreRecord) -> AppResult<()> {
    record.validate()?;
    // Never replace a corrupt/foreign-version recovery record.
    if read_restore(paths)?.is_some_and(|existing| existing.session_id != record.session_id) {
        return Err(AppError::new(
            ErrorCode::Busy,
            "Una sessione surround precedente deve essere recuperata prima dell'attivazione.",
        ));
    }
    let bytes =
        serde_json::to_vec(record).map_err(|error| invalid_restore().detail(error.to_string()))?;
    atomic_write(&paths.state_dir.join(RESTORE_FILE), &bytes)
}

pub(crate) fn clear_restore(paths: &AppPaths, session: &str) -> AppResult<()> {
    if read_restore(paths)?.is_some_and(|record| record.session_id == session) {
        fs::remove_file(paths.state_dir.join(RESTORE_FILE))?;
        #[cfg(unix)]
        fs::File::open(&paths.state_dir)?.sync_all()?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreIntent {
    Bypass,
    Rollback,
    TargetLost,
    Recovery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RestoreAction {
    KeepExternal,
    SetOutput(String),
    ClearOutputDefault,
}

pub(crate) fn restore_decision(
    record: &RestoreRecord,
    configured: Option<&str>,
    target_present: bool,
    previous_present: bool,
    safe_physical_fallback: Option<&str>,
    intent: RestoreIntent,
) -> RestoreAction {
    if configured != Some(record.virtual_node_name.as_str()) {
        return RestoreAction::KeepExternal;
    }
    if intent == RestoreIntent::Rollback {
        return match &record.previous_configured_default {
            Some(name) if previous_present => RestoreAction::SetOutput(name.clone()),
            _ => RestoreAction::ClearOutputDefault,
        };
    }
    if record.previous_configured_default.is_none() && intent != RestoreIntent::TargetLost {
        return RestoreAction::ClearOutputDefault;
    }
    if intent == RestoreIntent::TargetLost || !target_present {
        return match safe_physical_fallback {
            Some(name) => RestoreAction::SetOutput(name.into()),
            None => RestoreAction::ClearOutputDefault,
        };
    }
    RestoreAction::SetOutput(record.selected_physical_target.clone())
}

#[cfg(target_os = "linux")]
pub(crate) mod runtime {
    use super::*;
    use crate::models::AudioNode;
    use crate::process;
    use std::collections::VecDeque;
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::process::{Child, ChildStdout, Command};
    use tokio::task::JoinHandle;

    const STDERR_BYTES: usize = 8192;

    pub(crate) struct OwnedChild {
        child: Option<Child>,
        stderr: Arc<Mutex<VecDeque<u8>>>,
        stderr_reader: Option<JoinHandle<()>>,
    }

    impl OwnedChild {
        pub fn spawn(mut command: Command) -> AppResult<Self> {
            let mut child = command.spawn().map_err(|error| {
                let code = match error.kind() {
                    std::io::ErrorKind::NotFound => ErrorCode::ProviderMissing,
                    std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
                    _ => ErrorCode::Io,
                };
                AppError::new(code, "Impossibile avviare il client PipeWire.")
                    .detail(error.to_string())
            })?;
            let stderr = Arc::new(Mutex::new(VecDeque::new()));
            let retained = stderr.clone();
            let stderr_reader = child.stderr.take().map(|mut reader| {
                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    while let Ok(length) = reader.read(&mut buffer).await {
                        if length == 0 {
                            break;
                        }
                        if let Ok(mut tail) = retained.lock() {
                            for byte in &buffer[..length] {
                                if tail.len() == STDERR_BYTES {
                                    tail.pop_front();
                                }
                                tail.push_back(*byte);
                            }
                        }
                    }
                })
            });
            Ok(Self {
                child: Some(child),
                stderr,
                stderr_reader,
            })
        }

        pub fn id(&self) -> Option<u32> {
            self.child.as_ref().and_then(Child::id)
        }

        pub fn stdout(&mut self) -> Option<ChildStdout> {
            self.child.as_mut().and_then(|child| child.stdout.take())
        }

        pub fn has_exited(&mut self) -> AppResult<bool> {
            match self.child.as_mut() {
                Some(child) => Ok(child.try_wait()?.is_some()),
                None => Ok(true),
            }
        }

        pub fn stderr(&self) -> String {
            self.stderr
                .lock()
                .map(|tail| {
                    String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<_>>()).into_owned()
                })
                .unwrap_or_default()
        }

        pub async fn terminate(&mut self) -> AppResult<()> {
            let result = if let Some(mut child) = self.child.take() {
                let _ = child.start_kill();
                match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
                    Ok(result) => result.map(|_| ()).map_err(AppError::from),
                    Err(_) => {
                        tokio::spawn(async move {
                            let _ = child.wait().await;
                        });
                        Err(AppError::new(
                            ErrorCode::Timeout,
                            "Attesa di chiusura del client PipeWire scaduta.",
                        ))
                    }
                }
            } else {
                Ok(())
            };
            if let Some(task) = self.stderr_reader.take() {
                task.abort();
                let _ = task.await;
            }
            result
        }
    }

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if let Some(task) = self.stderr_reader.take() {
                task.abort();
            }
            if let Some(mut child) = self.child.take() {
                let _ = child.start_kill();
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        let _ = child.wait().await;
                    });
                }
                // Without a live runtime, Tokio's kill_on_drop/reaper remains the backstop.
            }
        }
    }

    pub(crate) struct ActiveFilter {
        pub process: OwnedChild,
        pub record: RestoreRecord,
        pub directory: PathBuf,
        pub hrtf_reference: String,
    }

    impl ActiveFilter {
        pub async fn spawn(
            paths: &AppPaths,
            target: &AudioNode,
            hrtf: &HrtfSelection,
            previous_configured_default: Option<String>,
            previous_effective_default: Option<String>,
            core_cookie: Option<u64>,
        ) -> AppResult<Self> {
            let serial = target.object_serial.ok_or_else(|| {
                AppError::new(
                    ErrorCode::TargetUnavailable,
                    "Il target non espone un'identita PipeWire stabile.",
                )
            })?;
            let session_id = Uuid::new_v4().to_string();
            let mut record = RestoreRecord {
                schema_version: 1,
                virtual_node_name: format!("{APP_NODE_PREFIX}input.{session_id}"),
                playback_node_name: format!("{APP_NODE_PREFIX}playback.{session_id}"),
                session_id,
                previous_configured_default,
                previous_effective_default,
                selected_physical_target: target.node_name.clone(),
                target_serial: serial,
                app_process_id: std::process::id(),
                filter_process_id: None,
                virtual_object_serial: None,
                playback_object_serial: None,
                core_cookie,
            };
            private_dir(&paths.runtime_dir)?;
            let directory = paths.runtime_dir.join(&record.session_id);
            private_dir(&directory)?;
            let config_path = directory.join("spatializer.conf");
            let result = (|| {
                let config = render_config(
                    &target.node_name,
                    &hrtf.path,
                    &record.virtual_node_name,
                    &record.playback_node_name,
                    &record.session_id,
                )?;
                write_new_private(&config_path, config.as_bytes())?;
                write_restore(paths, &record)?;
                let mut command = process::owned_command("pipewire")?;
                command.arg("-c").arg(&config_path).stdout(Stdio::null());
                command
                    .env("PIPEWIRE_CONFIG_DIR", &directory)
                    .env("PIPEWIRE_CONFIG_PREFIX", "")
                    .env("PIPEWIRE_CONFIG_NAME", &config_path)
                    .env("PIPEWIRE_DAEMON", "false")
                    .env_remove("PIPEWIRE_NO_CONFIG")
                    .env_remove("PIPEWIRE_PROPS")
                    .env_remove("PIPEWIRE_NODE")
                    .env_remove("PIPEWIRE_AUTOCONNECT");
                let process = OwnedChild::spawn(command)?;
                record.filter_process_id = process.id();
                write_restore(paths, &record)?;
                Ok(Self {
                    process,
                    record: record.clone(),
                    directory: directory.clone(),
                    hrtf_reference: hrtf.status.reference.clone(),
                })
            })();
            if result.is_err() {
                let _ = fs::remove_file(config_path);
                let _ = fs::remove_dir(&directory);
                let _ = clear_restore(paths, &record.session_id);
            }
            result
        }

        pub async fn stop(&mut self) -> AppResult<()> {
            let result = self.process.terminate().await;
            let _ = fs::remove_file(self.directory.join("spatializer.conf"));
            let _ = fs::remove_dir(&self.directory);
            result
        }
    }

    impl Drop for ActiveFilter {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.directory.join("spatializer.conf"));
            let _ = fs::remove_dir(&self.directory);
            // Keep the independent restore record on abnormal drop/crash.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn template_is_isolated_escaped_eight_to_two_and_conservative() {
        let target = "alsa_output.\"\\{{SESSION}}.usb";
        let path = Path::new("/data/quoted \"hrtf\"\\sample.sofa");
        let config = render_config(target, path, "input.test", "playback.test", "session").unwrap();
        let json_start = config.find("\n{\n").unwrap();
        let config: Value = serde_json::from_str(&config[json_start..]).unwrap();
        assert_eq!(config["context.properties"]["core.daemon"], false);
        let modules = config["context.modules"].as_array().unwrap();
        for required in ["protocol-native", "client-node", "adapter"] {
            assert!(modules
                .iter()
                .any(|module| module["name"] == format!("libpipewire-module-{required}")));
        }
        let filter = &modules.last().unwrap()["args"];
        assert!(modules.last().unwrap().get("flags").is_none());
        let graph = &filter["filter.graph"];
        let nodes = graph["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 10);
        assert_eq!(
            nodes.iter().filter(|node| node["type"] == "sofa").count(),
            8
        );
        for node in nodes.iter().filter(|node| node["type"] == "sofa") {
            assert_eq!(node["config"]["filename"], path.to_str().unwrap());
        }
        for mixer in nodes.iter().filter(|node| node["label"] == "mixer") {
            assert_eq!(mixer["control"].as_object().unwrap().len(), 8);
            for channel in 1..=8 {
                assert_eq!(mixer["control"][format!("Gain {channel}")], 0.125);
            }
        }
        assert_eq!(graph["links"].as_array().unwrap().len(), 16);
        assert_eq!(graph["inputs"].as_array().unwrap().len(), 8);
        assert_eq!(graph["outputs"], json!(["mixL:Out", "mixR:Out"]));
        assert_eq!(
            filter["capture.props"]["audio.position"],
            json!(["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"])
        );
        let playback = &filter["playback.props"];
        assert_eq!(playback["target.object"], target);
        for property in [
            "node.passive",
            "node.dont-fallback",
            "node.dont-reconnect",
            "stream.dont-remix",
        ] {
            assert_eq!(playback[property], true);
        }
        assert_eq!(playback["audio.position"], json!(["FL", "FR"]));
        assert_eq!(config["context.exec"], json!([]));
    }

    fn record() -> RestoreRecord {
        let id = "4ccf058b-1487-4cda-9b5a-11a6033a5ad0";
        RestoreRecord {
            schema_version: 1,
            session_id: id.into(),
            previous_configured_default: Some("old.headset".into()),
            previous_effective_default: Some("old.headset".into()),
            selected_physical_target: "chosen.headset".into(),
            target_serial: 50,
            virtual_node_name: format!("{APP_NODE_PREFIX}input.{id}"),
            playback_node_name: format!("{APP_NODE_PREFIX}playback.{id}"),
            app_process_id: 2,
            filter_process_id: Some(3),
            virtual_object_serial: Some(51),
            playback_object_serial: Some(52),
            core_cookie: Some(1),
        }
    }

    #[test]
    fn restore_respects_external_choices_automatic_defaults_and_unplug() {
        let mut record = record();
        for external in [Some("external.choice"), None] {
            assert_eq!(
                restore_decision(
                    &record,
                    external,
                    true,
                    true,
                    Some("speakers"),
                    RestoreIntent::Bypass
                ),
                RestoreAction::KeepExternal
            );
        }
        let owned = Some(record.virtual_node_name.as_str());
        assert_eq!(
            restore_decision(
                &record,
                owned,
                true,
                true,
                Some("speakers"),
                RestoreIntent::Bypass
            ),
            RestoreAction::SetOutput("chosen.headset".into())
        );
        assert_eq!(
            restore_decision(
                &record,
                owned,
                true,
                true,
                Some("speakers"),
                RestoreIntent::Rollback
            ),
            RestoreAction::SetOutput("old.headset".into())
        );
        assert_eq!(
            restore_decision(
                &record,
                owned,
                false,
                false,
                Some("speakers"),
                RestoreIntent::TargetLost
            ),
            RestoreAction::SetOutput("speakers".into())
        );
        assert_eq!(
            restore_decision(&record, owned, false, false, None, RestoreIntent::Recovery),
            RestoreAction::ClearOutputDefault
        );
        record.previous_configured_default = None;
        assert_eq!(
            restore_decision(
                &record,
                Some(&record.virtual_node_name),
                true,
                false,
                None,
                RestoreIntent::Bypass
            ),
            RestoreAction::ClearOutputDefault
        );
        assert_eq!(
            restore_decision(
                &record,
                Some(&record.virtual_node_name),
                true,
                false,
                None,
                RestoreIntent::Recovery
            ),
            RestoreAction::ClearOutputDefault
        );
    }

    #[test]
    fn invalid_sofa_and_arbitrary_config_references_fail() {
        assert!(validate_sofa_container(b"not a sofa file").is_err());
        let mut bytes = vec![0; 256];
        bytes[..8].copy_from_slice(b"\x89HDF\r\n\x1a\n");
        assert!(validate_sofa_container(&bytes).is_err());
        bytes[32..47].copy_from_slice(b"SOFAConventions");
        assert!(validate_sofa_container(&bytes).is_ok());
        let mut record = record();
        record.virtual_node_name = "external.default".into();
        assert!(record.validate().is_err());
        assert!(validate_hrtf_reference("../../pipewire.conf").is_err());
    }

    #[test]
    fn recovery_record_is_separate_and_unknown_versions_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            config_dir: directory.path().join("config"),
            data_dir: directory.path().join("data"),
            state_dir: directory.path().join("state"),
            runtime_dir: directory.path().join("runtime"),
        };
        let record = record();
        write_restore(&paths, &record).unwrap();
        assert_eq!(
            read_restore(&paths).unwrap().unwrap().session_id,
            record.session_id
        );
        assert!(!paths.config_dir.exists());
        let file = paths.state_dir.join(RESTORE_FILE);
        let bytes = b"{\"schemaVersion\":999}";
        fs::write(&file, bytes).unwrap();
        assert!(write_restore(&paths, &record).is_err());
        assert_eq!(fs::read(file).unwrap(), bytes);
    }
}
