# Type4Me for Windows

Windows desktop implementation of Type4Me, built with Tauri 2, Rust, React, and
TypeScript.

## Current workflow

1. Select a microphone or use the Windows default input device.
2. Press `Ctrl+Shift+Space` to start recording.
3. Release the shortcut to stop, or use toggle mode to start and stop manually.
4. The recording is saved as 16 kHz mono PCM16 WAV.
5. If an OpenAI API key is configured, the WAV is transcribed through
   `POST /audio/transcriptions`.
6. The transcript is pasted into the previously active Windows application
   through the clipboard and `Ctrl+V`. If no external window is available, or
   pasting fails, the transcript remains on the clipboard.

Recording can also be controlled from the desktop UI. The default shortcut
style is hold-to-talk; toggle mode is available in the shortcut panel.

## OpenAI configuration

The Windows build defaults to `gpt-4o-transcribe` and
`https://api.openai.com/v1`. The API key is stored in Windows Credential
Manager under the target `Type4Me/OpenAI`; it is not written to the application
settings file. Non-sensitive ASR settings are stored in the application data
directory as `asr.json`.

When no API key is configured, recording still works and the WAV file is
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
