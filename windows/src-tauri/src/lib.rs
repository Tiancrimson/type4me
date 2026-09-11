use serde::Serialize;
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Manager, State};

mod asr;
mod audio;
mod credentials;
mod hotkey;
mod injection;
mod wav;

use asr::{AsrSettings, AsrSettingsInput};
use audio::{AudioController, AudioDeviceInfo, HotkeyStyle, RecordingState};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    app_name: &'static str,
    version: &'static str,
    platform: &'static str,
    architecture: &'static str,
    development_build: bool,
    backend_connected: bool,
}

#[tauri::command]
fn get_runtime_status() -> RuntimeStatus {
    RuntimeStatus {
        app_name: "Type4Me",
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        development_build: cfg!(debug_assertions),
        backend_connected: true,
    }
}

#[tauri::command]
fn get_recording_state(controller: State<'_, AudioController>) -> RecordingState {
    controller.snapshot()
}

#[tauri::command]
fn list_audio_devices(
    controller: State<'_, AudioController>,
) -> Result<Vec<AudioDeviceInfo>, String> {
    controller.list_devices()
}

#[tauri::command]
fn select_audio_device(
    controller: State<'_, AudioController>,
    device_id: Option<String>,
) -> Result<RecordingState, String> {
    controller.set_selected_device(device_id)
}

#[tauri::command]
fn set_hotkey_style(controller: State<'_, AudioController>, style: HotkeyStyle) -> RecordingState {
    controller.set_hotkey_style(style)
}

#[tauri::command]
fn start_recording(
    controller: State<'_, AudioController>,
    device_id: Option<String>,
) -> Result<RecordingState, String> {
    controller.start_recording(device_id)
}

#[tauri::command]
fn stop_recording(controller: State<'_, AudioController>) -> Result<RecordingState, String> {
    controller.stop_recording()
}

#[tauri::command]
fn cancel_recording(controller: State<'_, AudioController>) -> Result<RecordingState, String> {
    controller.cancel_recording()
}

#[tauri::command]
fn get_asr_settings(app: AppHandle) -> Result<AsrSettings, String> {
    asr::load_settings(&app)
}

#[tauri::command]
fn save_asr_settings(app: AppHandle, settings: AsrSettingsInput) -> Result<AsrSettings, String> {
    asr::save_settings(&app, settings)
}

#[tauri::command]
fn clear_openai_api_key() -> Result<(), String> {
    asr::clear_api_key()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_panic_logger();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(hotkey::plugin())
        .setup(|app| {
            let controller = AudioController::new(app.handle().clone());
            app.manage(controller.clone());
            controller.set_hotkey_registration(hotkey::register(app.handle()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_runtime_status,
            get_recording_state,
            list_audio_devices,
            select_audio_device,
            set_hotkey_style,
            start_recording,
            stop_recording,
            cancel_recording,
            get_asr_settings,
            save_asr_settings,
            clear_openai_api_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn install_panic_logger() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        if let Some(path) = panic_log_path() {
            let location = panic_info
                .location()
                .map(|location| {
                    format!(
                        "{}:{}:{}",
                        location.file(),
                        location.line(),
                        location.column()
                    )
                })
                .unwrap_or_else(|| "unknown location".to_string());
            let _ = fs::write(&path, format!("{panic_info}\nLocation: {location}\n"));
        }

        default_hook(panic_info);
    }));
}

fn panic_log_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("com.tiancrimson.type4me").join("panic.log"))
        .inspect(|path| {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
        })
}
