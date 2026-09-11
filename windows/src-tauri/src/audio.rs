use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    asr,
    injection::{self, InjectionOutcome},
    wav,
};

const LEVEL_DB_FLOOR: f32 = -50.0;
const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HotkeyStyle {
    Hold,
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingPhase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Cancelling,
    Transcribing,
    Injecting,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingState {
    pub phase: RecordingPhase,
    pub selected_device_id: Option<String>,
    pub selected_device_name: Option<String>,
    pub elapsed_ms: u64,
    pub level: f32,
    pub last_recording_path: Option<String>,
    pub last_transcript: Option<String>,
    pub last_injection_outcome: Option<InjectionOutcome>,
    pub last_error: Option<String>,
    pub hotkey_style: HotkeyStyle,
    pub hotkey: String,
    pub hotkey_registered: bool,
    pub hotkey_message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioLevelEvent {
    level: f32,
    elapsed_ms: u64,
}

struct RuntimeShared {
    phase: Mutex<RecordingPhase>,
    selected_device_id: Mutex<Option<String>>,
    selected_device_name: Mutex<Option<String>>,
    started_at: Mutex<Option<Instant>>,
    level: AtomicU32,
    stop_requested: AtomicBool,
    cancel_requested: AtomicBool,
    last_recording_path: Mutex<Option<String>>,
    last_transcript: Mutex<Option<String>>,
    last_injection_outcome: Mutex<Option<InjectionOutcome>>,
    last_error: Mutex<Option<String>>,
    hotkey_style: Mutex<HotkeyStyle>,
    hotkey: Mutex<String>,
    hotkey_registered: AtomicBool,
    hotkey_message: Mutex<Option<String>>,
}

impl RuntimeShared {
    fn new() -> Self {
        Self {
            phase: Mutex::new(RecordingPhase::Idle),
            selected_device_id: Mutex::new(None),
            selected_device_name: Mutex::new(None),
            started_at: Mutex::new(None),
            level: AtomicU32::new(0_f32.to_bits()),
            stop_requested: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            last_recording_path: Mutex::new(None),
            last_transcript: Mutex::new(None),
            last_injection_outcome: Mutex::new(None),
            last_error: Mutex::new(None),
            hotkey_style: Mutex::new(HotkeyStyle::Hold),
            hotkey: Mutex::new(crate::hotkey::DEFAULT_SHORTCUT_LABEL.to_string()),
            hotkey_registered: AtomicBool::new(false),
            hotkey_message: Mutex::new(None),
        }
    }

    fn phase(&self) -> RecordingPhase {
        *self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_phase(&self, phase: RecordingPhase) {
        *self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = phase;
    }

    fn selected_device_id(&self) -> Option<String> {
        self.selected_device_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn selected_device_name(&self) -> Option<String> {
        self.selected_device_name
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn hotkey_style(&self) -> HotkeyStyle {
        *self
            .hotkey_style
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn elapsed_ms(&self) -> u64 {
        self.started_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map(|started_at| started_at.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    fn snapshot(&self) -> RecordingState {
        RecordingState {
            phase: self.phase(),
            selected_device_id: self.selected_device_id(),
            selected_device_name: self.selected_device_name(),
            elapsed_ms: self.elapsed_ms(),
            level: f32::from_bits(self.level.load(Ordering::Relaxed)),
            last_recording_path: self
                .last_recording_path
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            last_transcript: self
                .last_transcript
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            last_injection_outcome: *self
                .last_injection_outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            last_error: self
                .last_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            hotkey_style: self.hotkey_style(),
            hotkey: self
                .hotkey
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            hotkey_registered: self.hotkey_registered.load(Ordering::Relaxed),
            hotkey_message: self
                .hotkey_message
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }
    }

    fn set_error(&self, message: impl Into<String>) {
        *self
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message.into());
    }

    fn take_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

#[derive(Clone)]
pub struct AudioController {
    app: AppHandle,
    shared: Arc<RuntimeShared>,
}

impl AudioController {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            shared: Arc::new(RuntimeShared::new()),
        }
    }

    pub fn snapshot(&self) -> RecordingState {
        self.shared.snapshot()
    }

    pub fn selected_device_id(&self) -> Option<String> {
        self.shared.selected_device_id()
    }

    pub fn hotkey_style(&self) -> HotkeyStyle {
        self.shared.hotkey_style()
    }

    pub fn set_hotkey_registration(&self, registration: crate::hotkey::HotkeyRegistration) {
        *self
            .shared
            .hotkey
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = registration.shortcut;
        self.shared
            .hotkey_registered
            .store(registration.registered, Ordering::Relaxed);
        *self
            .shared
            .hotkey_message
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = registration.message;
    }

    pub fn list_devices(&self) -> Result<Vec<AudioDeviceInfo>, String> {
        list_input_devices()
    }

    pub fn set_selected_device(&self, device_id: Option<String>) -> Result<RecordingState, String> {
        self.ensure_idle("select a microphone")?;
        let use_default = device_id.is_none();

        let selected = match device_id {
            Some(id) => list_input_devices()?
                .into_iter()
                .find(|device| device.id == id)
                .ok_or_else(|| "The selected microphone is no longer available".to_string())?,
            None => list_input_devices()?
                .into_iter()
                .find(|device| device.is_default)
                .ok_or_else(|| "No default microphone is available".to_string())?,
        };

        *self
            .shared
            .selected_device_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            if use_default { None } else { Some(selected.id) };
        *self
            .shared
            .selected_device_name
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(selected.name);
        self.shared.stop_requested.store(false, Ordering::SeqCst);
        self.shared.cancel_requested.store(false, Ordering::SeqCst);
        self.shared
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();

        self.emit_state();
        Ok(self.snapshot())
    }

    pub fn set_hotkey_style(&self, style: HotkeyStyle) -> RecordingState {
        *self
            .shared
            .hotkey_style
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = style;
        self.emit_state();
        self.snapshot()
    }

    pub fn start_recording(&self, device_id: Option<String>) -> Result<RecordingState, String> {
        self.ensure_idle("start a recording")?;

        let selected = resolve_input_device(device_id.as_deref()).inspect_err(|message| {
            self.shared.set_error(message.clone());
            self.emit_state();
        })?;

        let output_path = self.create_recording_path().inspect_err(|message| {
            self.shared.set_error(message.clone());
            self.emit_state();
        })?;

        *self
            .shared
            .selected_device_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = selected.id;
        *self
            .shared
            .selected_device_name
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(selected.name);
        *self
            .shared
            .started_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.shared.level.store(0_f32.to_bits(), Ordering::Relaxed);
        self.shared.stop_requested.store(false, Ordering::SeqCst);
        self.shared.cancel_requested.store(false, Ordering::SeqCst);
        self.shared
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        *self
            .shared
            .last_transcript
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .shared
            .last_injection_outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.shared.set_phase(RecordingPhase::Starting);
        self.emit_state();

        let controller = self.clone();
        let selected_device_id = self.shared.selected_device_id();
        if let Err(error) = thread::Builder::new()
            .name("type4me-audio-capture".to_string())
            .spawn(move || {
                controller.complete_recording_session(output_path, selected_device_id);
            })
        {
            let message = format!("Failed to start the audio worker: {error}");
            self.shared.set_phase(RecordingPhase::Idle);
            self.shared.set_error(message.clone());
            self.emit_state();
            return Err(message);
        }

        Ok(self.snapshot())
    }

    pub fn stop_recording(&self) -> Result<RecordingState, String> {
        match self.shared.phase() {
            RecordingPhase::Idle => Ok(self.snapshot()),
            RecordingPhase::Starting | RecordingPhase::Recording => {
                self.shared.stop_requested.store(true, Ordering::SeqCst);
                self.shared.set_phase(RecordingPhase::Stopping);
                self.emit_state();
                Ok(self.snapshot())
            }
            RecordingPhase::Stopping
            | RecordingPhase::Cancelling
            | RecordingPhase::Transcribing
            | RecordingPhase::Injecting => Ok(self.snapshot()),
        }
    }

    pub fn cancel_recording(&self) -> Result<RecordingState, String> {
        match self.shared.phase() {
            RecordingPhase::Idle => Ok(self.snapshot()),
            RecordingPhase::Starting | RecordingPhase::Recording => {
                self.shared.cancel_requested.store(true, Ordering::SeqCst);
                self.shared.stop_requested.store(true, Ordering::SeqCst);
                self.shared.set_phase(RecordingPhase::Cancelling);
                self.emit_state();
                Ok(self.snapshot())
            }
            RecordingPhase::Stopping
            | RecordingPhase::Cancelling
            | RecordingPhase::Transcribing
            | RecordingPhase::Injecting => Ok(self.snapshot()),
        }
    }

    fn ensure_idle(&self, action: &str) -> Result<(), String> {
        if self.shared.phase() == RecordingPhase::Idle {
            Ok(())
        } else {
            Err(format!(
                "Cannot {action} while a recording session is active"
            ))
        }
    }

    fn create_recording_path(&self) -> Result<PathBuf, String> {
        let app_data_dir = self.app.path().app_data_dir().map_err(|error| {
            format!("Failed to resolve the application data directory: {error}")
        })?;
        let recordings_dir = app_data_dir.join("recordings");
        fs::create_dir_all(&recordings_dir).map_err(|error| {
            format!(
                "Failed to create the recordings directory '{}': {error}",
                recordings_dir.display()
            )
        })?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("The system clock is unavailable: {error}"))?
            .as_millis();
        Ok(recordings_dir.join(format!("recording-{timestamp}.wav")))
    }

    fn complete_recording_session(&self, output_path: PathBuf, selected_device_id: Option<String>) {
        let capture_result = self.capture_audio(selected_device_id.as_deref());
        let mut saved_path = None;

        *self
            .shared
            .started_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.shared.level.store(0_f32.to_bits(), Ordering::Relaxed);

        match capture_result {
            Ok(capture) if capture.cancelled => {}
            Ok(capture) => {
                let wav = wav::encode_pcm16le(&capture.samples);
                if let Err(error) = fs::write(&output_path, wav) {
                    self.shared.set_error(format!(
                        "Failed to save the recording '{}': {error}",
                        output_path.display()
                    ));
                } else {
                    *self
                        .shared
                        .last_recording_path
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(output_path.to_string_lossy().into_owned());
                    saved_path = Some(output_path);
                }
            }
            Err(error) => self.shared.set_error(error),
        }

        if let Some(path) = saved_path {
            self.transcribe_and_inject(&path);
        }

        self.shared.set_phase(RecordingPhase::Idle);
        self.shared.stop_requested.store(false, Ordering::SeqCst);
        self.shared.cancel_requested.store(false, Ordering::SeqCst);
        self.emit_state();
    }

    fn transcribe_and_inject(&self, path: &std::path::Path) {
        let config = match asr::load_openai_config(&self.app) {
            Ok(Some(config)) => config,
            Ok(None) => return,
            Err(error) => {
                self.shared.set_error(error);
                return;
            }
        };

        self.shared.set_phase(RecordingPhase::Transcribing);
        self.emit_state();
        let transcript = match asr::transcribe_wav(path, &config) {
            Ok(transcript) => transcript,
            Err(error) => {
                self.shared.set_error(error);
                return;
            }
        };

        if transcript.is_empty() {
            return;
        }

        *self
            .shared
            .last_transcript
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(transcript.clone());
        self.shared.set_phase(RecordingPhase::Injecting);
        self.emit_state();

        match injection::inject_text(&transcript) {
            Ok(outcome) => {
                *self
                    .shared
                    .last_injection_outcome
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outcome);
            }
            Err(error) => self.shared.set_error(error),
        }
    }

    fn capture_audio(&self, selected_device_id: Option<&str>) -> Result<CaptureOutcome, String> {
        let device = resolve_input_device(selected_device_id)?.device;
        let supported_config = device
            .default_input_config()
            .map_err(|error| format!("Failed to read the microphone configuration: {error}"))?;
        let sample_format = supported_config.sample_format();
        let config = supported_config.config();
        let input_rate = config.sample_rate.0;
        let channels = config.channels as usize;

        let (sender, receiver) = mpsc::channel::<AudioPacket>();
        let error_shared = Arc::clone(&self.shared);
        let error_callback = move |error: cpal::StreamError| {
            error_shared.set_error(format!("The microphone stream stopped: {error}"));
            error_shared.stop_requested.store(true, Ordering::SeqCst);
        };

        let stream = build_input_stream(
            &device,
            &config,
            sample_format,
            input_rate,
            channels,
            sender,
            error_callback,
        )?;
        stream
            .play()
            .map_err(|error| format!("Failed to start microphone capture: {error}"))?;

        if self.shared.stop_requested.load(Ordering::SeqCst) {
            return Ok(CaptureOutcome {
                samples: Vec::new(),
                cancelled: true,
            });
        }

        *self
            .shared
            .started_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
        self.shared.set_phase(RecordingPhase::Recording);
        self.emit_state();

        let mut samples = Vec::new();
        let mut last_level_emit = Instant::now();

        while !self.shared.stop_requested.load(Ordering::SeqCst) {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(packet) => {
                    append_packet(&self.shared, &mut samples, packet);
                    if last_level_emit.elapsed() >= LEVEL_EMIT_INTERVAL {
                        self.emit_level();
                        last_level_emit = Instant::now();
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        drop(stream);

        while let Ok(packet) = receiver.try_recv() {
            append_packet(&self.shared, &mut samples, packet);
        }

        if let Some(error) = self.shared.take_error() {
            return Err(error);
        }

        Ok(CaptureOutcome {
            samples,
            cancelled: self.shared.cancel_requested.load(Ordering::SeqCst),
        })
    }

    fn emit_state(&self) {
        let _ = self.app.emit("recording-state", self.snapshot());
    }

    fn emit_level(&self) {
        let _ = self.app.emit(
            "audio-level",
            AudioLevelEvent {
                level: f32::from_bits(self.shared.level.load(Ordering::Relaxed)),
                elapsed_ms: self.shared.elapsed_ms(),
            },
        );
    }
}

struct CaptureOutcome {
    samples: Vec<i16>,
    cancelled: bool,
}

struct ResolvedDevice {
    device: cpal::Device,
    id: Option<String>,
    name: String,
}

struct AudioPacket {
    samples: Vec<i16>,
    level: f32,
}

fn append_packet(shared: &RuntimeShared, samples: &mut Vec<i16>, packet: AudioPacket) {
    samples.extend_from_slice(&packet.samples);
    shared
        .level
        .store(packet.level.to_bits(), Ordering::Relaxed);
}

fn list_input_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    let host = cpal::default_host();
    let default_device = host.default_input_device();
    let devices = host
        .input_devices()
        .map_err(|error| format!("Failed to enumerate microphones: {error}"))?;
    let mut result = Vec::new();

    for (index, device) in devices.enumerate() {
        let name = device
            .name()
            .map_err(|error| format!("Failed to read a microphone name: {error}"))?;
        result.push(AudioDeviceInfo {
            id: format!("{index}:{name}"),
            is_default: default_device
                .as_ref()
                .is_some_and(|default| devices_match(&device, default)),
            name,
        });
    }

    Ok(result)
}

fn devices_match(left: &cpal::Device, right: &cpal::Device) -> bool {
    let (cpal::platform::DeviceInner::Wasapi(left), cpal::platform::DeviceInner::Wasapi(right)) =
        (left.as_inner(), right.as_inner());

    left == right
}

fn resolve_input_device(selected_device_id: Option<&str>) -> Result<ResolvedDevice, String> {
    let host = cpal::default_host();

    if let Some(selected_device_id) = selected_device_id {
        let (index, expected_name) = parse_device_id(selected_device_id)?;
        let device = host
            .input_devices()
            .map_err(|error| format!("Failed to enumerate microphones: {error}"))?
            .nth(index)
            .ok_or_else(|| "The selected microphone is no longer available".to_string())?;
        let name = device
            .name()
            .map_err(|error| format!("Failed to read the selected microphone name: {error}"))?;

        if name != expected_name {
            return Err(
                "The microphone list changed. Refresh the device list and select again."
                    .to_string(),
            );
        }

        return Ok(ResolvedDevice {
            device,
            id: Some(selected_device_id.to_string()),
            name,
        });
    }

    let device = host
        .default_input_device()
        .ok_or_else(|| "No default microphone is available".to_string())?;
    let name = device
        .name()
        .map_err(|error| format!("Failed to read the default microphone name: {error}"))?;

    Ok(ResolvedDevice {
        device,
        id: None,
        name,
    })
}

fn parse_device_id(device_id: &str) -> Result<(usize, String), String> {
    let (index, name) = device_id
        .split_once(':')
        .ok_or_else(|| "The selected microphone identifier is invalid".to_string())?;
    let index = index
        .parse::<usize>()
        .map_err(|_| "The selected microphone identifier is invalid".to_string())?;
    Ok((index, name.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn build_input_stream<E>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    input_rate: u32,
    channels: usize,
    sender: mpsc::Sender<AudioPacket>,
    error_callback: E,
) -> Result<cpal::Stream, String>
where
    E: FnMut(cpal::StreamError) + Send + 'static,
{
    match sample_format {
        cpal::SampleFormat::F32 => {
            let mut processor = StreamProcessor::new(input_rate, channels);
            device
                .build_input_stream(
                    config,
                    move |data: &[f32], _| {
                        let packet = processor.process(data.iter().copied());
                        let _ = sender.send(packet);
                    },
                    error_callback,
                    None,
                )
                .map_err(|error| format!("Failed to create the microphone stream: {error}"))
        }
        cpal::SampleFormat::I16 => {
            let mut processor = StreamProcessor::new(input_rate, channels);
            device
                .build_input_stream(
                    config,
                    move |data: &[i16], _| {
                        let packet = processor.process(
                            data.iter()
                                .map(|sample| f32::from(*sample) / f32::from(i16::MAX)),
                        );
                        let _ = sender.send(packet);
                    },
                    error_callback,
                    None,
                )
                .map_err(|error| format!("Failed to create the microphone stream: {error}"))
        }
        cpal::SampleFormat::U16 => {
            let mut processor = StreamProcessor::new(input_rate, channels);
            device
                .build_input_stream(
                    config,
                    move |data: &[u16], _| {
                        let packet = processor.process(
                            data.iter()
                                .map(|sample| (f32::from(*sample) - 32_768.0) / 32_768.0),
                        );
                        let _ = sender.send(packet);
                    },
                    error_callback,
                    None,
                )
                .map_err(|error| format!("Failed to create the microphone stream: {error}"))
        }
        unsupported => Err(format!(
            "The microphone uses an unsupported sample format: {unsupported:?}"
        )),
    }
}

struct StreamProcessor {
    channels: usize,
    resampler: LinearResampler,
}

impl StreamProcessor {
    fn new(input_rate: u32, channels: usize) -> Self {
        Self {
            channels,
            resampler: LinearResampler::new(input_rate),
        }
    }

    fn process<I>(&mut self, samples: I) -> AudioPacket
    where
        I: Iterator<Item = f32>,
    {
        let mut frame = Vec::with_capacity(self.channels);
        let mut mono = Vec::with_capacity(1_024);
        let mut square_sum = 0_f64;
        let mut frame_count = 0_usize;

        for sample in samples {
            frame.push(sample);
            if frame.len() == self.channels {
                let value = frame.iter().copied().sum::<f32>() / self.channels as f32;
                square_sum += f64::from(value * value);
                frame_count += 1;
                mono.push(value);
                frame.clear();
            }
        }

        let mut output =
            Vec::with_capacity((mono.len() as f64 / self.resampler.ratio).ceil() as usize + 1);
        self.resampler.process(&mono, &mut output);

        let rms = if frame_count == 0 {
            0.0
        } else {
            (square_sum / frame_count as f64).sqrt() as f32
        };
        let decibels = 20.0 * rms.max(1e-7).log10();

        AudioPacket {
            samples: output,
            level: ((decibels - LEVEL_DB_FLOOR) / -LEVEL_DB_FLOOR).clamp(0.0, 1.0),
        }
    }
}

struct LinearResampler {
    ratio: f64,
    position: f64,
    previous: Option<f32>,
}

impl LinearResampler {
    fn new(input_rate: u32) -> Self {
        Self {
            ratio: f64::from(input_rate) / f64::from(wav::SAMPLE_RATE),
            position: 0.0,
            previous: None,
        }
    }

    fn process(&mut self, input: &[f32], output: &mut Vec<i16>) {
        if (self.ratio - 1.0).abs() < f64::EPSILON {
            output.extend(input.iter().copied().map(f32_to_i16));
            return;
        }

        for &current in input {
            let Some(previous) = self.previous else {
                self.previous = Some(current);
                continue;
            };

            while self.position <= 1.0 {
                let value = previous + (current - previous) * self.position as f32;
                output.push(f32_to_i16(value));
                self.position += self.ratio;
            }

            self.position -= 1.0;
            self.previous = Some(current);
        }
    }
}

fn f32_to_i16(value: f32) -> i16 {
    if value <= -1.0 {
        i16::MIN
    } else if value >= 1.0 {
        i16::MAX
    } else {
        (value * 32_768.0).round() as i16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_indexed_device_ids() {
        assert_eq!(
            parse_device_id("12:Studio Microphone").unwrap(),
            (12, "Studio Microphone".to_string())
        );
        assert!(parse_device_id("not-an-index:Microphone").is_err());
        assert!(parse_device_id("missing-separator").is_err());
    }

    #[test]
    fn identity_resampler_preserves_samples() {
        let mut processor = StreamProcessor::new(wav::SAMPLE_RATE, 1);
        let packet = processor.process([-1.0, 0.0, 0.5, 1.0].into_iter());

        assert_eq!(packet.samples, vec![i16::MIN, 0, 16_384, i16::MAX]);
    }

    #[test]
    fn downsampler_produces_half_as_many_samples_for_32khz_input() {
        let mut resampler = LinearResampler::new(32_000);
        let input = vec![0.0; 32];
        let mut output = Vec::new();

        resampler.process(&input, &mut output);

        assert_eq!(output.len(), 16);
    }
}
