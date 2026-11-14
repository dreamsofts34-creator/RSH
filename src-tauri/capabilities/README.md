# Capabilities — default-deny

Tauri 2 capabilities are default-deny: the frontend can only do what a
capability file explicitly permits. This directory holds the RSH capability
files (JSON has no comments, so the reasons live here).

## Granted now (Phase A2)

| Permission | Why the shell needs it |
|---|---|
| `core:default` | The Tauri-provided minimal base set for a single-window app (window, webview, app, event, menu, and tray defaults — what `cargo tauri init` generates). The shell needs it to show its one window, run `ping`/`get_version`, and (from P2.3) own a tray icon. It is a base set, not a wildcard: it grants no file, shell, network, or dialog access. |
| `dialog:default` | M13 (deep-scan Round 2): the Setup firmware step's native file picker (`frontend/src/screens/Setup/FirmwareStep.tsx` calls `open()` from `@tauri-apps/plugin-dialog` 2.8.4; Rust side registered in `src/main.rs` as `tauri_plugin_dialog::init()`). The frontend only ever calls `open()` (single `.bin` file); `dialog:default` is the plugin's default permission set (`allow-open` + `allow-save` + `allow-message` — `ask`/`confirm` are separate permissions and are NOT granted), so a future `save()`/`message()` use would already be permitted and needs its own reason recorded here. No picked path is ever executed: it is only handed to the `validate_firmware_image` / `upload_firmware` commands, which validate it Rust-side. |

## Explicitly NOT granted (later phases add them, one at a time, with a reason)

- `fs` — no file access from the frontend. All file I/O stays in the Rust backend (Phase C/SW-R4).
- `shell` / `opener` — no launching processes or opening folders (game launcher and "open config folder" are Phase E/P9 scope).
- `updater` — no self-update (Phase H scope; updater signing is mandatory there).
- `store` — no frontend key/value store (settings live in the Rust backend).
- `http` — no frontend network access.
- No wildcard permission (`fs:allow-read-*` or similar) is ever acceptable.
