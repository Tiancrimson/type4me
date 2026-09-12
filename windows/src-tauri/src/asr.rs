use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use bzip2::read::BzDecoder;
use hound::WavReader;
use reqwest::blocking::{
    multipart::{Form, Part},
    Client,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sherpa_rs::sense_voice::{SenseVoiceConfig, SenseVoiceRecognizer};
use tauri::{AppHandle, Emitter, Manager};

use crate::credentials;

pub const DEFAULT_MODEL: &str = "gpt-4o-transcribe";
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const SENSE_VOICE_MODEL_ID: &str = "sensevoice-small-int8";
const SENSE_VOICE_ARCHIVE_SHA256: &str =
    "7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e";
const SENSE_VOICE_ARCHIVE_BYTES: u64 = 163_002_883;
const SENSE_VOICE_DOWNLOAD_URLS: [&str; 2] = [
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2",
    "https://gh-proxy.com/https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2",
];
const SENSE_VOICE_PROGRESS_EVENT: &str = "sensevoice-model-status";
const MAX_ERROR_BODY_CHARS: usize = 500;
const MIN_AUDIO_BYTES: usize = 16_000;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AsrProvider {
    Openai,
    Groq,
    Siliconflow,
    Sensevoice,
    Custom,
}

impl AsrProvider {
    fn display_name(self) -> &'static str {
        match self {
            Self::Openai => "OpenAI",
            Self::Groq => "Groq",
            Self::Siliconflow => "SiliconFlow",
            Self::Sensevoice => "SenseVoice",
            Self::Custom => "Custom",
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            Self::Openai => DEFAULT_MODEL,
            Self::Groq => "whisper-large-v3-turbo",
            Self::Siliconflow => "FunAudioLLM/SenseVoiceSmall",
            Self::Sensevoice => SENSE_VOICE_MODEL_ID,
            Self::Custom => DEFAULT_MODEL,
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            Self::Openai => DEFAULT_BASE_URL,
            Self::Groq => "https://api.groq.com/openai/v1",
            Self::Siliconflow => "https://api.siliconflow.cn/v1",
            Self::Sensevoice => "",
            Self::Custom => DEFAULT_BASE_URL,
        }
    }

    fn requires_api_key(self) -> bool {
        self != Self::Sensevoice
    }

    fn credential_target(self) -> &'static str {
        match self {
            Self::Openai => "Type4Me/OpenAI",
            Self::Groq => "Type4Me/Groq",
            Self::Siliconflow => "Type4Me/SiliconFlow",
            Self::Sensevoice => "",
            Self::Custom => "Type4Me/Custom",
        }
    }

    fn credential_user(self) -> &'static str {
        self.display_name()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrSettings {
    pub provider: AsrProvider,
    pub model: String,
    pub base_url: String,
    pub settings_saved: bool,
    pub api_key_configured: bool,
    pub api_key_hint: Option<String>,
    pub requires_api_key: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrSettingsInput {
    pub provider: AsrProvider,
    pub api_key: Option<String>,
    pub model: String,
    pub base_url: String,
}

#[derive(Clone, Debug)]
pub struct AsrConfig {
    pub provider: AsrProvider,
    pub api_key: Option<String>,
    pub model: String,
    pub base_url: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SenseVoiceModelStatus {
    pub installed: bool,
    pub download_in_progress: bool,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub progress: f32,
    pub model_directory: Option<String>,
    pub model_size_bytes: u64,
    pub error: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredAsrSettings {
    #[serde(default)]
    provider: Option<AsrProvider>,
    #[serde(default)]
    model: String,
    #[serde(default)]
    base_url: String,
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[derive(Clone)]
pub struct SenseVoiceModelManager {
    app: AppHandle,
    state: Arc<Mutex<SenseVoiceModelState>>,
}

struct SenseVoiceModelState {
    installed: bool,
    download_in_progress: bool,
    downloaded_bytes: u64,
    total_bytes: u64,
    model_directory: Option<String>,
    model_size_bytes: u64,
    error: Option<String>,
}

impl SenseVoiceModelManager {
    pub fn new(app: AppHandle) -> Self {
        let model_directory = sense_voice_model_directory(&app).ok();
        let installed = model_directory
            .as_deref()
            .is_some_and(sense_voice_model_is_installed);
        let model_size_bytes = model_directory
            .as_deref()
            .filter(|path| sense_voice_model_is_installed(path))
            .map(sense_voice_model_size)
            .unwrap_or(0);

        Self {
            app,
            state: Arc::new(Mutex::new(SenseVoiceModelState {
                installed,
                download_in_progress: false,
                downloaded_bytes: if installed {
                    SENSE_VOICE_ARCHIVE_BYTES
                } else {
                    0
                },
                total_bytes: SENSE_VOICE_ARCHIVE_BYTES,
                model_directory: model_directory.map(|path| path.to_string_lossy().into_owned()),
                model_size_bytes,
                error: None,
            })),
        }
    }

    pub fn status(&self) -> SenseVoiceModelStatus {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        SenseVoiceModelStatus {
            installed: state.installed,
            download_in_progress: state.download_in_progress,
            downloaded_bytes: state.downloaded_bytes,
            total_bytes: state.total_bytes,
            progress: if state.total_bytes == 0 {
                0.0
            } else {
                (state.downloaded_bytes as f32 / state.total_bytes as f32).clamp(0.0, 1.0)
            },
            model_directory: state.model_directory.clone(),
            model_size_bytes: state.model_size_bytes,
            error: state.error.clone(),
        }
    }

    pub fn download(&self) -> Result<SenseVoiceModelStatus, String> {
        self.begin_download()?;
        let manager = self.clone();

        if let Err(error) = thread::Builder::new()
            .name("type4me-sensevoice-download".to_string())
            .spawn(move || {
                if let Err(error) = manager.run_download() {
                    manager.fail_download(error);
                }
            })
        {
            let message = format!("Failed to start the SenseVoice model download: {error}");
            self.fail_download(message.clone());
            return Err(message);
        }

        Ok(self.status())
    }

    pub fn ensure_installed(&self) -> Result<PathBuf, String> {
        if let Some(directory) = self.installed_directory() {
            return Ok(directory);
        }

        if self.status().download_in_progress {
            for _ in 0..1_200 {
                thread::sleep(Duration::from_millis(500));
                if let Some(directory) = self.installed_directory() {
                    return Ok(directory);
                }
                let status = self.status();
                if !status.download_in_progress {
                    return Err(status
                        .error
                        .unwrap_or_else(|| "The SenseVoice model download did not finish".into()));
                }
            }
            return Err("Timed out waiting for the SenseVoice model download".to_string());
        }

        self.begin_download()?;
        if let Err(error) = self.run_download() {
            self.fail_download(error.clone());
            return Err(error);
        }

        self.installed_directory()
            .ok_or_else(|| "The SenseVoice model files are incomplete".to_string())
    }

    fn installed_directory(&self) -> Option<PathBuf> {
        let directory = sense_voice_model_directory(&self.app).ok()?;
        sense_voice_model_is_installed(&directory).then_some(directory)
    }

    fn begin_download(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.download_in_progress {
            return Err("The SenseVoice model is already downloading".to_string());
        }
        if state.installed {
            return Ok(());
        }

        state.download_in_progress = true;
        state.downloaded_bytes = 0;
        state.total_bytes = SENSE_VOICE_ARCHIVE_BYTES;
        state.error = None;
        drop(state);
        self.publish_status();
        Ok(())
    }

    fn run_download(&self) -> Result<(), String> {
        let model_directory = sense_voice_model_directory(&self.app)?;
        let models_directory = model_directory
            .parent()
            .ok_or_else(|| "Failed to resolve the SenseVoice model directory".to_string())?;
        fs::create_dir_all(models_directory).map_err(|error| {
            format!(
                "Failed to create the model directory '{}': {error}",
                models_directory.display()
            )
        })?;

        let archive_path = models_directory.join(".sensevoice-model.tar.bz2");
        let staging_directory = models_directory.join(".sensevoice-model.extracting");
        let mut failures = Vec::new();

        for (index, url) in SENSE_VOICE_DOWNLOAD_URLS.iter().enumerate() {
            if index > 0 {
                self.set_download_progress(0, SENSE_VOICE_ARCHIVE_BYTES, None);
            }

            let download_result = self.download_file(url, &archive_path).and_then(|_| {
                verify_sha256(&archive_path, SENSE_VOICE_ARCHIVE_SHA256).and_then(|_| {
                    extract_sense_voice_model(&archive_path, &staging_directory, &model_directory)
                })
            });

            if let Err(error) = download_result {
                failures.push(format!("{url}: {error}"));
                let _ = fs::remove_file(&archive_path);
                let _ = fs::remove_dir_all(&staging_directory);
                continue;
            }

            let _ = fs::remove_file(&archive_path);
            let model_size_bytes = sense_voice_model_size(&model_directory);
            let directory = model_directory.to_string_lossy().into_owned();
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.installed = true;
            state.download_in_progress = false;
            state.downloaded_bytes = SENSE_VOICE_ARCHIVE_BYTES;
            state.total_bytes = SENSE_VOICE_ARCHIVE_BYTES;
            state.model_directory = Some(directory);
            state.model_size_bytes = model_size_bytes;
            state.error = None;
            drop(state);
            self.publish_status();
            return Ok(());
        }

        Err(format!(
            "Failed to download the SenseVoice model. Attempts: {}",
            failures.join(" | ")
        ))
    }

    fn download_file(&self, url: &str, destination: &Path) -> Result<(), String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(900))
            .build()
            .map_err(|error| format!("Failed to create the model download client: {error}"))?;
        let mut response = client
            .get(url)
            .send()
            .map_err(|error| format!("request failed: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("returned HTTP {status}"));
        }

        let total_bytes = response
            .content_length()
            .filter(|size| *size > 0)
            .unwrap_or(SENSE_VOICE_ARCHIVE_BYTES);
        let mut file = File::create(destination).map_err(|error| {
            format!(
                "failed to create the temporary model file '{}': {error}",
                destination.display()
            )
        })?;
        let mut buffer = [0_u8; 256 * 1024];
        let mut downloaded_bytes = 0_u64;
        let mut last_publish = Instant::now();

        loop {
            let read = response
                .read(&mut buffer)
                .map_err(|error| format!("download interrupted: {error}"))?;
            if read == 0 {
                break;
            }

            file.write_all(&buffer[..read])
                .map_err(|error| format!("failed to write the model file: {error}"))?;
            downloaded_bytes += read as u64;

            if last_publish.elapsed() >= Duration::from_millis(250) {
                self.set_download_progress(downloaded_bytes, total_bytes, None);
                last_publish = Instant::now();
            }
        }

        file.flush()
            .map_err(|error| format!("failed to flush the model file: {error}"))?;
        self.set_download_progress(downloaded_bytes, total_bytes, None);
        Ok(())
    }

    fn set_download_progress(
        &self,
        downloaded_bytes: u64,
        total_bytes: u64,
        error: Option<String>,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.download_in_progress = true;
        state.downloaded_bytes = downloaded_bytes;
        state.total_bytes = total_bytes.max(1);
        state.error = error;
        drop(state);
        self.publish_status();
    }

    fn fail_download(&self, error: String) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.download_in_progress = false;
        state.error = Some(error);
        drop(state);
        self.publish_status();
    }

    fn publish_status(&self) {
        let _ = self.app.emit(SENSE_VOICE_PROGRESS_EVENT, self.status());
    }
}

pub fn load_settings(app: &AppHandle) -> Result<AsrSettings, String> {
    let stored = read_stored_settings(app)?;
    let provider = stored.provider.unwrap_or(AsrProvider::Openai);
    let api_key = read_provider_api_key(provider)?;
    let settings_saved = settings_path(app)?.is_file();

    Ok(AsrSettings {
        provider,
        model: effective_model(provider, &stored.model),
        base_url: effective_base_url(provider, &stored.base_url),
        settings_saved,
        api_key_configured: api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        api_key_hint: api_key
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(credentials::mask_api_key),
        requires_api_key: provider.requires_api_key(),
    })
}

pub fn save_settings(app: &AppHandle, input: AsrSettingsInput) -> Result<AsrSettings, String> {
    let provider = input.provider;
    let model = effective_model(provider, &input.model);
    let base_url = if provider.requires_api_key() {
        normalize_base_url(provider, &input.base_url)?
    } else {
        String::new()
    };

    if let Some(api_key) = input.api_key.as_deref() {
        if provider.requires_api_key() && !api_key.trim().is_empty() {
            write_provider_api_key(provider, api_key)?;
        }
    }

    let stored = StoredAsrSettings {
        provider: Some(provider),
        model,
        base_url,
    };
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

pub fn clear_api_key(provider: AsrProvider) -> Result<(), String> {
    if !provider.requires_api_key() {
        return Ok(());
    }
    credentials::delete_api_key(provider.credential_target(), provider.credential_user())
}

pub fn load_config(app: &AppHandle) -> Result<Option<AsrConfig>, String> {
    let stored = read_stored_settings(app)?;
    let provider = stored.provider.unwrap_or(AsrProvider::Openai);
    let api_key = if provider.requires_api_key() {
        let Some(api_key) = read_provider_api_key(provider)? else {
            return Ok(None);
        };
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            return Ok(None);
        }
        Some(api_key)
    } else {
        None
    };

    Ok(Some(AsrConfig {
        provider,
        api_key,
        model: effective_model(provider, &stored.model),
        base_url: effective_base_url(provider, &stored.base_url),
    }))
}

pub fn transcribe_wav(
    models: &SenseVoiceModelManager,
    path: &Path,
    config: &AsrConfig,
) -> Result<String, String> {
    if config.provider == AsrProvider::Sensevoice {
        transcribe_sense_voice(models, path)
    } else {
        transcribe_cloud(path, config)
    }
}

fn transcribe_sense_voice(models: &SenseVoiceModelManager, path: &Path) -> Result<String, String> {
    let model_directory = models.ensure_installed()?;
    let mut reader = WavReader::open(path)
        .map_err(|error| format!("Failed to read the recording '{}': {error}", path.display()))?;
    let spec = reader.spec();

    if spec.channels != 1 || spec.sample_rate != 16_000 {
        return Err(format!(
            "SenseVoice requires 16 kHz mono audio, but '{}' is {} Hz with {} channels",
            path.display(),
            spec.sample_rate,
            spec.channels
        ));
    }

    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode the recording: {error}"))?,
        hound::SampleFormat::Int if spec.bits_per_sample == 16 => reader
            .samples::<i16>()
            .map(|sample| sample.map(|value| f32::from(value) / 32_768.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode the recording: {error}"))?,
        format => {
            return Err(format!(
                "SenseVoice does not support this WAV sample format: {format:?}"
            ));
        }
    };

    let mut recognizer = SenseVoiceRecognizer::new(SenseVoiceConfig {
        model: model_directory
            .join("model.int8.onnx")
            .to_string_lossy()
            .into_owned(),
        tokens: model_directory
            .join("tokens.txt")
            .to_string_lossy()
            .into_owned(),
        language: "auto".to_string(),
        use_itn: true,
        num_threads: Some(4),
        debug: false,
        ..Default::default()
    })
    .map_err(|error| format!("Failed to initialize the SenseVoice recognizer: {error}"))?;
    let result = recognizer.transcribe(spec.sample_rate, &samples);
    Ok(result.text.trim().to_string())
}

fn transcribe_cloud(path: &Path, config: &AsrConfig) -> Result<String, String> {
    let wav_bytes = fs::read(path)
        .map_err(|error| format!("Failed to read the recording '{}': {error}", path.display()))?;

    if wav_bytes.len().saturating_sub(44) < MIN_AUDIO_BYTES {
        return Ok(String::new());
    }

    let api_key = config
        .api_key
        .as_deref()
        .ok_or_else(|| format!("{} requires an API key", config.provider.display_name()))?;
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
        .map_err(|error| format!("Failed to create the speech recognition HTTP client: {error}"))?;
    let endpoint = format!(
        "{}/audio/transcriptions",
        config.base_url.trim_end_matches('/')
    );
    let response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .map_err(|error| {
            format!(
                "{} transcription request failed: {error}",
                config.provider.display_name()
            )
        })?;
    let status = response.status();

    let body = response
        .text()
        .map_err(|error| format!("Failed to read the speech recognition response: {error}"))?;

    if !status.is_success() {
        let summary = truncate_for_error(&body);
        return Err(format!(
            "{} transcription returned HTTP {status}: {summary}",
            config.provider.display_name()
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

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("asr.json"))
        .map_err(|error| format!("Failed to resolve the application data directory: {error}"))
}

fn sense_voice_model_directory(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("models").join(SENSE_VOICE_MODEL_ID))
        .map_err(|error| format!("Failed to resolve the application data directory: {error}"))
}

fn effective_model(provider: AsrProvider, model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        provider.default_model().to_string()
    } else {
        model.to_string()
    }
}

fn effective_base_url(provider: AsrProvider, base_url: &str) -> String {
    let base_url = base_url.trim();
    if base_url.is_empty() {
        provider.default_base_url().to_string()
    } else {
        base_url.trim_end_matches('/').to_string()
    }
}

fn normalize_base_url(provider: AsrProvider, base_url: &str) -> Result<String, String> {
    let base_url = effective_base_url(provider, base_url);
    if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
        return Err(
            "The speech recognition base URL must start with https:// or http://".to_string(),
        );
    }
    Ok(base_url)
}

fn read_provider_api_key(provider: AsrProvider) -> Result<Option<String>, String> {
    if !provider.requires_api_key() {
        return Ok(None);
    }
    credentials::read_api_key(provider.credential_target(), provider.credential_user())
}

fn write_provider_api_key(provider: AsrProvider, api_key: &str) -> Result<(), String> {
    credentials::write_api_key(
        provider.credential_target(),
        provider.credential_user(),
        api_key,
    )
}

fn sense_voice_model_is_installed(directory: &Path) -> bool {
    directory.join("model.int8.onnx").is_file() && directory.join("tokens.txt").is_file()
}

fn sense_voice_model_size(directory: &Path) -> u64 {
    ["model.int8.onnx", "tokens.txt"]
        .iter()
        .filter_map(|name| fs::metadata(directory.join(name)).ok())
        .map(|metadata| metadata.len())
        .sum()
}

fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let mut file = File::open(path)
        .map_err(|error| format!("failed to reopen the downloaded model: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 256 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("failed to read the downloaded model: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(format!(
            "SHA-256 mismatch (expected {expected}, received {actual})"
        ));
    }
    Ok(())
}

fn extract_sense_voice_model(
    archive_path: &Path,
    staging_directory: &Path,
    destination: &Path,
) -> Result<(), String> {
    if staging_directory.exists() {
        fs::remove_dir_all(staging_directory).map_err(|error| {
            format!(
                "failed to clear the temporary model directory '{}': {error}",
                staging_directory.display()
            )
        })?;
    }
    fs::create_dir_all(staging_directory).map_err(|error| {
        format!(
            "failed to create the temporary model directory '{}': {error}",
            staging_directory.display()
        )
    })?;

    let archive_file = File::open(archive_path)
        .map_err(|error| format!("failed to open the downloaded model archive: {error}"))?;
    let decoder = BzDecoder::new(archive_file);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(staging_directory)
        .map_err(|error| format!("failed to extract the model archive: {error}"))?;

    let extracted_directory = fs::read_dir(staging_directory)
        .map_err(|error| format!("failed to inspect the extracted model: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_dir() && sense_voice_model_is_installed(path))
        .ok_or_else(|| "the model archive does not contain the expected files".to_string())?;

    if destination.exists() {
        fs::remove_dir_all(destination).map_err(|error| {
            format!(
                "failed to replace the existing model directory '{}': {error}",
                destination.display()
            )
        })?;
    }
    fs::rename(&extracted_directory, destination).map_err(|error| {
        format!(
            "failed to install the model into '{}': {error}",
            destination.display()
        )
    })?;
    let _ = fs::remove_dir_all(staging_directory);
    Ok(())
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
        .map_err(|error| format!("Failed to parse the speech recognition response: {error}"))?;
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
    fn applies_provider_defaults_and_removes_a_trailing_slash() {
        assert_eq!(effective_model(AsrProvider::Openai, ""), DEFAULT_MODEL);
        assert_eq!(
            effective_model(AsrProvider::Sensevoice, ""),
            SENSE_VOICE_MODEL_ID
        );
        assert!(!AsrProvider::Sensevoice.requires_api_key());
        assert_eq!(
            effective_base_url(AsrProvider::Groq, ""),
            "https://api.groq.com/openai/v1"
        );
        assert_eq!(
            normalize_base_url(AsrProvider::Custom, "https://example.com/v1/").unwrap(),
            "https://example.com/v1"
        );
        assert!(normalize_base_url(AsrProvider::Custom, "example.com/v1").is_err());
    }

    #[test]
    fn recognizes_the_expected_model_file_layout() {
        let directory =
            std::env::temp_dir().join(format!("type4me-sensevoice-layout-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        assert!(!sense_voice_model_is_installed(&directory));
        fs::write(directory.join("model.int8.onnx"), b"model").unwrap();
        fs::write(directory.join("tokens.txt"), b"tokens").unwrap();
        assert!(sense_voice_model_is_installed(&directory));
        let _ = fs::remove_dir_all(&directory);
    }
}
