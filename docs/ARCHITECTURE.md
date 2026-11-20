# RSH v2.0 — Architecture

**Status:** v2.0 (Tauri 2 + React is gone) · **App:** Rust + Slint ·
**Firmware:** C++ v4.0.3 · **Protocol:** v2.2 (FROZEN)

This document describes the v2.0 architecture. Sources: the v2.0
redesign plans (target architecture and Connection Manager) and
[UI-DESIGN.md](UI-DESIGN.md) (the approved UI design). Nothing in this
document invents a component that is not in those plans. The wire
rules it stands on are [PROTOCOL-V2.2.md](PROTOCOL-V2.2.md).

## 1. Overview

```
rsh-daemon  (Rust, always running, no web engine)
  ├─ Connection Manager   (state machine, §4)
  ├─ Wi-Fi connect + password (Settings password, rejoin helper)
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
| **Wi-Fi connect + password** | Wi-Fi join (open AP first, WPA2 once a password is set), the Settings Wi-Fi password (set/clear, current-password rule), and the rejoin helper that joins the PC to the `RSH-XXXX` network. No pairing, no device key, no window. |
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
  something (connect, save a profile, set a tuning value, launch a game)
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
Idle ─connect→ LinkUp ─HELLO→ Hello ─GET_CAPS ok→ Active
  ▲              │ (no HELLO)    │ (error)              │
  │              └─────────► Error(reason, hint, retry_at) ◄────────────┘
  └──────────────  Backoff(2s→30s)  ◄── Active ─(no frames 2 s / transport lost)
```

Rules (PROTOCOL-V2.2.md §6):

- **`Active` only after an unsigned `GET_CAPS` passes.** The label
  "Connected" is shown only in the `Active` state. The setup wizard's
  Step 1 (board connect) unlocks Next when the board is reachable
  (state `Hello` or better — the `board_reachable` latch, set on every
  HELLO observed); it never waits for `Active` or for any pairing
  step. There is no `Authenticating` state and no pairing step at all.
- **Re-activation loop.** A failed activation is not a one-shot: the
  manager reports `Error(reason)` and retries with backoff (2 s →
  30 s). Nothing latches on a pairing reason anymore — every remaining
  failure retries with backoff (except `AddrInUse` and the watchdog
  latch), and the UI shows a connect banner with a hint instead of
  retrying forever in silence.
- **Heartbeat watchdog.** In `Active`, if no stream/status frame
  arrives for **2 seconds**, the transport is rebuilt and the session
  re-activated (covers wheel reboot, AP restart, Wi-Fi drop). UDP has
  no "source lost" signal, so this watchdog is the only detection.
- **HELLO wait is longer than the firmware reply limiter** (700–800 ms,
  3 attempts), so a HELLO reply is not lost to the board's 250 ms
  rate limit.
- **Resume after a restart:** a retried control frame keeps its
  **reqId** — the board answers from its last-32 duplicate cache and
  never executes twice (PROTOCOL-V2.2.md §2.3). There is no
  `BAD_TAG` / 10 s authenticated-session rejection anymore.
- **Typed bind errors.** A UDP bind failure is a typed reason
  (`AddrInUse`, `PermissionDenied`, `NoInterface`) with a UI hint,
  not a swallowed `.ok()`.
- **mDNS:** `rsh.local` is resolved (fallback `192.168.4.1`); the
  address field is not a bare hostname that is never resolved.
- **Every transition** is logged (redacted) and emitted as
  `AppEvent::ConnectionChanged{state, reason}`.

**Acceptance:** wheel reboot, AP restart, Wi-Fi off/on, USB
unplug/replug, app restart — in every case the UI shows the correct
state within 5 seconds and returns to `Active` on its own. There is
no state that reads "Connected" while no data flows.

## 5. Threads — blocking work never touches the UI

- **Engine thread (dedicated).** The 200 Hz engine has its own
  thread/runtime. In v1.x the forwarder/activator and the engine
  shared one `current_thread` runtime; in v2.0 they do not.
  Network/IO runs on a separate runtime.
- **Blocking work is an async job, never a UI block.** Wi-Fi join,
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
  board's own `RSH-XXXX` network (Wi-Fi) or to USB-serial. See
  [README](../README.md#connecting).
- **Not a protocol change — in v2.0.** Protocol v2.2 is frozen. The
  daemon/UI split, the IPC channel, and the firmware v4.0.3 features
  are all above or beside the wire protocol — no wire message changed
  *for the split*. The v2.0.6 redesign **did** change the wire
  (pairing and HMAC removed, protocol v2.1 → v2.2) — see
  [PROTOCOL-V2.2.md](PROTOCOL-V2.2.md) §0 for what changed and why.

## 7. Component map (plans only)

| Plan component | Lives in | Notes |
|---|---|---|
| Connection Manager (L1) | `rsh-daemon` | §4 |
| Wi-Fi connect + password (L2) | `rsh-daemon` | Settings password; rejoin helper; no pairing, no window |
| Engine + Output | `rsh-daemon`, own thread | 200 Hz; HIDMaestro / vJoy; lazy UAC start |
| Profile / Settings / Notifications store | `rsh-daemon` | Draft/save model; `NotificationCenter` |
| Tray icon + popup trigger | `rsh-daemon` | Every launch; Slint `SystemTrayIcon` |
| Local API server | `rsh-daemon` | Named pipe / local socket, JSON lines, versioned |
| `AppApi` trait + `AppEvent` enum | Shared boundary | Typed commands + push events |
| Slint UI (`rsh-ui`) | `rsh-ui` | Overview / Wheel / Settings / Setup / tray popup; only when opened |
| Firmware v4.0.3 | ESP32 board | Open AP until a password is set, BOOT-hold KEY_RESET (10 s), horn test; protocol v2.2 frozen |
| `rsh-app-core` | Shared library | Stays UI-agnostic; `AppApi` sits above it |

Reusable crates from v1.x (unchanged in role): `rsh-protocol`,
`rsh-engine`, `rsh-transport`, `rsh-security`, `rsh-profiles`,
`rsh-telemetry`, `rsh-output-*`, `rsh-app-core`.
