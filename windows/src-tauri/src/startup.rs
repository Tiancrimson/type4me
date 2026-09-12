use std::{path::Path, slice};

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR},
        System::Registry::{
            RegCloseKey, RegCreateKeyW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
            RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
            REG_VALUE_TYPE,
        },
    },
};

const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Type4Me";

pub fn is_enabled() -> Result<bool, String> {
    let expected = startup_command(&current_executable()?);
    Ok(read_value()?.as_deref() == Some(expected.as_str()))
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        let command = startup_command(&current_executable()?);
        write_value(&command)
    } else {
        delete_value()
    }
}

pub fn startup_command(executable: &Path) -> String {
    format!("\"{}\" --hidden", executable.display())
}

fn current_executable() -> Result<std::path::PathBuf, String> {
    std::env::current_exe()
        .map_err(|error| format!("Failed to resolve the Type4Me executable: {error}"))
}

fn read_value() -> Result<Option<String>, String> {
    let key = match open_run_key(KEY_QUERY_VALUE)? {
        Some(key) => key,
        None => return Ok(None),
    };
    let _key = RegistryKey(key);
    let value_name = wide(VALUE_NAME);
    let mut value_type = REG_VALUE_TYPE(0);
    let mut byte_length = 0u32;

    let result = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut value_type),
            None,
            Some(&mut byte_length),
        )
    };

    if result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    ensure_success(result, "read the Type4Me startup setting")?;

    if value_type != REG_SZ {
        return Err(
            "The Type4Me startup setting has an unexpected registry value type".to_string(),
        );
    }

    let mut data = vec![0u16; (byte_length as usize).div_ceil(2)];
    let mut actual_byte_length = (data.len() * std::mem::size_of::<u16>()) as u32;
    let result = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut value_type),
            Some(data.as_mut_ptr().cast()),
            Some(&mut actual_byte_length),
        )
    };
    ensure_success(result, "read the Type4Me startup command")?;

    data.truncate((actual_byte_length as usize).div_ceil(2));
    while data.last() == Some(&0) {
        data.pop();
    }

    String::from_utf16(&data)
        .map(Some)
        .map_err(|error| format!("The Type4Me startup command is not valid UTF-16: {error}"))
}

fn write_value(command: &str) -> Result<(), String> {
    let subkey = wide(RUN_KEY_PATH);
    let value_name = wide(VALUE_NAME);
    let mut key = HKEY::default();
    let result = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), &mut key) };
    ensure_success(result, "open the Windows startup registry key")?;
    let _key = RegistryKey(key);

    let command = command
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let command_bytes = unsafe {
        slice::from_raw_parts(
            command.as_ptr().cast::<u8>(),
            command.len() * std::mem::size_of::<u16>(),
        )
    };
    let result = unsafe {
        RegSetValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            None,
            REG_SZ,
            Some(command_bytes),
        )
    };
    ensure_success(result, "save the Type4Me startup setting")
}

fn delete_value() -> Result<(), String> {
    let Some(key) = open_run_key(KEY_SET_VALUE)? else {
        return Ok(());
    };
    let _key = RegistryKey(key);
    let value_name = wide(VALUE_NAME);
    let result = unsafe { RegDeleteValueW(key, PCWSTR(value_name.as_ptr())) };

    if result == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    ensure_success(result, "remove the Type4Me startup setting")
}

fn open_run_key(
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Result<Option<HKEY>, String> {
    let subkey = wide(RUN_KEY_PATH);
    let mut key = HKEY::default();
    let result = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            access,
            &mut key,
        )
    };

    if result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    ensure_success(result, "open the Windows startup registry key")?;
    Ok(Some(key))
}

fn ensure_success(result: WIN32_ERROR, action: &str) -> Result<(), String> {
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!(
            "Failed to {action}: Windows error 0x{:08X}",
            result.0
        ))
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::startup_command;
    use std::path::Path;

    #[test]
    fn quotes_paths_with_spaces_and_adds_hidden_flag() {
        assert_eq!(
            startup_command(Path::new(r"C:\Program Files\Type4Me\Type4Me.exe")),
            r#""C:\Program Files\Type4Me\Type4Me.exe" --hidden"#
        );
    }

    #[test]
    fn quotes_paths_without_spaces() {
        assert_eq!(
            startup_command(Path::new(r"C:\Type4Me\Type4Me.exe")),
            r#""C:\Type4Me\Type4Me.exe" --hidden"#
        );
    }
}
