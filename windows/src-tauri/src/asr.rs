use std::{fs, path::Path, time::Duration};

use reqwest::blocking::{
    multipart::{Form, Part},
    Client,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::credentials;

pub const DEFAULT_MODEL: &str = "gpt-4o-transcribe";
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const MAX_ERROR_BODY_CHARS: usize = 500;
const MIN_AUDIO_BYTES: usize = 16_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrSettings {
    pub model: String,
    pub base_url: String,
    pub api_key_configured: bool,
    pub api_key_hint: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrSettingsInput {
    pub api_key: Option<String>,
    pub model: String,
    pub base_url: String,
}

#[derive(Clone, Debug)]
pub struct OpenAiConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredAsrSettings {
    #[serde(default)]
    model: String,
    #[serde(default)]
    base_url: String,
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

pub fn load_settings(app: &AppHandle) -> Result<AsrSettings, String> {
    let stored = read_stored_settings(app)?;
    let api_key = credentials::read_openai_api_key()?;

    Ok(AsrSettings {
        model: effective_model(&stored.model),
        base_url: effective_base_url(&stored.base_url),
        api_key_configured: api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        api_key_hint: api_key
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(credentials::mask_api_key),
    })
}

pub fn save_settings(app: &AppHandle, input: AsrSettingsInput) -> Result<AsrSettings, String> {
    let model = effective_model(&input.model);
    let base_url = normalize_base_url(&input.base_url)?;

    if let Some(api_key) = input.api_key.as_deref() {
        if !api_key.trim().is_empty() {
            credentials::write_openai_api_key(api_key)?;
        }
    }

    let stored = StoredAsrSettings { model, base_url };
    let serialized = serde_json::to_vec_pretty(&stored)
        .map_err(|error| format!("Failed to serialize ASR settings: {error}"))?;
    let path = settings_path(app)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create the settings directory '{}': {error}",
                parent.display()
            )
        })?;
    }
    fs::write(&path, serialized)
        .map_err(|error| format!("Failed to save ASR settings '{}': {error}", path.display()))?;

    load_settings(app)
}

pub fn clear_api_key() -> Result<(), String> {
    credentials::delete_openai_api_key()
}

pub fn load_openai_config(app: &AppHandle) -> Result<Option<OpenAiConfig>, String> {
    let stored = read_stored_settings(app)?;
    let Some(api_key) = credentials::read_openai_api_key()? else {
        return Ok(None);
    };
    let api_key = api_key.trim().to_string();

    if api_key.is_empty() {
        return Ok(None);
    }

    Ok(Some(OpenAiConfig {
        api_key,
        model: effective_model(&stored.model),
        base_url: effective_base_url(&stored.base_url),
    }))
}

pub fn transcribe_wav(path: &Path, config: &OpenAiConfig) -> Result<String, String> {
    let wav_bytes = fs::read(path)
        .map_err(|error| format!("Failed to read the recording '{}': {error}", path.display()))?;

    if wav_bytes.len().saturating_sub(44) < MIN_AUDIO_BYTES {
        return Ok(String::new());
    }

    let file = Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|error| format!("Failed to prepare the audio upload: {error}"))?;
    let form = Form::new()
        .part("file", file)
        .text("model", config.model.clone())
        .text("response_format", "json");
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| format!("Failed to create the OpenAI HTTP client: {error}"))?;
    let endpoint = format!(
        "{}/audio/transcriptions",
        config.base_url.trim_end_matches('/')
    );
    let response = client
        .post(endpoint)
        .bearer_auth(&config.api_key)
        .multipart(form)
        .send()
        .map_err(|error| format!("OpenAI transcription request failed: {error}"))?;
    let status = response.status();

    let body = response
        .text()
        .map_err(|error| format!("Failed to read the OpenAI response: {error}"))?;

    if !status.is_success() {
        let summary = truncate_for_error(&body);
        return Err(format!(
            "OpenAI transcription returned HTTP {status}: {summary}"
        ));
    }

    parse_transcription_response(&body)
}

fn read_stored_settings(app: &AppHandle) -> Result<StoredAsrSettings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(StoredAsrSettings::default());
    }

    let bytes = fs::read(&path)
        .map_err(|error| format!("Failed to read ASR settings '{}': {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Failed to parse ASR settings '{}': {error}", path.display()))
}

fn settings_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("asr.json"))
        .map_err(|error| format!("Failed to resolve the application data directory: {error}"))
}

fn effective_model(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        DEFAULT_MODEL.to_string()
    } else {
        model.to_string()
    }
}

fn effective_base_url(base_url: &str) -> String {
    let base_url = base_url.trim();
    if base_url.is_empty() {
        DEFAULT_BASE_URL.to_string()
    } else {
        base_url.trim_end_matches('/').to_string()
    }
}

fn normalize_base_url(base_url: &str) -> Result<String, String> {
    let base_url = effective_base_url(base_url);
    if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
        return Err("The OpenAI base URL must start with https:// or http://".to_string());
    }
    Ok(base_url)
}

fn truncate_for_error(body: &str) -> String {
    let mut chars = body.chars();
    let summary = chars
        .by_ref()
        .take(MAX_ERROR_BODY_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{summary}...")
    } else {
        summary
    }
}

fn parse_transcription_response(body: &str) -> Result<String, String> {
    let response: TranscriptionResponse = serde_json::from_str(body)
        .map_err(|error| format!("Failed to parse the OpenAI transcription response: {error}"))?;
    Ok(response.text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_from_transcription_response() {
        assert_eq!(
            parse_transcription_response(r#"{"text":"  hello world  "}"#).unwrap(),
            "hello world"
        );
    }

    #[test]
    fn rejects_a_response_without_text() {
        assert!(parse_transcription_response(r#"{"error":"bad request"}"#).is_err());
    }

    #[test]
    fn applies_defaults_and_removes_a_trailing_slash() {
        assert_eq!(effective_model(""), DEFAULT_MODEL);
        assert_eq!(effective_base_url(""), DEFAULT_BASE_URL);
        assert_eq!(
            normalize_base_url("https://example.com/v1/").unwrap(),
            "https://example.com/v1"
        );
        assert!(normalize_base_url("example.com/v1").is_err());
    }
}
