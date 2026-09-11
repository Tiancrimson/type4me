# Type4Me for Windows

Windows desktop implementation of Type4Me, built with Tauri 2, Rust, React, and
TypeScript.

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
