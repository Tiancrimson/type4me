use std::{mem::size_of, ptr, slice, thread, time::Duration};

use serde::Serialize;
use windows::Win32::{
    Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
            IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
        },
        Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
        Ole::CF_UNICODETEXT,
    },
    UI::{
        Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
            KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_V,
        },
        WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
    },
};

const CLIPBOARD_RETRY_DELAY: Duration = Duration::from_millis(25);
const CLIPBOARD_RETRY_COUNT: usize = 12;
const PASTE_SETTLE_DELAY: Duration = Duration::from_millis(300);
const UNICODE_INPUT_CHUNK_SIZE: usize = 256;
const UNICODE_TEXT_FORMAT: u32 = CF_UNICODETEXT.0 as u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InjectionOutcome {
    Inserted,
    CopiedToClipboard,
}

pub fn inject_text(text: &str) -> Result<InjectionOutcome, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(InjectionOutcome::Inserted);
    }

    if !has_external_foreground_window() {
        write_clipboard_text(Some(text))?;
        return Ok(InjectionOutcome::CopiedToClipboard);
    }

    let original = ClipboardSnapshot::capture().ok();
    if let Err(clipboard_error) = write_clipboard_text(Some(text)) {
        return send_unicode_text(text).map_err(|unicode_error| {
            format!(
                "Failed to inject text. Clipboard access failed: {clipboard_error}. \
                 Direct Unicode input also failed: {unicode_error}"
            )
        });
    }

    let pasted_sequence_number = unsafe { GetClipboardSequenceNumber() };
    if simulate_paste().is_err() {
        return Ok(InjectionOutcome::CopiedToClipboard);
    }

    thread::sleep(PASTE_SETTLE_DELAY);
    if let Some(original) = original {
        let _ = original.restore(pasted_sequence_number);
    }

    Ok(InjectionOutcome::Inserted)
}

fn send_unicode_text(text: &str) -> Result<InjectionOutcome, String> {
    let code_units = text.encode_utf16().collect::<Vec<_>>();

    for chunk in code_units.chunks(UNICODE_INPUT_CHUNK_SIZE) {
        let mut inputs = Vec::with_capacity(chunk.len() * 2);
        for unit in chunk {
            inputs.push(unicode_input(*unit, false));
            inputs.push(unicode_input(*unit, true));
        }

        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if sent != inputs.len() as u32 {
            let error = std::io::Error::last_os_error();
            return Err(format!(
                "Windows accepted {sent} of {} Unicode input events: {error}",
                inputs.len()
            ));
        }
    }

    Ok(InjectionOutcome::Inserted)
}

struct ClipboardSnapshot {
    text: Option<String>,
}

impl ClipboardSnapshot {
    fn capture() -> Result<Self, String> {
        Ok(Self {
            text: read_clipboard_text()?,
        })
    }

    fn restore(self, expected_sequence_number: u32) -> Result<(), String> {
        if unsafe { GetClipboardSequenceNumber() } != expected_sequence_number {
            return Ok(());
        }

        write_clipboard_text(self.text.as_deref())
    }
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn open_clipboard() -> Result<ClipboardGuard, String> {
    let mut last_error = None;

    for attempt in 0..CLIPBOARD_RETRY_COUNT {
        match unsafe { OpenClipboard(None) } {
            Ok(()) => return Ok(ClipboardGuard),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < CLIPBOARD_RETRY_COUNT {
                    thread::sleep(CLIPBOARD_RETRY_DELAY);
                }
            }
        }
    }

    Err(format!(
        "Failed to open the Windows clipboard: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    ))
}

fn read_clipboard_text() -> Result<Option<String>, String> {
    let _guard = open_clipboard()?;
    if unsafe { IsClipboardFormatAvailable(UNICODE_TEXT_FORMAT) }.is_err() {
        return Ok(None);
    }

    let handle = unsafe { GetClipboardData(UNICODE_TEXT_FORMAT) }
        .map_err(|error| format!("Failed to read clipboard text: {error}"))?;
    if handle.is_invalid() {
        return Ok(None);
    }

    let global = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(global) };
    if size < size_of::<u16>() {
        return Ok(None);
    }

    let pointer = unsafe { GlobalLock(global) };
    if pointer.is_null() {
        return Err("Failed to lock the clipboard text buffer".to_string());
    }

    let decoded = unsafe {
        let units = slice::from_raw_parts(pointer.cast::<u16>(), size / size_of::<u16>());
        let length = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        String::from_utf16(&units[..length])
            .map_err(|error| format!("The clipboard text is not valid UTF-16: {error}"))
    };
    let _ = unsafe { GlobalUnlock(global) };

    decoded.map(Some)
}

fn write_clipboard_text(text: Option<&str>) -> Result<(), String> {
    let _guard = open_clipboard()?;
    let allocation = text.map(allocate_clipboard_text).transpose()?;
    let mut transferred = false;
    let result = (|| {
        unsafe { EmptyClipboard() }
            .map_err(|error| format!("Failed to empty the Windows clipboard: {error}"))?;

        if let Some(allocation) = allocation {
            unsafe { SetClipboardData(UNICODE_TEXT_FORMAT, Some(HANDLE(allocation.0))) }
                .map_err(|error| format!("Failed to write clipboard text: {error}"))?;
            transferred = true;
        }

        Ok(())
    })();

    if let Some(allocation) = allocation {
        if !transferred {
            let _ = unsafe { GlobalFree(Some(allocation)) };
        }
    }

    result
}

fn allocate_clipboard_text(text: &str) -> Result<HGLOBAL, String> {
    let mut encoded = text.encode_utf16().collect::<Vec<_>>();
    encoded.push(0);
    let byte_length = encoded
        .len()
        .checked_mul(size_of::<u16>())
        .ok_or_else(|| "The transcription is too large for the Windows clipboard".to_string())?;
    let allocation = unsafe { GlobalAlloc(GMEM_MOVEABLE, byte_length) }
        .map_err(|error| format!("Failed to allocate clipboard memory: {error}"))?;
    let pointer = unsafe { GlobalLock(allocation) };
    if pointer.is_null() {
        let _ = unsafe { GlobalFree(Some(allocation)) };
        return Err("Failed to lock clipboard memory".to_string());
    }

    unsafe {
        ptr::copy_nonoverlapping(
            encoded.as_ptr().cast::<u8>(),
            pointer.cast::<u8>(),
            byte_length,
        );
    }

    if let Err(error) = unsafe { GlobalUnlock(allocation) } {
        let _ = unsafe { GlobalFree(Some(allocation)) };
        return Err(format!("Failed to unlock clipboard memory: {error}"));
    }

    Ok(allocation)
}

fn simulate_paste() -> Result<(), String> {
    let inputs = [
        keyboard_input(VK_CONTROL, false),
        keyboard_input(VK_V, false),
        keyboard_input(VK_V, true),
        keyboard_input(VK_CONTROL, true),
    ];
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err("Windows did not accept the Ctrl+V input sequence".to_string())
    }
}

fn keyboard_input(key: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                ..Default::default()
            },
        },
    }
}

fn unicode_input(code_unit: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: code_unit,
                dwFlags: if key_up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
                ..Default::default()
            },
        },
    }
}

fn has_external_foreground_window() -> bool {
    let window: HWND = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return false;
    }

    let mut process_id = 0_u32;
    let thread_id = unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
    thread_id != 0 && process_id != 0 && process_id != std::process::id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_a_noop() {
        assert_eq!(inject_text("   ").unwrap(), InjectionOutcome::Inserted);
    }
}
