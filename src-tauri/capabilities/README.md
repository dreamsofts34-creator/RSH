# Capabilities — default-deny

Tauri 2 capabilities are default-deny: the frontend can only do what a
capability file explicitly permits. This directory holds the RSH capability
files (JSON has no comments, so the reasons live here).

## Granted now (Phase A2)

| Permission | Why the shell needs it |
|---|---|
| `core:default` | The Tauri-provided minimal base set for a single-window app (window, webview, app, event, menu, and tray defaults — what `cargo tauri init` generates). The shell needs it to show its one window, run `ping`/`get_version`, and (from P2.3) own a tray icon. It is a base set, not a wildcard: it grants no file, shell, network, or dialog access. |

## Explicitly NOT granted (later phases add them, one at a time, with a reason)

- `fs` — no file access from the frontend. All file I/O stays in the Rust backend (Phase C/SW-R4).
- `shell` / `opener` — no launching processes or opening folders (game launcher and "open config folder" are Phase E/P9 scope).
- `dialog` — no file pickers (profile import/export is Phase E/P7 scope).
- `updater` — no self-update (Phase H scope; updater signing is mandatory there).
- `store` — no frontend key/value store (settings live in the Rust backend).
- `http` — no frontend network access.
- No wildcard permission (`fs:allow-read-*` or similar) is ever acceptable.
