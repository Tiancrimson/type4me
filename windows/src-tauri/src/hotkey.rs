use tauri::{plugin::TauriPlugin, AppHandle, Manager, Wry};
use tauri_plugin_global_shortcut::{Builder, Shortcut, ShortcutEvent, ShortcutState};

use crate::audio::{AudioController, HotkeyStyle, RecordingPhase};

pub const DEFAULT_SHORTCUT: &str = "CommandOrControl+Shift+Space";
pub const DEFAULT_SHORTCUT_LABEL: &str = "Ctrl+Shift+Space";

pub fn plugin() -> TauriPlugin<Wry> {
    Builder::new()
        .with_shortcut(DEFAULT_SHORTCUT)
        .expect("the default Type4Me shortcut must be valid")
        .with_handler(handle_shortcut)
        .build()
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
