use crate::models::{AppError, AppResult, AudioProfile, ErrorCode, Snapshot};
use crate::{request_exit, AppState, AudioMutation};
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub async fn get_snapshot(state: State<'_, Arc<AppState>>) -> AppResult<Snapshot> {
    state.get_snapshot().await
}

#[tauri::command]
pub async fn refresh_devices(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> AppResult<Snapshot> {
    state.refresh_all(&app).await
}

#[tauri::command]
pub async fn set_dpi(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    device_id: String,
    x: u32,
    y: u32,
) -> AppResult<Snapshot> {
    state.set_dpi(&app, &device_id, x, y).await
}

#[tauri::command]
pub async fn set_polling_rate(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    device_id: String,
    rate: u32,
) -> AppResult<Snapshot> {
    state.set_polling_rate(&app, &device_id, rate).await
}

#[tauri::command]
pub async fn set_default_output(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    node_name: String,
) -> AppResult<Snapshot> {
    state
        .mutate_audio(&app, AudioMutation::DefaultOutput(node_name))
        .await
}

#[tauri::command]
pub async fn set_volume(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    node_name: String,
    volume: f64,
) -> AppResult<Snapshot> {
    state
        .mutate_audio(&app, AudioMutation::Volume(node_name, volume))
        .await
}

#[tauri::command]
pub async fn set_mute(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    node_name: String,
    muted: bool,
) -> AppResult<Snapshot> {
    state
        .mutate_audio(&app, AudioMutation::Mute(node_name, muted))
        .await
}

#[tauri::command]
pub async fn set_surround(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    enabled: bool,
) -> AppResult<Snapshot> {
    state
        .mutate_audio(&app, AudioMutation::Surround(enabled))
        .await
}

#[tauri::command]
pub async fn import_hrtf(app: AppHandle, state: State<'_, Arc<AppState>>) -> AppResult<Snapshot> {
    state.require_platform()?;
    let dialog_app = app.clone();
    let selected = tokio::task::spawn_blocking(move || {
        dialog_app
            .dialog()
            .file()
            .set_title("Seleziona un file HRTF SOFA")
            .add_filter("HRTF SOFA", &["sofa"])
            .blocking_pick_file()
    })
    .await
    .map_err(|error| {
        AppError::new(ErrorCode::Io, "Dialogo HRTF non disponibile.").detail(error.to_string())
    })?
    .ok_or_else(|| AppError::new(ErrorCode::Cancelled, "Importazione HRTF annullata."))?;
    let path = selected.into_path().map_err(|error| {
        AppError::new(ErrorCode::Validation, "Seleziona un file SOFA locale.")
            .detail(error.to_string())
    })?;
    state.mutate_audio(&app, AudioMutation::Import(path)).await
}

#[tauri::command]
pub async fn save_audio_profile(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    profile: AudioProfile,
) -> AppResult<Snapshot> {
    state.save_profile(&app, profile).await
}

#[tauri::command]
pub async fn apply_audio_profile(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    profile_id: String,
) -> AppResult<Snapshot> {
    state.apply_profile(&app, &profile_id).await
}

#[tauri::command]
pub async fn exit_app(app: AppHandle) -> AppResult<()> {
    request_exit(app);
    Ok(())
}
