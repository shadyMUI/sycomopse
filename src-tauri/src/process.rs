use crate::models::{AppError, AppResult, ErrorCode};
#[cfg(any(target_os = "linux", test))]
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

const STDOUT_LIMIT: u64 = 16 * 1024 * 1024;
const STDERR_LIMIT: u64 = 64 * 1024;

#[derive(Debug)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

fn allowed(executable: &str) -> bool {
    matches!(
        executable,
        "pw-dump" | "wpctl" | "pipewire" | "headsetcontrol" | "ratbagd" | "systemctl"
    )
}

pub fn owned_command(executable: &str) -> AppResult<Command> {
    if !allowed(executable) {
        return Err(AppError::new(
            ErrorCode::Validation,
            "Eseguibile non consentito.",
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(AppError::unsupported_platform())
    }
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let parent = std::process::id() as libc::pid_t;
        // The child must not outlive this app, including a crash between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "Parent exited before exec",
                    ));
                }
                Ok(())
            });
        }
        Ok(command)
    }
}

async fn read_bounded(reader: impl AsyncRead + Unpin, limit: u64) -> AppResult<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > limit {
        return Err(AppError::new(
            ErrorCode::InvalidData,
            "Output del processo troppo grande.",
        ));
    }
    Ok(bytes)
}

async fn capture(mut child: Child, timeout: Duration) -> AppResult<ProcessOutput> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new(ErrorCode::Io, "stdout non disponibile."))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::new(ErrorCode::Io, "stderr non disponibile."))?;
    let mut stdout_task = tokio::spawn(read_bounded(stdout, STDOUT_LIMIT));
    let mut stderr_task = tokio::spawn(read_bounded(stderr, STDERR_LIMIT));
    let result = tokio::time::timeout(timeout, async {
        let status = child.wait().await?;
        let stdout = (&mut stdout_task).await.map_err(|error| {
            AppError::new(ErrorCode::Io, "Lettura stdout interrotta.").detail(error.to_string())
        })??;
        let stderr = (&mut stderr_task).await.map_err(|error| {
            AppError::new(ErrorCode::Io, "Lettura stderr interrotta.").detail(error.to_string())
        })??;
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        if !status.success() {
            return Err(AppError::new(
                ErrorCode::CommandFailed,
                "Il comando del sistema non e' riuscito.",
            )
            .detail(format!("exit={status}; {stderr}")));
        }
        Ok(ProcessOutput { stdout, stderr })
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
        other => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            stdout_task.abort();
            stderr_task.abort();
            match other {
                Ok(Err(error)) => Err(error),
                Err(_) => Err(AppError::new(
                    ErrorCode::Timeout,
                    "Il comando del sistema ha superato il tempo massimo.",
                )),
                Ok(Ok(_)) => unreachable!(),
            }
        }
    }
}

pub async fn run(executable: &str, args: &[&str], timeout: Duration) -> AppResult<ProcessOutput> {
    let mut command = owned_command(executable)?;
    command.args(args);
    let child = command.spawn().map_err(|error| {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::ProviderMissing,
            std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            _ => ErrorCode::Io,
        };
        AppError::new(code, format!("Impossibile eseguire {executable}.")).detail(error.to_string())
    })?;
    capture(child, timeout).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_shells_and_arbitrary_executables() {
        assert!(!allowed("sh"));
        assert!(!allowed("sudo"));
        assert!(!allowed("wpctl --help"));
        assert!(allowed("wpctl"));
        assert!(allowed("systemctl"));
        assert!(!allowed("systemctl --user start openrazer-daemon"));
        assert_eq!(
            owned_command("powershell").unwrap_err().code,
            ErrorCode::Validation
        );
    }

    #[tokio::test]
    async fn bounded_reader_rejects_excess_data() {
        assert_eq!(
            read_bounded(&b"12345"[..], 4).await.unwrap_err().code,
            ErrorCode::InvalidData
        );
        assert_eq!(read_bounded(&b"1234"[..], 4).await.unwrap(), b"1234");
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn no_cli_execution_on_unsupported_platform() {
        let error = run("wpctl", &["set-volume", "999", "1"], Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UnsupportedPlatform);
    }

    #[test]
    #[ignore = "child fixture invoked only by the timeout test"]
    fn timeout_child() {
        std::thread::sleep(Duration::from_secs(30));
    }

    #[tokio::test]
    async fn timeout_kills_and_reaps_owned_child() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", "process::tests::timeout_child"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let error = capture(command.spawn().unwrap(), Duration::from_millis(80))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Timeout);
    }
}
