import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useState } from "react";
import "./App.css";

type RuntimeStatus = {
  appName: string;
  version: string;
  platform: string;
  architecture: string;
  developmentBuild: boolean;
  backendConnected: boolean;
};

type AudioDeviceInfo = {
  id: string;
  name: string;
  isDefault: boolean;
};

type HotkeyStyle = "hold" | "toggle";

type AsrProvider =
  | "openai"
  | "groq"
  | "siliconflow"
  | "sensevoice"
  | "custom";

type RecordingPhase =
  | "idle"
  | "starting"
  | "recording"
  | "stopping"
  | "cancelling"
  | "transcribing"
  | "injecting";

type InjectionOutcome = "inserted" | "copiedToClipboard";

type RecordingState = {
  phase: RecordingPhase;
  selectedDeviceId: string | null;
  selectedDeviceName: string | null;
  elapsedMs: number;
  level: number;
  lastRecordingPath: string | null;
  recordingsDirectory: string | null;
  lastTranscript: string | null;
  lastInjectionOutcome: InjectionOutcome | null;
  lastError: string | null;
  hotkeyStyle: HotkeyStyle;
  hotkey: string;
  hotkeyLabel: string;
  hotkeyRegistered: boolean;
  hotkeyMessage: string | null;
};

type AsrSettings = {
  provider: AsrProvider;
  model: string;
  baseUrl: string;
  settingsSaved: boolean;
  apiKeyConfigured: boolean;
  apiKeyHint: string | null;
  requiresApiKey: boolean;
};

type SenseVoiceModelStatus = {
  installed: boolean;
  downloadInProgress: boolean;
  downloadedBytes: number;
  totalBytes: number;
  progress: number;
  modelDirectory: string | null;
  modelSizeBytes: number;
  error: string | null;
};

type AudioLevelEvent = {
  level: number;
  elapsedMs: number;
};

const phaseLabels: Record<RecordingPhase, string> = {
  idle: "待机",
  starting: "正在启动",
  recording: "录音中",
  stopping: "正在停止",
  cancelling: "正在取消",
  transcribing: "正在识别",
  injecting: "正在输入",
};

const asrProviderPresets: Record<
  AsrProvider,
  {
    label: string;
    kind: "cloud" | "local";
    keyLabel: string;
    keyUrl?: string;
    model: string;
    models: string[];
    baseUrl: string;
    keyPlaceholder: string;
  }
> = {
  openai: {
    label: "OpenAI",
    kind: "cloud",
    keyLabel: "OpenAI",
    keyUrl: "https://platform.openai.com/api-keys",
    model: "gpt-4o-transcribe",
    models: ["gpt-4o-transcribe", "gpt-4o-mini-transcribe", "whisper-1"],
    baseUrl: "https://api.openai.com/v1",
    keyPlaceholder: "sk-...",
  },
  groq: {
    label: "Groq",
    kind: "cloud",
    keyLabel: "Groq",
    keyUrl: "https://console.groq.com/keys",
    model: "whisper-large-v3-turbo",
    models: [
      "whisper-large-v3-turbo",
      "whisper-large-v3",
      "distil-whisper-large-v3-en",
    ],
    baseUrl: "https://api.groq.com/openai/v1",
    keyPlaceholder: "gsk_...",
  },
  siliconflow: {
    label: "SiliconFlow",
    kind: "cloud",
    keyLabel: "SiliconFlow",
    keyUrl: "https://cloud.siliconflow.cn/account/ak",
    model: "FunAudioLLM/SenseVoiceSmall",
    models: ["FunAudioLLM/SenseVoiceSmall", "TeleAI/TeleSpeechASR"],
    baseUrl: "https://api.siliconflow.cn/v1",
    keyPlaceholder: "sk-...",
  },
  sensevoice: {
    label: "SenseVoice（本地）",
    kind: "local",
    keyLabel: "本地模型",
    model: "sensevoice-small-int8",
    models: ["sensevoice-small-int8"],
    baseUrl: "",
    keyPlaceholder: "",
  },
  custom: {
    label: "自定义 OpenAI 兼容",
    kind: "cloud",
    keyLabel: "服务商",
    model: "gpt-4o-transcribe",
    models: ["gpt-4o-transcribe", "whisper-1"],
    baseUrl: "https://api.openai.com/v1",
    keyPlaceholder: "API Key",
  },
};

