#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use crate::models::{AppError, AppResult, AudioNode, ErrorCode};
use crate::profiles::validate_node_name;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

const WORKER_BINARY: &str = "peripheral-hub-audio-worker";
const MAX_REQUEST_BYTES: usize = 32 * 1024;
const MAX_RESPONSE_BYTES: usize = 8192;
const MAX_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const ADDRESS_SPACE_BYTES: u64 = 512 * 1024 * 1024;
const CPU_SECONDS: u64 = 8;
const DEADLINE: Duration = Duration::from_secs(12);
const CONTROL_DEADLINE: Duration = Duration::from_secs(6);
const REAP_DEADLINE: Duration = Duration::from_secs(2);
const CONTROL_SCRIPT: &str = include_str!("../resources/helper/audio-control.lua");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NodeIdentity {
    id: u32,
    object_serial: String,
    node_name: String,
    media_class: String,
}

impl NodeIdentity {
    fn from_node(node: &AudioNode) -> AppResult<Self> {
        let identity = Self {
            id: node.id,
            object_serial: node
                .object_serial
                .ok_or_else(|| {
                    AppError::new(ErrorCode::TargetUnavailable, "Identita PipeWire assente.")
                })?
                .to_string(),
            node_name: node.node_name.clone(),
            media_class: node.media_class.clone(),
        };
        identity.validate()?;
        Ok(identity)
    }

    fn validate(&self) -> AppResult<()> {
        validate_node_name(&self.node_name)?;
        if self.id == 0
            || self.id == u32::MAX
            || decimal(&self.object_serial, u64::MAX)? == 0
            || !(self.media_class == "Audio/Sink"
                || self.media_class.starts_with("Audio/Sink/")
                || self.media_class == "Audio/Source"
                || self.media_class.starts_with("Audio/Source/"))
            || self.media_class.len() > 128
            || self.media_class.chars().any(char::is_control)
        {
            return Err(invalid_request());
        }
        Ok(())
    }

