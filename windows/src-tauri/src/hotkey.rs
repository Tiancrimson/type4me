use tauri::{plugin::TauriPlugin, AppHandle, Manager, Wry};
use tauri_plugin_global_shortcut::{
    Builder, GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState,
};

use crate::audio::{AudioController, HotkeyStyle, RecordingPhase};

pub const DEFAULT_SHORTCUT: &str = "CommandOrControl+Shift+Space";
pub const DEFAULT_SHORTCUT_LABEL: &str = "Ctrl+Shift+Space";

#[derive(Clone, Debug)]
pub struct HotkeyRegistration {
    pub shortcut: String,
    pub registered: bool,
    pub message: Option<String>,
}

pub fn plugin() -> TauriPlugin<Wry> {
    Builder::new().with_handler(handle_shortcut).build()
}

pub fn register(app: &AppHandle) -> HotkeyRegistration {
    let candidates = [
        (DEFAULT_SHORTCUT, DEFAULT_SHORTCUT_LABEL),
        ("CommandOrControl+Alt+Space", "Ctrl+Alt+Space"),
        ("CommandOrControl+Shift+F9", "Ctrl+Shift+F9"),
    ];
    let mut errors = Vec::new();

    for (index, (shortcut, label)) in candidates.into_iter().enumerate() {
        match app.global_shortcut().register(shortcut) {
            Ok(()) => {
                let message = if index == 0 {
                    None
                } else {
                    Some(format!(
                        "{DEFAULT_SHORTCUT_LABEL} 已被其他程序占用，已改用 {label}。"
                    ))
                };
                return HotkeyRegistration {
                    shortcut: label.to_string(),
                    registered: true,
                    message,
                };
            }
            Err(error) => errors.push(format!("{label}: {error}")),
        }
    }

    HotkeyRegistration {
        shortcut: DEFAULT_SHORTCUT_LABEL.to_string(),
        registered: false,
        message: Some(format!(
            "无法注册全局快捷键；请关闭占用快捷键的程序后重启 Type4Me。尝试结果：{}",
            errors.join("；")
        )),
    }
}

fn handle_shortcut(app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent) {
    let Some(controller) = app.try_state::<AudioController>() else {
        return;
    };
    let device_id = controller.selected_device_id();

    match controller.hotkey_style() {
        HotkeyStyle::Hold => match event.state {
            ShortcutState::Pressed => {
                if controller.snapshot().phase == RecordingPhase::Idle {
                    let _ = controller.start_recording(device_id);
                }
            }
            ShortcutState::Released => {
                let _ = controller.stop_recording();
            }
        },
        HotkeyStyle::Toggle => {
            if event.state == ShortcutState::Pressed {
                match controller.snapshot().phase {
                    RecordingPhase::Idle => {
                        let _ = controller.start_recording(device_id);
                    }
                    RecordingPhase::Starting | RecordingPhase::Recording => {
                        let _ = controller.stop_recording();
                    }
                    RecordingPhase::Stopping
                    | RecordingPhase::Cancelling
                    | RecordingPhase::Transcribing
                    | RecordingPhase::Injecting => {}
                }
            }
        }
    }
}
