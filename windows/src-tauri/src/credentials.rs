use std::{
    ptr::{null_mut, NonNull},
    slice,
};

use windows::{
    core::{w, PWSTR},
    Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    },
};

const CREDENTIAL_NOT_FOUND: i32 = 0x8007_0490u32 as i32;

pub fn read_openai_api_key() -> Result<Option<String>, String> {
    let mut credential: *mut CREDENTIALW = null_mut();
    let result = unsafe {
        CredReadW(
            w!("Type4Me/OpenAI"),
            CRED_TYPE_GENERIC,
            None,
            &mut credential,
        )
    };

    if let Err(error) = result {
        if error.code().0 == CREDENTIAL_NOT_FOUND {
            return Ok(None);
        }
        return Err(format!(
            "Failed to read the OpenAI API key from Windows Credential Manager: {error}"
        ));
    }

    let Some(credential) = NonNull::new(credential) else {
        return Ok(None);
    };

    let value =
        unsafe {
            let credential_ref = credential.as_ref();
            let blob_size = credential_ref.CredentialBlobSize as usize;
            if credential_ref.CredentialBlob.is_null() || blob_size == 0 {
                None
            } else {
                let blob = slice::from_raw_parts(credential_ref.CredentialBlob, blob_size);
                Some(String::from_utf8(blob.to_vec()).map_err(|error| {
                    format!("The stored OpenAI API key is not valid UTF-8: {error}")
                }))
            }
        };

    unsafe {
        CredFree(credential.as_ptr().cast());
    }

    match value {
        Some(value) => value.map(Some),
        None => Ok(None),
    }
}

pub fn write_openai_api_key(api_key: &str) -> Result<(), String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return delete_openai_api_key();
    }

    let mut target_name = wide("Type4Me/OpenAI");
    let mut user_name = wide("OpenAI");
    let mut blob = api_key.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target_name.as_mut_ptr()),
        CredentialBlobSize: u32::try_from(blob.len())
            .map_err(|_| "The OpenAI API key is too long".to_string())?,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user_name.as_mut_ptr()),
        ..Default::default()
    };

    unsafe { CredWriteW(&credential, 0) }
        .map_err(|error| format!("Failed to save the OpenAI API key: {error}"))
}

pub fn delete_openai_api_key() -> Result<(), String> {
    match unsafe { CredDeleteW(w!("Type4Me/OpenAI"), CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(()),
        Err(error) if error.code().0 == CREDENTIAL_NOT_FOUND => Ok(()),
        Err(error) => Err(format!("Failed to delete the OpenAI API key: {error}")),
    }
}

pub fn mask_api_key(api_key: &str) -> String {
    let api_key = api_key.trim();
    let visible = api_key
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();

    if api_key.chars().count() <= 4 {
        "****".to_string()
    } else {
        format!("****{visible}")
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::mask_api_key;

    #[test]
    fn masks_all_but_the_last_four_characters() {
        assert_eq!(mask_api_key("sk-1234567890"), "****7890");
        assert_eq!(mask_api_key("abc"), "****");
        assert_eq!(mask_api_key(""), "****");
    }
}
