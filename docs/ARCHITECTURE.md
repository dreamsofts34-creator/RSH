# RSH v2.0 — Architecture

**Status:** v2.0 (Tauri 2 + React is gone) · **App:** Rust + Slint ·
**Firmware:** C++ v4.0.2 · **Protocol:** v2.1 (FROZEN)

This document describes the v2.0 architecture. Sources:
[REDESIGN-PLAN.md](../redesign-plan/RSH-UI-Redesign/REDESIGN-PLAN.md)
§10 and
[LOGIC-FIX-PLAN.md](../redesign-plan/RSH-UI-Redesign/LOGIC-FIX-PLAN.md)
§2 (target architecture) and §4.1 (Connection Manager). Nothing in this
document invents a component that is not in those plans.

## 1. Overview

```
rsh-daemon  (Rust, always running, no web engine)
  ├─ Connection Manager   (state machine, §4)
  ├─ Pairing Manager      (USB-first, pair button, rejoin helper)
  ├─ Engine + Output      (200 Hz, on its own dedicated thread)
  ├─ Profile / Settings / Notifications store
  ├─ Tray icon + tray popup trigger
  └─ Local API server     (named pipe / local socket, JSON lines, versioned)
          ▲  commands (request/response)  +  events (push)
rsh-ui (Slint)  only runs when opened; when it closes, the daemon keeps running
```

The v1.x app was one Tauri process: Rust core + WebView2 + React, with
the engine, the UI and the tray all in the same process. In v2.0 the
work is split in two:

- **The daemon does the work.** It owns the engine, the transport to
  the board, the game output, the tray icon, and automatic profile
  switching. It runs from login (or from first launch) and keeps
  running when the window is closed.
- **The UI only shows the work.** It is a Slint window that opens when
  the user opens it and closes when the user closes it. Closing the
  window never stops the wheel, the output, or the daemon.

## 2. rsh-daemon — always on

The daemon is a plain Rust process. **There is no web engine in it**
(no WebView2, no browser, no JavaScript runtime) — that is the whole
point of the split: background cost stays low because nothing web is
kept alive in the background.

The daemon owns:

| Part | What it does |
|---|---|
| **Connection Manager** | The state machine to the board (§4). Only it decides whether the wheel is connected. |
| **Pairing Manager** | USB-first pairing, the pair-button window, the Wi-Fi password (shown once), and the rejoin helper that joins the PC to the `RSH` network after pairing. |
| **Engine + Output** | The 200 Hz input pipeline and the game output (HIDMaestro / vJoy). The engine runs on **its own dedicated thread** (see §5) — never on the UI thread, never sharing a runtime with the network code. |
| **Profile / Settings / Notifications store** | Profiles (with the draft/save model), settings, and the `NotificationCenter` (persistent notification store behind the bell badge and toasts). |
| **Tray icon + tray popup trigger** | The tray icon exists on **every** launch (the v1.x bug was that it only existed on `--tray` autostart launches). Left-click opens the tray popup; right-click opens the tray menu. |
| **Local API server** | The only way the UI talks to the daemon (§3). |

The daemon is **single-instance** (named mutex). Launching it a second
time does not start a second daemon — it opens the UI window.

### Version — one source

The version comes from one place (the Cargo workspace version) and is
used for the app title, Settings, README and installer. No second copy
of the version string lives anywhere else.

## 3. Local IPC — daemon ↔ UI

The UI and the daemon talk over a **local IPC channel**:

- **Transport:** a named pipe (Windows) / local (Unix) socket.
- **Format:** **JSON lines** (one JSON object per line).
- **Versioned:** the protocol carries a version, so a newer UI and an
  older daemon (or the reverse) can detect a mismatch instead of
  misreading each other.
- **Access:** the pipe is restricted to the current user (current-user
  ACL) — no other user on the machine can drive the wheel through it.

Two message directions travel over the same channel:

- **Commands (request/response):** the UI asks the daemon to do
  something (pair, save a profile, set a tuning value, launch a game)
  and gets a reply. Commands are typed, not strings.
- **Events (push):** the daemon pushes state changes to the UI as they
  happen (connection state, live snapshot, job progress, notifications).
  The UI does **not poll** the daemon — the old 5-second
  `get_pairing_status` poll is gone.

### AppApi trait + AppEvent enum

Above the transport sits one typed boundary, shared by the Slint UI
and the tray:

- **`AppApi` trait** — the commands. Every operation the UI or tray
  can ask for is one method on this trait, with typed arguments and
  typed errors (`UserError`: what happened, what to do). There are no
  raw `String` errors on this boundary.
- **`AppEvent` enum** — the push events. Connection changes
  (`ConnectionChanged{state, reason}`), live snapshots, job progress
  (`Job{id, state, progress, error}`), profile and notification
  events.

The old Tauri event queue (`drain_app_events`) is gone. The UI and the
tray both speak `AppApi` / `AppEvent` and nothing else.

### Live data — push, only while the UI is attached

The daemon pushes a live snapshot at **~30 Hz, only while a UI is
attached**. When no UI is open, no snapshot traffic is produced at
all. The 200 Hz engine stream to the game is unaffected — it is a
daemon-internal path, not an IPC path.

## 4. Connection Manager (L1) — the state machine

The Connection Manager is the single authority on connection state.
The UI status card, the banners, and the setup wizard gating all read
from it — nothing else decides "connected".

