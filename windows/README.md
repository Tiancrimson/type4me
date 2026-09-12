# Type4Me for Windows

Windows desktop implementation of Type4Me, built with Tauri 2, Rust, React, and
TypeScript.

## Branch workflow

Windows development uses `windows` as its long-lived integration branch.
Changes to the Windows implementation are developed on short-lived branches and
merged back into `windows` through pull requests:

```text
upstream/main
    └─ origin/windows
          ├─ feat/windows-audio
          ├─ feat/windows-hotkey
          ├─ feat/windows-injection
          └─ feat/windows-<feature>
```

Use one focused branch per independently reviewable change. Base new branches
on the latest `origin/windows`, open the pull request with `windows` as its base
branch, and delete the short-lived branch after it is merged. Do not use
`upstream/main` as the base for Windows-only work.

## Current workflow

1. Select a microphone or use the Windows default input device.
2. Press the active shortcut, by default `Ctrl+Shift+Space`, to start recording.
3. Release the shortcut to stop, or use toggle mode to start and stop manually.
4. The recording is saved as 16 kHz mono PCM16 WAV.
5. The WAV is transcribed locally with SenseVoice, or through an
   OpenAI-compatible cloud endpoint when a cloud provider is selected.
6. The transcript is pasted into the previously active Windows application
   through the clipboard and `Ctrl+V`. If no external window is available, or
   pasting fails, the transcript remains on the clipboard.

Recording can also be controlled from the desktop UI. The default shortcut
style is hold-to-talk; toggle mode is available in the shortcut panel. The
shortcut can be replaced from the UI and is persisted in the application data
directory as `hotkey.json`.

If another application owns `Ctrl+Shift+Space`, Type4Me tries
`Ctrl+Alt+Space` and then `Ctrl+Shift+F9`. The active shortcut is shown in the
UI. If all combinations are unavailable, the application still starts and
reports the conflict instead of exiting.

WAV files are stored in:

```text
%APPDATA%\com.tiancrimson.type4me\recordings
```

The UI displays this directory and can open it in Windows Explorer.

## Speech recognition configuration

The Windows build supports local and OpenAI-compatible cloud providers:

| Provider | Type | Default model | Default Base URL |
| --- | --- | --- | --- |
| SenseVoice | Local | `sensevoice-small-int8` | Not applicable |
| OpenAI | Cloud | `gpt-4o-transcribe` | `https://api.openai.com/v1` |
| Groq | Cloud | `whisper-large-v3-turbo` | `https://api.groq.com/openai/v1` |
| SiliconFlow | Cloud | `FunAudioLLM/SenseVoiceSmall` | `https://api.siliconflow.cn/v1` |
| Custom | Cloud | `gpt-4o-transcribe` | `https://api.openai.com/v1` |

SenseVoice runs fully offline after its model is downloaded from the settings
panel. It uses the INT8 SenseVoice Small model and supports Chinese, English,
Cantonese, Japanese, and Korean through automatic language detection.

Each cloud provider has a separate API key. The custom option accepts another
OpenAI-compatible Base URL and model. Cloud providers use:

```text
POST {baseUrl}/audio/transcriptions
```

API keys are stored in Windows Credential Manager, are not written to the
application settings file. Non-sensitive ASR settings are stored in the
application data directory as `asr.json`.

When no cloud API key is configured, recording still works and the WAV file is
retained. Without a successful transcription, there is no text to inject.

## Development

```powershell
npm install
npm run windows:dev
```

The wrapper loads the installed Visual Studio C++ build environment before it
starts Tauri. A plain `npm run tauri -- dev` only works when the current shell
already has MSVC on `PATH`.

Run the frontend check independently:

```powershell
npm run build
```

Run the frontend build and Rust unit tests:

```powershell
npm run windows:test
```

Build the Windows executable:

```powershell
npm run windows:build
```

## Structure

| Path | Responsibility |
| --- | --- |
| `src/` | React application shell and user interface |
| `src-tauri/src/` | Rust backend and Tauri commands |
| `src-tauri/tauri.conf.json` | Window, bundle, and application configuration |
| `scripts/tauri.ps1` | Loads MSVC and invokes Tauri |

The macOS implementation remains in the repository root as the behavioral
reference. Windows-specific audio capture, global hotkeys, credential storage,
and text injection belong in `src-tauri/`.
