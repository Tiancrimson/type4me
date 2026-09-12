use serde::Serialize;
use std::{fs, path::PathBuf};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime, State, WindowEvent,
};

mod asr;
mod audio;
mod credentials;
mod hotkey;
mod injection;
mod startup;
mod wav;

use asr::{
    AsrProvider, AsrSettings, AsrSettingsInput, SenseVoiceModelManager, SenseVoiceModelStatus,
};
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
fn set_hotkey_style(
    controller: State<'_, AudioController>,
    style: HotkeyStyle,
) -> Result<RecordingState, String> {
    controller.set_hotkey_style(style)
}

#[tauri::command]
fn update_hotkey(
    app: AppHandle,
    controller: State<'_, AudioController>,
    shortcut: String,
) -> Result<RecordingState, String> {
    let state = controller.snapshot();
    let registration = hotkey::update(&app, &state.hotkey, state.hotkey_registered, &shortcut)?;
    controller.set_hotkey_registration(registration);
    Ok(controller.snapshot())
}

#[tauri::command]
fn reset_hotkey(
    app: AppHandle,
    controller: State<'_, AudioController>,
) -> Result<RecordingState, String> {
    let state = controller.snapshot();
    let registration = hotkey::reset(&app, &state.hotkey, state.hotkey_registered)?;
    controller.set_hotkey_registration(registration);
    Ok(controller.snapshot())
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
fn open_recordings_folder(controller: State<'_, AudioController>) -> Result<(), String> {
    controller.open_recordings_folder()
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
fn clear_asr_api_key(provider: AsrProvider) -> Result<(), String> {
    asr::clear_api_key(provider)
}

#[tauri::command]
fn get_sensevoice_model_status(models: State<'_, SenseVoiceModelManager>) -> SenseVoiceModelStatus {
    models.status()
}

#[tauri::command]
fn download_sensevoice_model(
    models: State<'_, SenseVoiceModelManager>,
) -> Result<SenseVoiceModelStatus, String> {
    models.download()
}

#[tauri::command]
fn get_launch_at_startup() -> Result<bool, String> {
    startup::is_enabled()
}

#[tauri::command]
fn set_launch_at_startup(enabled: bool) -> Result<bool, String> {
    startup::set_enabled(enabled)?;
    startup::is_enabled()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_panic_logger();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(hotkey::plugin())
        .setup(|app| {
            let model_manager = SenseVoiceModelManager::new(app.handle().clone());
            app.manage(model_manager.clone());
            let controller = AudioController::new(app.handle().clone(), model_manager);
            app.manage(controller.clone());
            controller.set_hotkey_registration(hotkey::register(app.handle()));
            create_tray(app.handle())?;

            if let Some(window) = app.get_webview_window("main") {
                if should_start_hidden() {
                    let _ = window.hide();
                } else {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_runtime_status,
            get_recording_state,
            list_audio_devices,
            select_audio_device,
            set_hotkey_style,
            update_hotkey,
            reset_hotkey,
            start_recording,
            stop_recording,
            cancel_recording,
            open_recordings_folder,
            get_asr_settings,
            save_asr_settings,
            clear_asr_api_key,
            get_sensevoice_model_status,
            download_sensevoice_model,
            get_launch_at_startup,
            set_launch_at_startup,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn create_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(
        app,
        "tray-show",
        "显示主窗口 / Show Window",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "tray-quit", "退出 / Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &separator, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .tooltip("Type4Me")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-show" => show_main_window(app),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn should_start_hidden() -> bool {
    std::env::args_os().any(|argument| argument == "--hidden")
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