```
Idle ─connect→ LinkUp ─HELLO→ Hello ─pair/resume→ Authenticating ─GET_CAPS ok→ Active
  ▲              │ (no HELLO)    │ (window closed / bad tag / no key)   │
  │              └─────────► Error(reason, hint, retry_at) ◄────────────┘
  └──────────────  Backoff(2s→30s)  ◄── Active ─(no frames 2 s / transport lost)
```

Rules (LOGIC-FIX-PLAN §4.1):

- **`Active` only after a signed `GET_CAPS` passes.** The label
  "Connected" is shown only in the `Active` state. The setup wizard
  lets the user continue only from `Active`.
- **Re-activation loop.** A failed activation is not a one-shot: the
  manager reports `Error(reason)` and retries with backoff (2 s →
  30 s). For reasons that need a user action (`NoStoredKey`,
  `WindowClosed`), auto-retry stops and the UI shows a "Pairing
  needed" banner instead of retrying forever.
- **Heartbeat watchdog.** In `Active`, if no stream/status frame
  arrives for **2 seconds**, the transport is rebuilt and the session
  re-activated (covers wheel reboot, AP restart, Wi-Fi drop). UDP has
  no "source lost" signal, so this watchdog is the only detection.
- **HELLO wait is longer than the firmware reply limiter** (700–800 ms,
  3 attempts), so a HELLO reply is not lost to the board's 250 ms
  rate limit.
- **Resume after a restart:** if a resume fails with `BAD_TAG` (the
  board rejects a new session for 10 s after an authenticated one),
  the manager retries once after 10 s and tells the user the wheel
  is busy — it does not fail silently.
- **Typed bind errors.** A UDP bind failure is a typed reason
  (`AddrInUse`, `PermissionDenied`, `NoInterface`) with a UI hint,
  not a swallowed `.ok()`.
- **mDNS:** `rsh.local` is resolved (fallback `192.168.4.1`); the
  address field is not a bare hostname that is never resolved.
- **Every transition** is logged (redacted) and emitted as
  `AppEvent::ConnectionChanged{state, reason}`.

**Acceptance:** wheel reboot, AP restart, Wi-Fi off/on, USB
unplug/replug, app restart — in every case the UI shows the correct
state within 5 seconds and (if paired) returns to `Active` on its
own. There is no state that reads "Connected" while no data flows.

## 5. Threads — blocking work never touches the UI

- **Engine thread (dedicated).** The 200 Hz engine has its own
  thread/runtime. In v1.x the forwarder/activator and the engine
  shared one `current_thread` runtime; in v2.0 they do not.
  Network/IO runs on a separate runtime.
- **Blocking work is an async job, never a UI block.** Pairing,
  firmware upload, config push, wheel restart — all run as jobs with
  progress events (`Job{id, state, progress, error}`). No
  long-running call ever blocks the UI thread (in v1.x, 63 of 64
  Tauri commands were synchronous and `begin_pairing` blocked the UI
  for up to 3 s, `upload_firmware` for up to 60 s).
- **The Slint UI thread only renders and forwards commands.** It
  never does IO, never waits on the network, and never runs the
  engine.

## 6. What v2.0 is not

- **Not Tauri.** There is no WebView2, no React frontend, no Tauri
  command layer. The v1.x frontend (`frontend/`) and Tauri shell
  (`src-tauri/`) are superseded by `rsh-ui` (Slint) + `rsh-daemon`.
- **Not one process.** The engine does not live in the UI process. A
  UI crash cannot stop the wheel, and closing the window cannot stop
  the output.
- **Not a home-network device.** The board is a Wi-Fi access point
  only — it never joins a home network. The daemon talks to the
  board's own `RSH` network (Wi-Fi) or to USB-serial. See
  [USER-GUIDE.md](USER-GUIDE.md#pairing).
- **Not a protocol change.** Protocol v2.1 is frozen. The daemon/UI
  split, the IPC channel, and the firmware v4.0.2 features are all
  above or beside the wire protocol — no wire message changed.

## 7. Component map (plans only)

| Plan component | Lives in | Notes |
|---|---|---|
| Connection Manager (L1) | `rsh-daemon` | §4 |
| Pairing Manager (L2) | `rsh-daemon` | USB-first; 3 s pair button, 120 s window; rejoin helper; password shown once |
| Engine + Output | `rsh-daemon`, own thread | 200 Hz; HIDMaestro / vJoy; lazy UAC start |
| Profile / Settings / Notifications store | `rsh-daemon` | Draft/save model; `NotificationCenter` |
| Tray icon + popup trigger | `rsh-daemon` | Every launch; Slint `SystemTrayIcon` |
| Local API server | `rsh-daemon` | Named pipe / local socket, JSON lines, versioned |
| `AppApi` trait + `AppEvent` enum | Shared boundary | Typed commands + push events |
| Slint UI (`rsh-ui`) | `rsh-ui` | Overview / Wheel / Settings / Setup / tray popup; only when opened |
| Firmware v4.0.2 | ESP32 board | Pair button (F-02), AP lock (F-01), persistent LED (F-14), handbrake button mode (F-15), shifter sensitivity (D-4), horn test (D-5); protocol v2.1 frozen |
| `rsh-app-core` | Shared library | Stays UI-agnostic; `AppApi` sits above it |

Reusable crates from v1.x (unchanged in role): `rsh-protocol`,
`rsh-engine`, `rsh-transport`, `rsh-security`, `rsh-profiles`,
`rsh-telemetry`, `rsh-output-*`, `rsh-app-core`.