const phaseHints: Record<RecordingPhase, string> = {
  idle: "选择麦克风后开始录音",
  starting: "正在连接音频设备",
  recording: "松开快捷键后自动识别",
  stopping: "正在完成本次录音",
  cancelling: "正在放弃本次录音",
  transcribing: "正在请求语音识别",
  injecting: "正在粘贴识别结果",
};

const injectionLabels: Record<InjectionOutcome, string> = {
  inserted: "已粘贴到当前应用",
  copiedToClipboard: "已复制到剪贴板",
};

function formatError(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function formatDuration(milliseconds: number) {
  const safeMilliseconds = Math.max(0, milliseconds);
  const minutes = Math.floor(safeMilliseconds / 60_000);
  const seconds = Math.floor((safeMilliseconds % 60_000) / 1_000);
  const tenths = Math.floor((safeMilliseconds % 1_000) / 100);

  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(
    2,
    "0",
  )}.${tenths}`;
}

function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 MB";
  }

  return `${(bytes / 1024 / 1024).toFixed(bytes >= 100 * 1024 * 1024 ? 0 : 1)} MB`;
}

function shortcutFromKeyboardEvent(event: KeyboardEvent) {
  const modifierKeys = new Set([
    "AltLeft",
    "AltRight",
    "ControlLeft",
    "ControlRight",
    "MetaLeft",
    "MetaRight",
    "ShiftLeft",
    "ShiftRight",
  ]);
  const modifiers = [
    event.ctrlKey && "control",
    event.altKey && "alt",
    event.shiftKey && "shift",
    event.metaKey && "super",
  ].filter((modifier): modifier is string => Boolean(modifier));
  const key = event.code || event.key;

  if (!modifiers.length) {
    return {
      error: "快捷键至少需要包含 Ctrl、Alt、Shift 或 Win 中的一个修饰键。",
    };
  }
  if (!key || modifierKeys.has(key)) {
    return { shortcut: null, error: null };
  }

  return { shortcut: [...modifiers, key].join("+"), error: null };
}

function App() {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [recording, setRecording] = useState<RecordingState | null>(null);
  const [asrSettings, setAsrSettings] = useState<AsrSettings | null>(null);
  const [modelStatus, setModelStatus] =
    useState<SenseVoiceModelStatus | null>(null);
  const [launchAtStartup, setLaunchAtStartup] = useState(false);
  const [devices, setDevices] = useState<AudioDeviceInfo[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [provider, setProvider] = useState<AsrProvider>("openai");
  const [model, setModel] = useState("gpt-4o-transcribe");
  const [baseUrl, setBaseUrl] = useState("https://api.openai.com/v1");
  const [level, setLevel] = useState(0);
  const [elapsedMs, setElapsedMs] = useState(0);
  const [isCapturingHotkey, setIsCapturingHotkey] = useState(false);
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [commandError, setCommandError] = useState<string | null>(null);

  const applyRecordingState = useCallback((next: RecordingState) => {
    setRecording(next);
    setSelectedDeviceId(next.selectedDeviceId ?? "");
    setLevel(next.level);
    setElapsedMs(next.elapsedMs);
    setCommandError(next.lastError);
  }, []);

  const applyAsrSettings = useCallback((next: AsrSettings) => {
    setAsrSettings(next);
    setProvider(next.provider);
    setModel(next.model);
    setBaseUrl(next.baseUrl);
    setApiKey("");
  }, []);

  const loadDevices = useCallback(async () => {
    const nextDevices = await invoke<AudioDeviceInfo[]>("list_audio_devices");
    setDevices(nextDevices);
    return nextDevices;
  }, []);

  const bootstrap = useCallback(async () => {
    setPendingAction("bootstrap");
    setCommandError(null);

    try {
      const [
        status,
        nextDevices,
        state,
        nextAsrSettings,
        nextModelStatus,
        launchAtStartupEnabled,
      ] = await Promise.all([
        invoke<RuntimeStatus>("get_runtime_status"),
        invoke<AudioDeviceInfo[]>("list_audio_devices"),
        invoke<RecordingState>("get_recording_state"),
        invoke<AsrSettings>("get_asr_settings"),
        invoke<SenseVoiceModelStatus>("get_sensevoice_model_status"),
        invoke<boolean>("get_launch_at_startup"),
      ]);

      setRuntime(status);
      setDevices(nextDevices);
      applyRecordingState(state);
      applyAsrSettings(nextAsrSettings);
      setModelStatus(nextModelStatus);
      setLaunchAtStartup(launchAtStartupEnabled);
    } catch (error) {
      setCommandError(formatError(error));
    } finally {
      setPendingAction(null);
    }
  }, [applyAsrSettings, applyRecordingState]);

  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    const attach = async () => {
      const attached = await Promise.all([
        listen<RecordingState>("recording-state", (event) => {
          applyRecordingState(event.payload);
        }),
        listen<AudioLevelEvent>("audio-level", (event) => {
          setLevel(event.payload.level);
          setElapsedMs(event.payload.elapsedMs);
        }),
        listen<SenseVoiceModelStatus>("sensevoice-model-status", (event) => {
          setModelStatus(event.payload);
        }),
      ]);

      if (disposed) {
        attached.forEach((unlisten) => unlisten());
      } else {
        unlisteners.push(...attached);
      }
    };

    void attach().catch((error) => setCommandError(formatError(error)));

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [applyRecordingState]);

  const phase = recording?.phase ?? "idle";
  const isIdle = phase === "idle";
  const isActive = phase === "starting" || phase === "recording";
  const isBusy = pendingAction !== null;
  const selectedDevice = useMemo(
    () => devices.find((device) => device.id === selectedDeviceId),
    [devices, selectedDeviceId],
  );
  const providerPreset = asrProviderPresets[provider];
  const providerSettingsSaved =
    asrSettings?.settingsSaved === true && asrSettings.provider === provider;
  const selectedProviderKeyConfigured =
    providerSettingsSaved && asrSettings?.apiKeyConfigured === true;
  const selectedProviderReady =
    providerPreset.kind === "local"
      ? providerSettingsSaved && modelStatus?.installed === true
      : selectedProviderKeyConfigured;
  const senseVoiceInstalled = modelStatus?.installed === true;
  const senseVoiceDownloading = modelStatus?.downloadInProgress === true;
  const senseVoiceProgress = Math.round((modelStatus?.progress ?? 0) * 100);

  const runRecordingAction = async (
    action: string,
    command: () => Promise<RecordingState>,
  ) => {
    setPendingAction(action);
    setCommandError(null);

    try {
      applyRecordingState(await command());
    } catch (error) {
      setCommandError(formatError(error));
    } finally {
      setPendingAction(null);
    }
  };

  const handleDeviceChange = async (deviceId: string) => {
    setSelectedDeviceId(deviceId);
    await runRecordingAction("device", () =>
      invoke<RecordingState>("select_audio_device", {
        deviceId: deviceId || null,
      }),
    );
  };

  const handleHotkeyStyle = async (style: HotkeyStyle) => {
    await runRecordingAction("hotkey-style", () =>
      invoke<RecordingState>("set_hotkey_style", { style }),
    );
  };

  useEffect(() => {
    if (!isCapturingHotkey) {
      return;
    }

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        setIsCapturingHotkey(false);
        return;
      }

      const result = shortcutFromKeyboardEvent(event);
      event.preventDefault();
      event.stopPropagation();

      if (result.error) {
        setCommandError(result.error);
        setIsCapturingHotkey(false);
        return;
      }
      if (!result.shortcut) {
        return;
      }

      setIsCapturingHotkey(false);
      void runRecordingAction("hotkey-update", () =>
        invoke<RecordingState>("update_hotkey", {
          shortcut: result.shortcut,
        }),
      );
    };

    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [isCapturingHotkey, runRecordingAction]);

  const handleResetHotkey = () =>
    runRecordingAction("hotkey-reset", () =>
      invoke<RecordingState>("reset_hotkey"),
    );

  const handleStart = () =>
    runRecordingAction("start", () =>
      invoke<RecordingState>("start_recording", {
        deviceId: selectedDeviceId || null,
      }),
    );

  const handleStop = () =>
    runRecordingAction("stop", () =>
      invoke<RecordingState>("stop_recording"),
    );

  const handleCancel = () =>
    runRecordingAction("cancel", () =>
      invoke<RecordingState>("cancel_recording"),
    );

  const handleRefresh = async () => {
    setPendingAction("devices");
    setCommandError(null);

    try {
      const nextDevices = await loadDevices();
      const stillAvailable =
        !selectedDeviceId ||
        nextDevices.some((device) => device.id === selectedDeviceId);

      if (!stillAvailable) {
        await handleDeviceChange("");
      }
    } catch (error) {
      setCommandError(formatError(error));
    } finally {
      setPendingAction(null);
    }
  };

  const handleSaveAsrSettings = async () => {
    setPendingAction("asr-save");
    setCommandError(null);

    try {
      const next = await invoke<AsrSettings>("save_asr_settings", {
        settings: {
          provider,
          apiKey: apiKey.trim() || null,
          model: model.trim(),
          baseUrl: baseUrl.trim(),
        },
      });
      applyAsrSettings(next);
    } catch (error) {
      setCommandError(formatError(error));
    } finally {
      setPendingAction(null);
    }
  };

  const handleClearApiKey = async () => {
    setPendingAction("asr-clear");
    setCommandError(null);

    try {
      await invoke("clear_asr_api_key", { provider });
      applyAsrSettings(await invoke<AsrSettings>("get_asr_settings"));
      setApiKey("");
    } catch (error) {
      setCommandError(formatError(error));
    } finally {
      setPendingAction(null);
    }
  };

  const handleProviderChange = (nextProvider: AsrProvider) => {
    const preset = asrProviderPresets[nextProvider];
    const savedSettings =
      asrSettings?.provider === nextProvider ? asrSettings : null;

    setProvider(nextProvider);
    setModel(savedSettings?.model ?? preset.model);
    setBaseUrl(
      preset.kind === "local" ? "" : (savedSettings?.baseUrl ?? preset.baseUrl),
    );
    setApiKey("");
    setCommandError(null);
  };

  const handleDownloadSenseVoiceModel = async () => {
    setPendingAction("sensevoice-download");
    setCommandError(null);

    try {
      setModelStatus(
        await invoke<SenseVoiceModelStatus>("download_sensevoice_model"),
      );
    } catch (error) {
      setCommandError(formatError(error));
    } finally {
      setPendingAction(null);
    }
  };

  const handleOpenApiKeyPage = async (url?: string) => {
    if (!url) {
      return;
    }

    try {
      await openUrl(url);
    } catch (error) {
      setCommandError(formatError(error));
    }
  };

  const handleOpenRecordingsFolder = async () => {
    setPendingAction("open-recordings");
    setCommandError(null);

    try {
      await invoke("open_recordings_folder");
    } catch (error) {
      setCommandError(formatError(error));
    } finally {
      setPendingAction(null);
    }
  };

  const handleLaunchAtStartupChange = async (enabled: boolean) => {
    setPendingAction("startup");
    setCommandError(null);

    try {
      setLaunchAtStartup(
        await invoke<boolean>("set_launch_at_startup", { enabled }),
      );
    } catch (error) {
      setCommandError(formatError(error));
    } finally {
      setPendingAction(null);
    }
  };

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">4</span>
          <span className="brand-name">Type4Me</span>
        </div>

        <nav className="sidebar-nav" aria-label="应用信息">
          <div className="sidebar-block">
            <span className="sidebar-label">平台</span>
            <strong>Windows</strong>
          </div>
          <div className="sidebar-block">
            <span className="sidebar-label">录音状态</span>
            <strong>{phaseLabels[phase]}</strong>
          </div>
          <div className="sidebar-block">
            <span className="sidebar-label">全局快捷键</span>
            <strong>{recording?.hotkeyLabel ?? "Ctrl+Shift+Space"}</strong>
            {recording?.hotkeyRegistered === false && <span>不可用</span>}
          </div>
        </nav>

        <div className="sidebar-note">
          <span
            className="status-dot"
            data-online={runtime?.backendConnected === true}
          />
          <span>Rust 音频后端</span>
          <span className="sidebar-version">v{runtime?.version ?? "..."}</span>
        </div>
      </aside>

      <main className="workspace">
        <header className="page-header">
          <div>
            <p className="eyebrow">Windows / Audio capture</p>
            <h1>语音录音</h1>
          </div>
          <span className="phase-badge" data-phase={phase}>
            <span />
            {phaseLabels[phase]}
          </span>
        </header>

        {commandError && (
          <div className="error-banner" role="alert">
            <strong>操作未完成</strong>
            <span>{commandError}</span>
          </div>
        )}

        <section className="capture-band" aria-label="录音控制">
          <div className="capture-readout">
            <span className="section-label">REC</span>
            <strong>{formatDuration(elapsedMs)}</strong>
            <span className="capture-hint">{phaseHints[phase]}</span>
          </div>

          <div className="level-panel" aria-label="实时输入电平">
            <div className="level-heading">
              <span>输入电平</span>
              <span>{Math.round(level * 100)}%</span>
            </div>
            <div className="level-track">
              <span style={{ width: `${Math.round(level * 100)}%` }} />
            </div>
            <div className="level-scale" aria-hidden="true">
              <span>-50 dB</span>
              <span>0 dB</span>
            </div>
          </div>

          <div className="capture-actions">
            <button
              className="record-button"
              type="button"
              onClick={() => void handleStart()}
              disabled={!isIdle || isBusy}
            >
              <span className="record-icon" />
              {pendingAction === "start" ? "启动中" : "开始录音"}
            </button>
            <button
              className="stop-button"
              type="button"
              onClick={() => void handleStop()}
              disabled={!isActive || isBusy}
            >
              <span className="stop-icon" />
              {pendingAction === "stop" ? "停止中" : "停止并保存"}
            </button>
            <button
              className="cancel-button"
              type="button"
              onClick={() => void handleCancel()}
              disabled={!isActive || isBusy}
              aria-label="取消当前录音"
              title="取消当前录音"
            >
              <span aria-hidden="true">×</span>
            </button>
          </div>
        </section>

        <section className="control-grid">
          <article className="control-panel">
            <div className="panel-heading">
              <div>
                <span className="section-label">INPUT</span>
                <h2>麦克风</h2>
              </div>
              <button
                className="text-button"
                type="button"
                onClick={() => void handleRefresh()}
                disabled={!isIdle || isBusy}
              >
                {pendingAction === "devices" ? "扫描中" : "重新扫描"}
              </button>
            </div>

            <label className="field-label" htmlFor="microphone">
              录音设备
            </label>
            <select
              id="microphone"
              value={selectedDeviceId}
              onChange={(event) => void handleDeviceChange(event.target.value)}
              disabled={!isIdle || isBusy}
            >
              <option value="">系统默认麦克风</option>
              {devices.map((device) => (
                <option key={device.id} value={device.id}>
                  {device.name}
                  {device.isDefault ? "（默认）" : ""}
                </option>
              ))}
            </select>

            <div className="device-summary">
              <span className="device-indicator" />
              <div>
                <strong>
                  {selectedDevice?.name ??
                    recording?.selectedDeviceName ??
                    "尚未选择"}
                </strong>
                <span>
                  {devices.length > 0
                    ? `检测到 ${devices.length} 个输入设备`
                    : "未检测到输入设备"}
                </span>
              </div>
            </div>
          </article>

          <article className="control-panel">
            <div className="panel-heading">
              <div>
                <span className="section-label">HOTKEY</span>
                <h2>全局快捷键</h2>
              </div>
              <div className="hotkey-meta">
                <kbd>{recording?.hotkeyLabel ?? "Ctrl+Shift+Space"}</kbd>
                <span
                  className="hotkey-state"
                  data-registered={recording?.hotkeyRegistered === true}
                >
                  {recording?.hotkeyRegistered ? "已启用" : "不可用"}
                </span>
              </div>
            </div>

            <span className="field-label">触发方式</span>
            <div
              className="segmented-control"
              role="group"
              aria-label="快捷键模式"
            >
              <button
                type="button"
                aria-pressed={recording?.hotkeyStyle === "hold"}
                onClick={() => void handleHotkeyStyle("hold")}
                disabled={!isIdle || isBusy}
              >
                按住说话
              </button>
              <button
                type="button"
                aria-pressed={recording?.hotkeyStyle !== "hold"}
                onClick={() => void handleHotkeyStyle("toggle")}
                disabled={!isIdle || isBusy}
              >
                按一次切换
              </button>
            </div>

            <div className="hotkey-actions">
              <button
                className="secondary-button"
                type="button"
                aria-pressed={isCapturingHotkey}
                onClick={() =>
                  setIsCapturingHotkey((currentState) => !currentState)
                }
                disabled={!isIdle || isBusy}
              >
                {pendingAction === "hotkey-update"
                  ? "正在更新"
                  : isCapturingHotkey
                    ? "请按下新组合"
                    : "录制新快捷键"}
              </button>
              <button
                className="text-button"
                type="button"
                onClick={() => void handleResetHotkey()}
                disabled={!isIdle || isBusy}
              >
                {pendingAction === "hotkey-reset" ? "恢复中" : "恢复默认"}
              </button>
            </div>

            <p
              className="panel-note"
              data-warning={recording?.hotkeyRegistered === false}
            >
              {isCapturingHotkey
                ? "请按下包含 Ctrl、Alt、Shift 或 Win 的组合键，按 Esc 取消。"
                : (recording?.hotkeyMessage ??
                  (recording?.hotkeyStyle === "hold"
                    ? "按下快捷键开始录音，松开立即停止。"
                    : "每次按下快捷键，在开始和停止之间切换。"))}
            </p>
          </article>

          <article className="control-panel settings-panel">
            <div className="panel-heading">
              <div>
                <span className="section-label">ASR</span>
                <h2>语音识别</h2>
              </div>
              <span
                className="settings-state"
                data-configured={selectedProviderReady}
              >
                <span />
                {providerPreset.kind === "local"
                  ? !providerSettingsSaved
                    ? "尚未保存"
                    : senseVoiceInstalled
                      ? "本地模型已就绪"
                      : senseVoiceDownloading
                        ? "正在下载模型"
                        : "需要下载模型"
                  : selectedProviderKeyConfigured
                    ? "API Key 已配置"
                    : providerSettingsSaved
                      ? "需要 API Key"
                      : "尚未保存"}
              </span>
            </div>

            <div className="settings-form">
              <label className="field">
                <span className="field-label">服务商</span>
                <select
                  value={provider}
                  onChange={(event) =>
                    handleProviderChange(event.target.value as AsrProvider)
                  }
                  disabled={!isIdle || isBusy}
                >
                  {Object.entries(asrProviderPresets).map(
                    ([providerId, preset]) => (
                      <option key={providerId} value={providerId}>
                        {preset.label}
                      </option>
                    ),
                  )}
                </select>
              </label>

              {providerPreset.kind === "local" ? (
                <div className="local-model-summary">
                  <div className="local-model-heading">
                    <div>
                      <strong>SenseVoice Small</strong>
                      <span>中、英、粤、日、韩，本地离线识别</span>
                    </div>
                    <span
                      className="local-model-badge"
                      data-ready={senseVoiceInstalled}
                    >
                      {senseVoiceInstalled
                        ? "已就绪"
                        : senseVoiceDownloading
                          ? `${senseVoiceProgress}%`
                          : "未下载"}
                    </span>
                  </div>

                  <div
                    className="model-progress"
                    data-active={senseVoiceDownloading}
                  >
                    <span
                      style={{
                        width: `${senseVoiceDownloading ? senseVoiceProgress : senseVoiceInstalled ? 100 : 0}%`,
                      }}
                    />
                  </div>
                  <div className="model-meta">
                    <span>
                      {senseVoiceInstalled
                        ? `模型大小 ${formatBytes(modelStatus?.modelSizeBytes ?? 0)}`
                        : `下载大小 ${formatBytes(modelStatus?.totalBytes ?? 0)}`}
                    </span>
                    <span>
                      {senseVoiceDownloading
                        ? `${formatBytes(modelStatus?.downloadedBytes ?? 0)} / ${formatBytes(modelStatus?.totalBytes ?? 0)}`
                        : "首次使用时下载"}
                    </span>
                  </div>

                  <button
                    className="primary-button"
                    type="button"
                    onClick={() => void handleDownloadSenseVoiceModel()}
                    disabled={
                      !isIdle ||
                      isBusy ||
                      senseVoiceInstalled ||
                      senseVoiceDownloading
                    }
                  >
                    {senseVoiceDownloading
                      ? "正在下载"
                      : senseVoiceInstalled
                        ? "模型已就绪"
                        : "下载本地模型"}
                  </button>

                  {modelStatus?.error && (
                    <p className="inline-error">{modelStatus.error}</p>
                  )}
                </div>
              ) : (
                <>
                  <label className="field">
                    <span className="field-label">识别模型</span>
                    <input
                      list="asr-model-options"
                      value={model}
                      onChange={(event) => setModel(event.target.value)}
                      spellCheck={false}
                      disabled={!isIdle || isBusy}
                    />
                    <datalist id="asr-model-options">
                      {providerPreset.models.map((modelId) => (
                        <option key={modelId} value={modelId} />
                      ))}
                    </datalist>
                  </label>

                  <label className="field">
                    <span className="field-label">
                      API 地址（OpenAI 兼容 Base URL）
                    </span>
                    <input
                      type="url"
                      value={baseUrl}
                      onChange={(event) => setBaseUrl(event.target.value)}
                      spellCheck={false}
                      disabled={!isIdle || isBusy}
                    />
                  </label>

                  <label className="field">
                    <span className="field-label-row">
                      <span>{providerPreset.keyLabel} API Key</span>
                      {providerPreset.keyUrl && (
                        <button
                          className="link-button"
                          type="button"
                          onClick={() =>
                            void handleOpenApiKeyPage(providerPreset.keyUrl)
                          }
                        >
                          获取 API Key
                        </button>
                      )}
                    </span>
                    <input
                      type="password"
                      value={apiKey}
                      onChange={(event) => setApiKey(event.target.value)}
                      placeholder={
                        selectedProviderKeyConfigured
                          ? (asrSettings?.apiKeyHint ?? "已保存")
                          : providerPreset.keyPlaceholder
                      }
                      autoComplete="off"
                      spellCheck={false}
                      disabled={!isIdle || isBusy}
                    />
                  </label>
                </>
              )}

              <div className="settings-actions">
                <button
                  className="primary-button"
                  type="button"
                  onClick={() => void handleSaveAsrSettings()}
                  disabled={!isIdle || isBusy}
                >
                  {pendingAction === "asr-save" ? "保存中" : "保存设置"}
                </button>
                {providerPreset.kind === "cloud" && (
                  <button
                    className="text-button"
                    type="button"
                    onClick={() => void handleClearApiKey()}
                    disabled={
                      !isIdle || isBusy || !selectedProviderKeyConfigured
                    }
                  >
                    {pendingAction === "asr-clear" ? "清除中" : "清除 Key"}
                  </button>
                )}
              </div>
            </div>

            <p className="panel-note">
              {providerPreset.kind === "local"
                ? "模型文件保存在本机，录音无需上传到云端。首次识别前需要下载约 156 MB 的量化模型。"
                : "云服务通过 OpenAI 兼容的音频转写接口调用。API Key 分别保存在 Windows 凭据管理器中；未配置时仍会保存 WAV 文件。"}
            </p>
          </article>

          <article className="control-panel output-panel">
            <div className="panel-heading">
              <div>
                <span className="section-label">OUTPUT</span>
                <h2>识别与输出</h2>
              </div>
              <span className="output-status">
                {recording?.lastInjectionOutcome
                  ? injectionLabels[recording.lastInjectionOutcome]
                  : "等待识别"}
              </span>
            </div>

            <div className="result-stack">
              <section className="result-section">
                <span className="field-label">识别结果</span>
                {recording?.lastTranscript ? (
                  <p className="transcript-text">
                    {recording.lastTranscript}
                  </p>
                ) : (
                  <p className="result-empty">尚未生成识别结果</p>
                )}
              </section>

              <section className="result-section">
                <div className="result-heading">
                  <span className="field-label">录音文件</span>
                  <button
                    className="text-button"
                    type="button"
                    onClick={() => void handleOpenRecordingsFolder()}
                    disabled={isBusy}
                  >
                    {pendingAction === "open-recordings"
                      ? "正在打开"
                      : "打开录音文件夹"}
                  </button>
                </div>
                <div
                  className="output-path"
                  title={recording?.recordingsDirectory ?? undefined}
                >
                  <span className="output-icon" />
                  <span>
                    {recording?.recordingsDirectory ?? "正在解析录音目录"}
                  </span>
                </div>
                <p
                  className="last-recording-path"
                  title={recording?.lastRecordingPath ?? undefined}
                >
                  {recording?.lastRecordingPath
                    ? `最近文件：${recording.lastRecordingPath}`
                    : "本次尚未生成录音文件"}
                </p>
              </section>
            </div>
          </article>

          <article className="control-panel system-panel">
            <div className="panel-heading">
              <div>
                <span className="section-label">SYSTEM</span>
                <h2>后台运行 / Background</h2>
              </div>
              <span
                className="settings-state"
                data-configured={launchAtStartup}
              >
                <span />
                {launchAtStartup
                  ? "开机启动已开启 / Startup On"
                  : "仅手动启动 / Manual Start"}
              </span>
            </div>

            <label className="switch-control">
              <input
                type="checkbox"
                role="switch"
                checked={launchAtStartup}
                onChange={(event) =>
                  void handleLaunchAtStartupChange(event.target.checked)
                }
                disabled={isBusy}
              />
              <span className="switch-track" aria-hidden="true">
                <span />
              </span>
              <span className="switch-copy">
                <strong>登录 Windows 后自动启动</strong>
                <span>Launch Type4Me after you sign in</span>
              </span>
            </label>

            <p className="panel-note">
              关闭主窗口后，Type4Me 会继续在系统托盘运行。需要完全退出时，请右键单击托盘图标并选择“退出”。
              Closing the window keeps Type4Me running in the system
              tray. Choose Quit from the tray menu to exit completely.
            </p>
          </article>
        </section>
      </main>
    </div>
  );
}

export default App;
