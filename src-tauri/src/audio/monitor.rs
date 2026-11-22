use super::controller::AudioEvent;
#[cfg(target_os = "linux")]
use super::parser::JsonArrayParser;
#[cfg(target_os = "linux")]
use crate::models::{AppError, AppResult, ErrorCode};
#[cfg(target_os = "linux")]
use crate::process;
#[cfg(target_os = "linux")]
use crate::surround::runtime::OwnedChild;
#[cfg(target_os = "linux")]
use std::time::Duration;
#[cfg(target_os = "linux")]
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::Sender;

pub async fn monitor(sender: Sender<AudioEvent>) {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = sender.send(AudioEvent::Disconnected).await;
    }
    #[cfg(target_os = "linux")]
    {
        loop {
            if sender.is_closed() {
                return;
            }
            let result = monitor_once(&sender).await;
            if sender.send(AudioEvent::Disconnected).await.is_err() {
                return;
            }
            let delay = if result.is_ok() {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(2)
            };
            tokio::select! { _ = sender.closed() => return, _ = tokio::time::sleep(delay) => {} }
        }
    }
}

#[cfg(target_os = "linux")]
pub async fn monitor_once(sender: &Sender<AudioEvent>) -> AppResult<()> {
    let mut command = process::owned_command("pw-dump")?;
    command.args(["-m", "-N"]);
    let mut process = OwnedChild::spawn(command)?;
    let mut stdout = process
        .stdout()
        .ok_or_else(|| AppError::new(ErrorCode::Io, "Monitor PipeWire senza stdout."))?;
    let mut parser = JsonArrayParser::default();
    let mut initial = true;
    let mut buffer = [0; 8192];
    let result = async {
        loop {
            let read = tokio::select! {
                _ = sender.closed() => return Ok(()),
                read = stdout.read(&mut buffer) => read,
            };
            let length = read?;
            if length == 0 {
                parser.finish()?;
                return Err(AppError::new(
                    ErrorCode::AudioUnavailable,
                    "Monitor PipeWire terminato.",
                )
                .detail(process.stderr()));
            }
            for array in parser.feed(&buffer[..length])? {
                let event = if initial {
                    initial = false;
                    AudioEvent::Snapshot(array)
                } else {
                    AudioEvent::Changed(array)
                };
                if sender.send(event).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
    .await;
    let _ = process.terminate().await;
    result
}
