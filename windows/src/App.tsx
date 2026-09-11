import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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
  lastTranscript: string | null;
  lastInjectionOutcome: InjectionOutcome | null;
  lastError: string | null;
  hotkeyStyle: HotkeyStyle;
  hotkey: string;
};

type AsrSettings = {
  model: string;
  baseUrl: string;
  apiKeyConfigured: boolean;
  apiKeyHint: string | null;
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

const phaseHints: Record<RecordingPhase, string> = {
  idle: "选择麦克风后开始录音",
  starting: "正在连接音频设备",
  recording: "松开快捷键后自动识别",
  stopping: "正在完成本次录音",
  cancelling: "正在放弃本次录音",
  transcribing: "正在请求 OpenAI 语音识别",
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

function App() {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [recording, setRecording] = useState<RecordingState | null>(null);
  const [asrSettings, setAsrSettings] = useState<AsrSettings | null>(null);
  const [devices, setDevices] = useState<AudioDeviceInfo[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("gpt-4o-transcribe");
  const [baseUrl, setBaseUrl] = useState("https://api.openai.com/v1");
  const [level, setLevel] = useState(0);
  const [elapsedMs, setElapsedMs] = useState(0);
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
      const [status, nextDevices, state, nextAsrSettings] = await Promise.all([
        invoke<RuntimeStatus>("get_runtime_status"),
        invoke<AudioDeviceInfo[]>("list_audio_devices"),
        invoke<RecordingState>("get_recording_state"),
        invoke<AsrSettings>("get_asr_settings"),
      ]);

      setRuntime(status);
      setDevices(nextDevices);
      applyRecordingState(state);
      applyAsrSettings(nextAsrSettings);
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
    await runRecordingAction("hotkey", () =>
      invoke<RecordingState>("set_hotkey_style", { style }),
    );
  };

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
          apiKey: apiKey.trim() || null,
          model,
          baseUrl,
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
      await invoke("clear_openai_api_key");
      applyAsrSettings(await invoke<AsrSettings>("get_asr_settings"));
      setApiKey("");
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
            <strong>{recording?.hotkey ?? "Ctrl+Shift+Space"}</strong>
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
              <kbd>{recording?.hotkey ?? "Ctrl+Shift+Space"}</kbd>
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

            <p className="panel-note">
              {recording?.hotkeyStyle === "hold"
                ? "按下快捷键开始录音，松开立即停止。"
                : "每次按下快捷键，在开始和停止之间切换。"}
            </p>
          </article>

          <article className="control-panel settings-panel">
            <div className="panel-heading">
              <div>
                <span className="section-label">OPENAI</span>
                <h2>语音识别</h2>
              </div>
              <span
                className="settings-state"
                data-configured={asrSettings?.apiKeyConfigured === true}
              >
                <span />
                {asrSettings?.apiKeyConfigured ? "Key 已配置" : "需要 API Key"}
              </span>
            </div>

            <div className="settings-form">
              <label className="field">
                <span className="field-label">识别模型</span>
                <select
                  value={model}
                  onChange={(event) => setModel(event.target.value)}
                  disabled={!isIdle || isBusy}
                >
                  <option value="gpt-4o-transcribe">
                    gpt-4o-transcribe（推荐）
                  </option>
                  <option value="gpt-4o-mini-transcribe">
                    gpt-4o-mini-transcribe
                  </option>
                  <option value="whisper-1">whisper-1</option>
                </select>
              </label>

              <label className="field">
                <span className="field-label">API 地址</span>
                <input
                  type="url"
                  value={baseUrl}
                  onChange={(event) => setBaseUrl(event.target.value)}
                  spellCheck={false}
                  disabled={!isIdle || isBusy}
                />
              </label>

              <label className="field">
                <span className="field-label">OpenAI API Key</span>
                <input
                  type="password"
                  value={apiKey}
                  onChange={(event) => setApiKey(event.target.value)}
                  placeholder={
                    asrSettings?.apiKeyConfigured
                      ? (asrSettings.apiKeyHint ?? "已保存")
                      : "sk-..."
                  }
                  autoComplete="off"
                  spellCheck={false}
                  disabled={!isIdle || isBusy}
                />
              </label>

              <div className="settings-actions">
                <button
                  className="primary-button"
                  type="button"
                  onClick={() => void handleSaveAsrSettings()}
                  disabled={!isIdle || isBusy}
                >
                  {pendingAction === "asr-save" ? "保存中" : "保存设置"}
                </button>
                <button
                  className="text-button"
                  type="button"
                  onClick={() => void handleClearApiKey()}
                  disabled={
                    !isIdle || isBusy || !asrSettings?.apiKeyConfigured
                  }
                >
                  {pendingAction === "asr-clear" ? "清除中" : "清除 Key"}
                </button>
              </div>
            </div>

            <p className="panel-note">
              API Key 仅保存在 Windows 凭据管理器。未配置时仍会保存 WAV
              文件。
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
                <span className="field-label">录音文件</span>
                {recording?.lastRecordingPath ? (
                  <div
                    className="output-path"
                    title={recording.lastRecordingPath}
                  >
                    <span className="output-icon" />
                    <span>{recording.lastRecordingPath}</span>
                  </div>
                ) : (
                  <p className="result-empty">尚未生成录音文件</p>
                )}
              </section>
            </div>
          </article>
        </section>
      </main>
    </div>
  );
}

export default App;