    fn filter_session(&self) -> AppResult<&str> {
        let session = self
            .node_name
            .strip_prefix(crate::profiles::APP_NODE_PREFIX)
            .and_then(|name| name.strip_prefix("input."))
            .filter(|_| self.media_class == "Audio/Sink")
            .ok_or_else(invalid_request)?;
        let uuid = uuid::Uuid::parse_str(session).map_err(|_| invalid_request())?;
        if uuid.to_string() != session {
            return Err(invalid_request());
        }
        Ok(session)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Operation {
    SetLevel {
        node: NodeIdentity,
        core_cookie: String,
        #[serde(deserialize_with = "required_option")]
        volume: Option<f64>,
        #[serde(deserialize_with = "required_option")]
        muted: Option<bool>,
    },
    SetDefault {
        #[serde(deserialize_with = "required_option")]
        node: Option<NodeIdentity>,
        core_cookie: String,
        #[serde(deserialize_with = "required_option")]
        expected_configured: Option<String>,
        #[serde(deserialize_with = "required_option")]
        expected_effective: Option<String>,
    },
    SetEqualizer {
        node: NodeIdentity,
        core_cookie: String,
        settings: crate::models::EqualizerSettings,
    },
    ValidateSofa {
        // Unix paths need not be UTF-8. These bytes are data, never shell or Lua code.
        path: Vec<u8>,
    },
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    schema_version: u32,
    request: Operation,
}

impl Request {
    fn new(request: Operation) -> AppResult<Self> {
        let request = Self {
            schema_version: 1,
            request,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> AppResult<()> {
        if self.schema_version != 1 {
            return Err(invalid_request());
        }
        match &self.request {
            Operation::SetLevel {
                node,
                core_cookie,
                volume,
                muted,
            } => {
                node.validate()?;
                decimal(core_cookie, u32::MAX as u64)?;
                if (volume.is_none() && muted.is_none())
                    || volume.is_some_and(|v| !(0.0..=10.0).contains(&v))
                {
                    return Err(invalid_request());
                }
                // A handoff may inherit an externally amplified value. This must
                // never extend the volume range on physical nodes or unrelated sinks.
                if volume.is_some_and(|v| v > 1.0) {
                    node.filter_session()?;
                }
            }
            Operation::SetDefault {
                node,
                core_cookie,
                expected_configured,
                expected_effective,
            } => {
                decimal(core_cookie, u32::MAX as u64)?;
                if let Some(node) = node {
                    node.validate()?;
                    if node.media_class != "Audio/Sink" {
                        return Err(invalid_request());
                    }
                }
                for name in expected_configured.iter().chain(expected_effective) {
                    validate_node_name(name)?;
                }
            }
            Operation::ValidateSofa { path } => {
                if path.is_empty()
                    || path.len() > MAX_PATH_BYTES
                    || path[0] != b'/'
                    || path.contains(&0)
                {
                    return Err(invalid_request());
                }
            }
            Operation::SetEqualizer {
                node,
                core_cookie,
                settings,
            } => {
                node.validate()?;
                node.filter_session()?;
                decimal(core_cookie, u32::MAX as u64)?;
                settings.validate()?;
                if !settings.enabled {
                    return Err(invalid_request());
                }
            }
        }
        Ok(())
    }

    fn parse(bytes: &[u8]) -> AppResult<Self> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(invalid_request());
        }
        let request: Self = serde_json::from_slice(bytes)
            .map_err(|error| invalid_request().detail(error.to_string()))?;
        request.validate()?;
        Ok(request)
    }
}

fn invalid_request() -> AppError {
    AppError::new(
        ErrorCode::Validation,
        "Richiesta del worker audio non valida.",
    )
}

fn decimal(value: &str, maximum: u64) -> AppResult<u64> {
    if value.is_empty()
        || value.len() > 20
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(invalid_request());
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|number| *number <= maximum)
        .ok_or_else(invalid_request)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
enum Outcome {
    Confirmed {},
    Error {
        code: ErrorCode,
        message: String,
        detail: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Response {
    schema_version: u32,
    outcome: Outcome,
}

impl Response {
    fn from_result(result: AppResult<()>) -> Self {
        Self {
            schema_version: 1,
            outcome: match result {
                Ok(()) => Outcome::Confirmed {},
                Err(error) => Outcome::Error {
                    code: error.code,
                    message: bounded_text(error.message, 2048),
                    detail: error.detail.map(|detail| bounded_text(detail, 4096)),
                },
            },
        }
    }

    fn parse(bytes: &[u8], success: bool) -> AppResult<()> {
        let invalid = || {
            AppError::new(
                ErrorCode::NotConfirmed,
                "Risposta del worker audio non confermata.",
            )
        };
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(invalid());
        }
        let response: Self =
            serde_json::from_slice(bytes).map_err(|error| invalid().detail(error.to_string()))?;
        if response.schema_version != 1 {
            return Err(invalid());
        }
        match response.outcome {
            Outcome::Confirmed {} if success => Ok(()),
            Outcome::Confirmed {} => Err(invalid()),
            Outcome::Error {
                code,
                message,
                detail,
            } => {
                if message.is_empty()
                    || message.len() > 2048
                    || detail.as_ref().is_some_and(|detail| detail.len() > 4096)
                {
                    return Err(invalid());
                }
                let mut error = AppError::new(code, message);
                if let Some(detail) = detail {
                    error = error.detail(detail);
                }
                Err(error)
            }
        }
    }
}

fn bounded_text(mut text: String, maximum: usize) -> String {
    if text.len() > maximum {
        let mut end = maximum;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

pub async fn set_level(
    node: &AudioNode,
    core_cookie: &str,
    volume: f64,
    muted: bool,
) -> AppResult<()> {
    invoke(Request::new(Operation::SetLevel {
        node: NodeIdentity::from_node(node)?,
        core_cookie: core_cookie.into(),
        volume: Some(volume),
        muted: Some(muted),
    })?)
    .await
}

pub async fn set_volume(node: &AudioNode, core_cookie: &str, volume: f64) -> AppResult<()> {
    invoke(Request::new(Operation::SetLevel {
        node: NodeIdentity::from_node(node)?,
        core_cookie: core_cookie.into(),
        volume: Some(volume),
        muted: None,
    })?)
    .await
}

pub async fn set_mute(node: &AudioNode, core_cookie: &str, muted: bool) -> AppResult<()> {
    invoke(Request::new(Operation::SetLevel {
        node: NodeIdentity::from_node(node)?,
        core_cookie: core_cookie.into(),
        volume: None,
        muted: Some(muted),
    })?)
    .await
}

// For either baseline, None means that metadata key was absent, not "ignore it".
// PipeWire metadata does not offer a server-side CAS.
pub async fn set_default(
    node: Option<&AudioNode>,
    core_cookie: &str,
    expected_configured: Option<&str>,
    expected_effective: Option<&str>,
) -> AppResult<()> {
    invoke(Request::new(Operation::SetDefault {
        node: node.map(NodeIdentity::from_node).transpose()?,
        core_cookie: core_cookie.into(),
        expected_configured: expected_configured.map(str::to_owned),
        expected_effective: expected_effective.map(str::to_owned),
    })?)
    .await
}

pub(crate) async fn set_equalizer(
    node: &AudioNode,
    core_cookie: &str,
    settings: &crate::models::EqualizerSettings,
) -> AppResult<()> {
    invoke(Request::new(Operation::SetEqualizer {
        node: NodeIdentity::from_node(node)?,
        core_cookie: core_cookie.into(),
        settings: settings.clone(),
    })?)
    .await
}

pub async fn validate_sofa(path: &Path) -> AppResult<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        invoke(Request::new(Operation::ValidateSofa {
            path: path.as_os_str().as_bytes().to_vec(),
        })?)
        .await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        Err(AppError::unsupported_platform())
    }
}

#[cfg(not(target_os = "linux"))]
async fn invoke(_request: Request) -> AppResult<()> {
    Err(AppError::unsupported_platform())
}

#[cfg(target_os = "linux")]
async fn invoke(request: Request) -> AppResult<()> {
    use tokio::sync::Semaphore;
    static SLOTS: Semaphore = Semaphore::const_new(2);
    let _slot = tokio::time::timeout(Duration::from_millis(500), SLOTS.acquire())
        .await
        .map_err(|_| AppError::new(ErrorCode::Busy, "Worker audio gia' occupati."))?
        .map_err(|_| AppError::new(ErrorCode::Io, "Worker audio non disponibile."))?;
    let bytes = serde_json::to_vec(&request).map_err(|_| invalid_request())?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(invalid_request());
    }
    let executable = worker_path(&std::env::current_exe()?)?;
    let mut command = limited_command(&executable, None)?;
    command.arg("--stdio");
    let output = capture(command, Some(&bytes), DEADLINE).await?;
    Response::parse(&output.stdout, output.success).map_err(|error| {
        if output.stderr.is_empty() || error.detail.is_some() {
            error
        } else {
            error.detail(output.stderr)
        }
    })
}

#[cfg(target_os = "linux")]
fn worker_path(executable: &Path) -> AppResult<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if executable
        .file_name()
        .is_some_and(|name| name == WORKER_BINARY)
    {
        return Err(AppError::new(
            ErrorCode::Validation,
            "Avvio ricorsivo del worker audio rifiutato.",
        ));
    }
    let directory = executable.parent().ok_or_else(invalid_request)?;
    let mut candidates = vec![directory.join(WORKER_BINARY)];
    #[cfg(debug_assertions)]
    if directory.file_name().is_some_and(|name| name == "deps") {
        if let Some(target) = directory.parent() {
            candidates.push(target.join(WORKER_BINARY));
        }
    }
    for candidate in candidates {
        if std::fs::metadata(&candidate)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            if std::fs::canonicalize(&candidate)? == std::fs::canonicalize(executable)? {
                return Err(invalid_request());
            }
            return Ok(candidate);
        }
    }
    Err(AppError::new(ErrorCode::ProviderMissing,
        "Helper audio non installato. Compilare/installare peripheral-hub-audio-worker accanto all'app."))
}

