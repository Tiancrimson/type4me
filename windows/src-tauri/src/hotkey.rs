use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{plugin::TauriPlugin, AppHandle, Manager, Wry};
use tauri_plugin_global_shortcut::{
    Builder, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent, ShortcutState,
};

use crate::audio::{AudioController, HotkeyStyle, RecordingPhase};

pub const DEFAULT_SHORTCUT: &str = "control+shift+Space";
pub const DEFAULT_SHORTCUT_LABEL: &str = "Ctrl+Shift+Space";

const FALLBACK_SHORTCUTS: [(&str, &str); 2] = [
    ("control+alt+Space", "Ctrl+Alt+Space"),
    ("control+shift+F9", "Ctrl+Shift+F9"),
];
const SETTINGS_FILE: &str = "hotkey.json";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyRegistration {
    pub shortcut: String,
    pub label: String,
    pub registered: bool,
    pub message: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredHotkeySettings {
    #[serde(default)]
    shortcut: Option<String>,
    #[serde(default)]
    style: Option<String>,
}

pub fn plugin() -> TauriPlugin<Wry> {
    Builder::new().with_handler(handle_shortcut).build()
}

pub fn register(app: &AppHandle) -> HotkeyRegistration {
    let saved_shortcut = read_saved_shortcut(app).ok().flatten();
    let mut candidates = Vec::new();
    let mut errors = Vec::new();

    if let Some(shortcut) = saved_shortcut {
        candidates.push((shortcut.to_string(), shortcut_to_label(&shortcut)));
    }
    for (shortcut, label) in
        std::iter::once((DEFAULT_SHORTCUT, DEFAULT_SHORTCUT_LABEL)).chain(FALLBACK_SHORTCUTS)
    {
        if candidates
            .iter()
            .all(|(candidate, _)| candidate.as_str() != shortcut)
        {
            candidates.push((shortcut.to_string(), label.to_string()));
        }
    }

    for (index, (shortcut, label)) in candidates.iter().enumerate() {
        match register_exclusive(app, shortcut) {
            Ok(_) => {
                let message = if index == 0 || candidates[0].0 == shortcut.as_str() {
                    None
                } else {
                    Some(format!("{} 不可用，已改用 {label}。", candidates[0].1))
                };
                let _ = save_shortcut(app, shortcut);

                return HotkeyRegistration {
                    shortcut: shortcut.clone(),
                    label: label.clone(),
                    registered: true,
                    message,
                };
            }
            Err(error) => errors.push(format!("{label}: {error}")),
        }
    }

    let (shortcut, label) = candidates.first().cloned().unwrap_or_else(|| {
        (
            DEFAULT_SHORTCUT.to_string(),
            DEFAULT_SHORTCUT_LABEL.to_string(),
        )
    });

    HotkeyRegistration {
        shortcut,
        label,
        registered: false,
        message: Some(format!(
            "无法注册全局快捷键；请关闭占用快捷键的程序后重启 Type4Me。尝试结果：{}",
            errors.join("；")
        )),
    }
}

pub fn update(
    app: &AppHandle,
    current_shortcut: &str,
    current_registered: bool,
    requested_shortcut: &str,
) -> Result<HotkeyRegistration, String> {
    let normalized = normalize_shortcut(requested_shortcut)?;
    let normalized_string = normalized.to_string();

    if normalized_string == current_shortcut && current_registered {
        return Ok(registration_for(normalized));
    }

    app.global_shortcut()
        .unregister_all()
        .map_err(|error| format!("无法更新全局快捷键：{error}"))?;

    match app.global_shortcut().register(normalized) {
        Ok(()) => {
            if let Err(error) = save_shortcut(app, &normalized_string) {
                let _ = app.global_shortcut().unregister(normalized);
                let _ = app.global_shortcut().register(current_shortcut);
                return Err(error);
            }

            Ok(registration_for(normalized))
        }
        Err(error) => {
            if current_registered {
                let _ = app.global_shortcut().register(current_shortcut);
            }
            Err(format!(
                "快捷键 {} 已被其他程序占用，未保存修改：{error}",
                shortcut_to_label(&normalized_string)
            ))
        }
    }
}

pub fn reset(
    app: &AppHandle,
    current_shortcut: &str,
    current_registered: bool,
) -> Result<HotkeyRegistration, String> {
    update(app, current_shortcut, current_registered, DEFAULT_SHORTCUT)
}

pub fn load_style(app: &AppHandle) -> Option<HotkeyStyle> {
    match read_settings(app).ok()?.style.as_deref() {
        Some("hold") => Some(HotkeyStyle::Hold),
        Some("toggle") => Some(HotkeyStyle::Toggle),
        _ => None,
    }
}

pub fn save_style(app: &AppHandle, style: HotkeyStyle) -> Result<(), String> {
    let mut settings = read_settings(app).unwrap_or_default();
    settings.style = Some(
        match style {
            HotkeyStyle::Hold => "hold",
            HotkeyStyle::Toggle => "toggle",
        }
        .to_string(),
    );
    write_settings(app, &settings)
}

fn register_exclusive(app: &AppHandle, shortcut: &str) -> Result<HotkeyRegistration, String> {
    let normalized = normalize_shortcut(shortcut)?;

    app.global_shortcut()
        .unregister_all()
        .map_err(|error| format!("无法准备全局快捷键：{error}"))?;
    app.global_shortcut()
        .register(normalized)
        .map_err(|error| error.to_string())?;

    Ok(registration_for(normalized))
}

fn registration_for(shortcut: Shortcut) -> HotkeyRegistration {
    let shortcut = shortcut.to_string();
    HotkeyRegistration {
        label: shortcut_to_label(&shortcut),
        shortcut,
        registered: true,
        message: None,
    }
}

fn normalize_shortcut(shortcut: &str) -> Result<Shortcut, String> {
    let shortcut = shortcut
        .trim()
        .parse::<Shortcut>()
        .map_err(|error| format!("快捷键格式无效，请使用“修饰键 + 主键”的形式：{error}"))?;
    let supported_modifiers =
        Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::SUPER;

    if (shortcut.mods & supported_modifiers).is_empty() {
        return Err("快捷键至少需要包含 Ctrl、Alt、Shift 或 Win 中的一个修饰键".to_string());
    }

    Ok(shortcut)
}

fn shortcut_to_label(shortcut: &str) -> String {
    let Ok(shortcut) = shortcut.parse::<Shortcut>() else {
        return shortcut.to_string();
    };
    let mut labels = Vec::with_capacity(5);

    if shortcut.mods.contains(Modifiers::CONTROL) {
        labels.push("Ctrl".to_string());
    }
    if shortcut.mods.contains(Modifiers::SHIFT) {
        labels.push("Shift".to_string());
    }
    if shortcut.mods.contains(Modifiers::ALT) {
        labels.push("Alt".to_string());
    }
    if shortcut.mods.contains(Modifiers::SUPER) {
        labels.push("Win".to_string());
    }

    labels.push(key_to_label(&shortcut.key.to_string()));
    labels.join("+")
}

fn key_to_label(key: &str) -> String {
    if key.starts_with("Key") && key.len() == 4 {
        return key[3..].to_ascii_uppercase();
    }
    if key.starts_with("Digit") && key.len() == 6 {
        return key[5..].to_string();
    }

    match key {
        "Space" => "Space".to_string(),
        "ArrowUp" => "Up".to_string(),
        "ArrowDown" => "Down".to_string(),
        "ArrowLeft" => "Left".to_string(),
        "ArrowRight" => "Right".to_string(),
        "NumpadAdd" => "Num +".to_string(),
        "NumpadSubtract" => "Num -".to_string(),
        "NumpadMultiply" => "Num *".to_string(),
        "NumpadDivide" => "Num /".to_string(),
        "NumpadDecimal" => "Num .".to_string(),
        key if key.starts_with("Numpad") => key[6..].to_string(),
        key => key.to_string(),
    }
}

fn read_saved_shortcut(app: &AppHandle) -> Result<Option<String>, String> {
    Ok(read_settings(app)?
        .shortcut
        .filter(|shortcut| !shortcut.trim().is_empty()))
}

fn save_shortcut(app: &AppHandle, shortcut: &str) -> Result<(), String> {
    let mut settings = read_settings(app).unwrap_or_default();
    settings.shortcut = Some(shortcut.to_string());
    write_settings(app, &settings)
}

fn read_settings(app: &AppHandle) -> Result<StoredHotkeySettings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(StoredHotkeySettings::default());
    }

    let bytes = fs::read(&path)
        .map_err(|error| format!("无法读取快捷键设置 '{}': {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("无法解析快捷键设置 '{}': {error}", path.display()))
}

fn write_settings(app: &AppHandle, settings: &StoredHotkeySettings) -> Result<(), String> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建快捷键设置目录 '{}': {error}", parent.display()))?;
    }
    let serialized = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("无法序列化快捷键设置：{error}"))?;
    fs::write(&path, serialized)
        .map_err(|error| format!("无法保存快捷键设置 '{}': {error}", path.display()))
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join(SETTINGS_FILE))
        .map_err(|error| format!("无法解析应用数据目录：{error}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_prints_shortcut_labels() {
        assert_eq!(shortcut_to_label("control+shift+Space"), "Ctrl+Shift+Space");
        assert_eq!(shortcut_to_label("super+alt+KeyQ"), "Alt+Win+Q");
        assert_eq!(shortcut_to_label("control+Digit1"), "Ctrl+1");
    }

    #[test]
    fn rejects_shortcuts_without_modifiers() {
        assert!(normalize_shortcut("Space").is_err());
        assert!(normalize_shortcut("control+shift+Space").is_ok());
    }
}