// Every byte is escaped with exactly three decimal digits. JSON quoting is NOT
// Lua quoting (notably \u escapes); neither names nor paths become Lua syntax.
fn lua_string(value: &str) -> String {
    use std::fmt::Write;
    let mut quoted = String::with_capacity(value.len() * 4 + 2);
    quoted.push('"');
    for byte in value.bytes() {
        let _ = write!(quoted, "\\{byte:03}");
    }
    quoted.push('"');
    quoted
}

fn render_lua(request: &Request) -> AppResult<String> {
    request.validate()?;
    let node_table = |node: &NodeIdentity| {
        format!(
            "{{id={},serial={},name={},class={},session={}}}",
            node.id,
            lua_string(&node.object_serial),
            lua_string(&node.node_name),
            lua_string(&node.media_class),
            node.filter_session()
                .map_or_else(|_| "nil".into(), lua_string),
        )
    };
    let prefix = match &request.request {
        Operation::SetLevel {
            node,
            core_cookie,
            volume,
            muted,
        } => {
            // A decimal point forces a GVariant double even for 0.0 and 1.0.
            let volume = volume
                .map(|volume| format!("{volume:.17}"))
                .unwrap_or_else(|| "nil".into());
            let muted = muted
                .map(|muted| muted.to_string())
                .unwrap_or_else(|| "nil".into());
            format!(
                "local request={{operation=\"setLevel\",node={},cookie={},volume={},muted={}}}\n",
                node_table(node),
                lua_string(core_cookie),
                volume,
                muted
            )
        }
        Operation::SetDefault {
            node,
            core_cookie,
            expected_configured,
            expected_effective,
        } => {
            let baseline = |name: Option<&str>| name.map_or_else(|| "nil".into(), lua_string);
            let name_json = node
                .as_ref()
                .map(|node| {
                    serde_json::to_string(&serde_json::json!({"name": node.node_name}))
                        .map(|json| lua_string(&json))
                        .map_err(|_| invalid_request())
                })
                .transpose()?
                .unwrap_or_else(|| "nil".into());
            format!(
                "local request={{operation=\"setDefault\",node={},cookie={},\
                 expected_configured={},expected_effective={},name_json={name_json}}}\n",
                node.as_ref()
                    .map(node_table)
                    .unwrap_or_else(|| "nil".into()),
                lua_string(core_cookie),
                baseline(expected_configured.as_deref()),
                baseline(expected_effective.as_deref())
            )
        }
        Operation::SetEqualizer {
            node,
            core_cookie,
            settings,
        } => {
            use std::fmt::Write;
            let mut controls = String::new();
            for (name, value) in settings.controls() {
                write!(controls, "{},{value:e},", lua_string(&name))
                    .map_err(|_| invalid_request())?;
            }
            format!("local request={{operation=\"setEqualizer\",node={},cookie={},controls={{{controls}}}}}\n", node_table(node), lua_string(core_cookie))
        }
        Operation::ValidateSofa { .. } => return Err(invalid_request()),
    };
    let script = prefix + CONTROL_SCRIPT;
    if script.len() > MAX_SCRIPT_BYTES {
        return Err(invalid_request());
    }
    Ok(script)
}

#[cfg(target_os = "linux")]
fn resource_limits() -> std::io::Result<()> {
    for (resource, limit) in [
        (libc::RLIMIT_AS, ADDRESS_SPACE_BYTES),
        (libc::RLIMIT_CPU, CPU_SECONDS),
        (libc::RLIMIT_CORE, 0),
        (libc::RLIMIT_NOFILE, 64),
        (libc::RLIMIT_FSIZE, 1024 * 1024),
    ] {
        let limits = libc::rlimit {
            rlim_cur: limit as libc::rlim_t,
            rlim_max: limit as libc::rlim_t,
        };
        if unsafe { libc::setrlimit(resource, &limits) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn limited_command(
    executable: &Path,
    script_fd: Option<std::os::fd::RawFd>,
) -> AppResult<tokio::process::Command> {
    use std::process::Stdio;
    let parent = std::process::id() as libc::pid_t;
    let mut command = tokio::process::Command::new(executable);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Only connection selectors are inherited. Script/module overrides must not
    // turn a fixed helper request into executable user configuration.
    for key in [
        "WIREPLUMBER_CONFIG_DIR",
        "WIREPLUMBER_CONFIG_NAME",
        "WIREPLUMBER_CONFIG_PREFIX",
        "WIREPLUMBER_DATA_DIR",
        "WIREPLUMBER_MODULE_DIR",
        "WIREPLUMBER_LUA_PATH",
        "LUA_PATH",
        "LUA_CPATH",
        "PIPEWIRE_CONFIG_DIR",
        "PIPEWIRE_CONFIG_NAME",
        "PIPEWIRE_CONFIG_PREFIX",
        "PIPEWIRE_MODULE_DIR",
        "PIPEWIRE_PROPS",
        "PIPEWIRE_NODE",
        "PIPEWIRE_AUTOCONNECT",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
    ] {
        command.env_remove(key);
    }
    command.env("LC_ALL", "C");
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
            }
            resource_limits()?;
            if let Some(fd) = script_fd {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    Ok(command)
}

#[cfg(target_os = "linux")]
struct OwnedChild(Option<tokio::process::Child>);

#[cfg(target_os = "linux")]
impl OwnedChild {
    fn child(&mut self) -> &mut tokio::process::Child {
        self.0.as_mut().expect("owned child")
    }

    fn kill(&mut self) {
        // try_wait prevents signaling a PID/group that has already been reaped.
        let Some(child) = self.0.as_mut() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
            }
            let _ = child.start_kill();
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill();
        if let Some(mut child) = self.0.take() {
            if child.try_wait().ok().flatten().is_none() {
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        let _ = child.wait().await;
                    });
                }
                // Without a live runtime, kill_on_drop/Tokio's reaper is the backstop.
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Captured {
    stdout: Vec<u8>,
    stderr: String,
    success: bool,
}

#[cfg(target_os = "linux")]
async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> AppResult<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > limit {
        return Err(AppError::new(
            ErrorCode::InvalidData,
            "Output del worker audio troppo grande.",
        ));
    }
    Ok(bytes)
}

#[cfg(target_os = "linux")]
async fn capture(
    mut command: tokio::process::Command,
    input: Option<&[u8]>,
    deadline: Duration,
) -> AppResult<Captured> {
    use tokio::io::AsyncWriteExt;
    let mut child = OwnedChild(Some(command.spawn().map_err(|error| {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::ProviderMissing,
            std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            _ => ErrorCode::Io,
        };
        AppError::new(code, "Impossibile avviare il worker audio/wpexec.").detail(error.to_string())
    })?));
    let stdin = child.child().stdin.take();
    let stdout = child.child().stdout.take().ok_or_else(invalid_request)?;
    let stderr = child.child().stderr.take().ok_or_else(invalid_request)?;
    let result = tokio::time::timeout(deadline, async {
        let write = async move {
            if let Some(mut stdin) = stdin {
                if let Some(input) = input {
                    stdin.write_all(input).await?;
                }
                stdin.shutdown().await?;
                // Closing the pipe, not merely flushing it, supplies protocol EOF.
                drop(stdin);
            }
            Ok::<(), AppError>(())
        };
        let wait = async { child.child().wait().await.map_err(AppError::from) };
        // No detached pipe readers: cancellation drops all IO and kills the child.
        let (_, stdout, stderr, status) = tokio::try_join!(
            write,
            read_bounded(stdout, MAX_RESPONSE_BYTES),
            read_bounded(stderr, MAX_RESPONSE_BYTES),
            wait
        )?;
        Ok(Captured {
            stdout,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            success: status.success(),
        })
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
        result => {
            child.kill();
            let confirmed = tokio::time::timeout(REAP_DEADLINE, child.child().wait()).await;
            let mut error = match result {
                Ok(Err(error)) => error,
                Err(_) => AppError::new(
                    ErrorCode::Timeout,
                    "Tempo massimo del worker audio superato; terminazione richiesta.",
                ),
                Ok(Ok(_)) => unreachable!(),
            };
            if !matches!(confirmed, Ok(Ok(_))) {
                error = error.detail("SIGKILL inviato; chiusura del worker non ancora confermata.");
            }
            Err(error)
        }
    }
}

#[cfg(target_os = "linux")]
fn resolve_wpexec_path() -> PathBuf {
    if let Some(path) = std::env::var_os("PERIPHERAL_HUB_WPEXEC_PATH").map(PathBuf::from) {
        if path.is_file() {
            return path;
        }
    }
    for candidate in &[
        "/usr/bin/wpexec",
        "/usr/sbin/wpexec",
        "/bin/wpexec",
        "/sbin/wpexec",
        "/usr/local/bin/wpexec",
    ] {
        let p = Path::new(candidate);
        if p.is_file() {
            return p.to_path_buf();
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("wpexec");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("/usr/bin/wpexec")
}

#[cfg(target_os = "linux")]
async fn run_control(request: &Request) -> AppResult<()> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    let script = render_lua(request)?;
    let fd = unsafe {
        libc::memfd_create(
            c"peripheral-hub-audio.lua".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd == -1 {
        return Err(AppError::from(std::io::Error::last_os_error()));
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(script.as_bytes())?;
    let seals = libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, seals) } == -1 {
        return Err(AppError::from(std::io::Error::last_os_error()));
    }
    let wpexec = resolve_wpexec_path();
    let mut command = limited_command(&wpexec, Some(file.as_raw_fd()))?;
    // No wpexec arguments: 0.4 uses key=value, 0.5 uses a SPA JSON object.
    command.arg(format!("/proc/self/fd/{}", file.as_raw_fd()));
    let output = capture(command, None, CONTROL_DEADLINE).await?;
    Response::parse(&output.stdout, output.success).map_err(|error| {
        if error.detail.is_some() || output.stderr.is_empty() {
            error
        } else {
            error.detail(output.stderr)
        }
    })
}

#[cfg(target_os = "linux")]
fn validate_sofa_child(bytes: Vec<u8>) -> AppResult<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::OpenOptionsExt;
    let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || !(128..=crate::surround::MAX_SOFA_BYTES as u64).contains(&metadata.len())
    {
        return Err(AppError::new(
            ErrorCode::HrtfUnavailable,
            "File SOFA non regolare o oltre il limite di 64 MiB.",
        ));
    }
    // libmysofa opens the same held inode, not a pathname that can be replaced.
    crate::surround::validate_sofa_native(Path::new(&format!("/proc/self/fd/{}", file.as_raw_fd())))
}

// Private binary entry point. Native validation is synchronous ONLY in this
// limited child process; the parent never calls mysofa_load or spawn_blocking.
#[cfg(target_os = "linux")]
pub fn run_stdio() -> i32 {
    use std::io::{Read, Write};
    let result = (|| {
        resource_limits()?;
        unsafe {
            libc::alarm(DEADLINE.as_secs() as libc::c_uint);
        }
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        let request = Request::parse(&bytes)?;
        match request.request {
            Operation::ValidateSofa { path } => validate_sofa_child(path),
            _ => tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(run_control(&request)),
        }
    })();
    let exit = i32::from(result.is_err());
    let response = Response::from_result(result);
    match serde_json::to_vec(&response) {
        Ok(bytes) if bytes.len() <= MAX_RESPONSE_BYTES => {
            if std::io::stdout().write_all(&bytes).is_err() {
                return 1;
            }
        }
        _ => return 1,
    }
    exit
}

#[cfg(not(target_os = "linux"))]
pub fn run_stdio() -> i32 {
    eprintln!("Il worker audio richiede Linux.");
    1
}

#[cfg(test)]
#[path = "audio_worker_tests.rs"]
mod tests;

#[cfg(test)]
mod drop_safety_tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn owned_child_empty_drop_is_infallible() {
        let mut child = OwnedChild(None);
        child.kill();
        drop(child);
    }
}
