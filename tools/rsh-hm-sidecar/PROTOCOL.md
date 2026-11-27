# rsh-hm-sidecar protocol (v1)

JSON-lines between the Rust crate (`crates/rsh-output-hidmaestro`)
and the C# sidecar (`tools/rsh-hm-sidecar`). One command per line,
one JSON reply per line, in order. The sidecar answers sequentially;
the Rust side keeps at most one command in flight.

## Transports

- **stdio** — default. The sidecar reads stdin, writes stdout.
  Used when RSH itself runs elevated (dev) and in the sandbox.
- **TCP connect-back** — production on Windows. RSH spawns the
  sidecar elevated via `runas` (handle redirection does not survive
  a `runas` launch), so RSH listens on loopback and the sidecar
  connects back:
  `RshCoreHiDcr.exe --connect-back 127.0.0.1:<port> --token <nonce>`.
  The one-time token (128-bit hex) travels on the sidecar's command
  line (`--token`, v2.0.12) — NOT in the environment: AIS builds a
  fresh environment for a `runas` child, dropping runtime-set
  variables, so the old `RSH_HM_SIDECAR_TOKEN` env transport died
  with exit 2 before connecting (proven on the owner's PC, v2.0.11
  retest). The nonce is single-use random and briefly visible in
  the process list / WMI — accepted; it is never logged anywhere
  (presence only). The token is echoed in `hello`; RSH refuses a
  hello whose token does not match, and additionally verifies the
  accepted socket's peer PID against the spawned sidecar process
  (C-TOK-1). The sidecar validates `--connect-back` strictly
  (loopback host + port 1–65535 only; `--token` is required with a
  connect-back, and a missing value / duplicate flag is
  `bad-args`); anything else is a distinctive `[sidecar] bad-args:`
  stderr line + fast exit 2.
  The same JSON framing runs over the socket.
  > Sandbox-blocked: in the Linux dev sandbox, a .NET process's
  > raw TCP connect is redirected to the egress proxy, which
  > refuses it ("Other TCP connections is turned off"). The
  > connect-back code is identical in shape to a minimal .NET TCP
  > client that fails the same way, while Python→Python loopback
  > works — so this is a sandbox restriction, not a sidecar bug.
  > The TCP path must be exercised on Windows (CI / owner PC,
  > gate H-HID-5). The stdio path is fully verified in the sandbox
  > instead (see `verify-linux.sh` + the Rust smoke test).

Logging goes to stderr AND the file log
`%PROGRAMDATA%\RSH\logs\rsh-sidecar-YYYYMMDD.log` (v2.0.11 P0 —
stderr is discarded on the TCP path and when elevated, so the file
is the only trace a pre-hello death leaves). stdout / the socket
is the protocol.

## Hello (first line, before any command)

```json
{"ok":true,"hello":"rsh-hm-sidecar","sidecar":"rsh-hm-sidecar","sdk":"2.1.0","protocol":1,"pid":1234,"token":"…","transport":"stdio","os":"…","admin":false,"driver_installed":false,"dll_sha256":"f13e…","dll_sha256_ok":true}
```

RSH refuses the sidecar when `ok` is not `true`, `sidecar` is not
`rsh-hm-sidecar`, `protocol` > 1, `sdk` is missing or not the pinned
`2.1.0`, `dll_sha256` is missing or not the pinned hash,
`dll_sha256_ok` is false, or (TCP mode) the token does not match
(C-SEC-1 — all of `ok` / `sdk` / `dll_sha256` are REQUIRED; a hello
that stays silent about any of them is refused).
`driver_installed` is the state at startup (null when the context
could not be created yet — v2.0.11 sends the hello BEFORE the
context is created, so a null here means "context not attempted
yet", and `install-driver` / `create` then report
`context-unavailable` when the context cannot be created);
`version` re-reports it live.

## Commands

| Command | Request fields | Reply (success) | Notes |
|---|---|---|---|
| `ping` | — | `{"ok":true,"pong":true,"t_us":N}` | Heartbeat. |
| `version` | — | `sdk`, `driver_installed`, `driver`, `profile`, `transport_index`, `os`, `admin`, `dll_sha256`, `dll_sha256_ok`, `transport`, `submit_count`, `on_submit_latency_us`, `output_received_count` | `driver`/`profile`/`transport_index` null when no controller. `output_received_count` is the G29 FFB-discard counter (v2.0.16) — packets the game sent and the sidecar discarded, never payload data. |
| `describe-profile` | `profile` (optional) | `profile`, `vid`, `pid_value`, `vid_pid_provisional`, `descriptor_len`, `descriptor_hex`, `input_report_size`, `input_report_size_legacy_assumption`, `button_count`, `axis_count`, `has_hat`, `backend` | Without `profile`: the ACTIVE profile once a controller exists, else the custom profile (pure, no driver/context). With `profile: "logitech-g29"`: loads the catalog profile from the SDK (needs the context + `LoadDefaultProfiles`) — VID `0x046D`, PID `0xC24F`, 123-byte descriptor, derived 12-byte report, 25 buttons, 4 axes, hat. Any other id is refused (`profile-refused`). |
| `install-driver` | — | `was_installed`, `now_installed`, `outcome` | Fast path: when already installed, returns `outcome:"AlreadyCurrent"` without installing. `not-admin` when not elevated. |
| `create` | `profile` (optional, default `rsh-wheel-custom`) | `created`, `profile`, `transport_index`, `driver`, `create_ms`, `input_report_size`, `output_received_count`, `capabilities` | Exactly two profiles are accepted: `rsh-wheel-custom` and `logitech-g29` (v2.0.16, G29 spoof mode — default-off, HIDMaestro-only). Every other catalog profile is refused (`profile-refused`) — creating one would spoof a brand. The G29 profile is loaded from the SDK catalog and its descriptor bytes must equal the pinned 123-byte G29 descriptor (VID `0x046D` / PID `0xC24F`); a mismatch refuses create. The G29 input size is derived from the descriptor (12 bytes) — the SDK's own `InputReportSize` is 0 for catalog profiles and is never trusted. Installs the driver first when needed. |
| `submit` | `axes` object (`X`/`Y`/`Z`/`Rx`/`Slider` → 0..1 floats), `buttons` uint | `submit_us`, `on_submit_latency_us`, `submit_count` | Buttons are masked to 25 bits (buttons 1–25). Unknown axis names are ignored. Axis values are clamped to 0..1 by the sidecar (non-finite → axis neutral); the Rust side clamps too. REFUSED (`bad-command`) while a G29 controller is active — the SubmitState frame layout does not match the G29 profile; use `submit-raw` (the Rust side forces the raw path under G29). |
| `submit-raw` | `bytes` hex string | `submit_us`, `on_submit_latency_us`, `submit_count` | Length must equal the profile's derived input report size (15 custom / 12 G29); otherwise `bad-report-size`. The field is `bytes` — a `report` field is rejected as `bad-command` (C1). The raw path is the Rust default (M3: buttons 1–32 + encoder pulses reach the game). |
| `neutral` | `raw` bool (optional) | `submit_count` | `SubmitState` neutral (X=0.5) by default; `raw:true` sends the neutral raw report instead (15 bytes custom / 12 bytes G29: X=32768, pedals 0, hat 0x08, buttons 0, vendor bytes 0). Under G29, `SubmitState` neutral is refused (`bad-command`) — `raw:true` is required. Rust USES this for watchdog / shutdown neutral (L2). |
| `dispose` | — | `disposed` | Disposes controller + context; the device is removed. Rust USES this on shutdown, before `exit` (L2) — both are sent best-effort with no reply wait. |
| `exit` | — | `bye` | Replies, disposes, exits 0. Rust never waits for this reply on shutdown (H4 — the transport is killed right after queueing it; stdin EOF / socket close disposes anyway). |

`capabilities` (in `create` replies):

```json
{"axes":5,"buttons":32,"has_hat":true,"input_report_size":15,"submit_state_max_button":25,"submit_state_button_mask":33554431}
```

## Errors

```json
{"ok":false,"code":"not-admin","error":"…"}
```

| Code | Meaning |
|---|---|
| `bad-command` | Malformed JSON, missing `cmd`, or a missing/invalid required field. |
| `unknown-command` | No such command. |
| `not-admin` | The operation needs elevation (driver install) and the sidecar is not administrator. |
| `driver-not-installed` | Driver required and not installed (install failed or was refused). |
| `driver-install-failed` | `InstallDriver()` threw. |
| `context-unavailable` | The HIDMaestro context could not be created (`new HMContext()` threw) — the sidecar is alive, the context is not. v2.0.11: install-driver / create report this instead of a generic failure, so the daemon can name the state. |
| `dll-hash-mismatch` | (Rust-side refusal) the loaded DLL's SHA-256 is not the pinned v2.1.0 hash. |
| `profile-refused` | The requested profile is not `rsh-wheel-custom` or `logitech-g29`, the loaded G29 catalog profile does not match the pinned G29 identity (VID/PID/descriptor), or the built profile fails the SEC-H-06 rules (usbip / self-reporting / not deployable). |
| `create-failed` | `CreateController` threw. |
| `no-controller` | `submit`/`submit-raw`/`neutral` before `create`. |
| `bad-report-size` | `submit-raw` length ≠ the profile's derived input report size. |
| `internal-error` | Any other per-command exception (loop stays alive). |

## The G29 profile (v2.0.16 — G29 spoof mode)

`logitech-g29`, the HIDMaestro v2.1.0 built-in catalog profile,
loaded from the SDK with `LoadDefaultProfiles` + `GetProfile`
(never hand-built). VID `0x046D`, PID `0xC24F` (PC mode — NOT the
PS4-mode `0xC260`), 123-byte descriptor, derived input report:
12 bytes (hat + 25 buttons in 4 bytes, X steering u16, Z/Rz/Y
pedals u8, 3 vendor bytes). 4 axes, 25 buttons, 1 hat. The 16-byte
0xFF00 Output report is kept, so the game's FFB writes have an
endpoint — the sidecar subscribes `OutputReceived` and DISCARDS
them (the wheel has no motor): a counter only
(`output_received_count`, logged every 1000th packet), never
payload data, never an error to the user. `HapticReceived` does
NOT exist on `HMController` (it is only on `HMVRController`) and
is not subscribed. `submit` (SubmitState) is refused while a G29
controller is active — only `submit-raw` (12 bytes) may be used.

RISK DISCLOSURE (research 2026-11-27): presenting Logitech's
VID/PID violates USB-IF policy as written (no enforcement found
for private hobby use); no Forza ToS text on spoofing was found
(risk low, not zero); the user's pid.codes `0x1209`/`0x5253`
claim is UNAFFECTED (simply unused while G29 is active). Even a
REAL G29 needs a custom control scheme in FH4 (Forza Known
Issues) — this mode promises no auto-recognition, and FH4
end-to-end is unproven. Pedal roles are per the HIDMaestro
profile layout (Z = accelerator, Rz = brake, Y = clutch);
PCGamingWiki disagrees (Y = accelerator) — the Rust side keeps
the assignment swappable (`G29PedalMap`) for the PC test.

## The custom profile

`rsh-wheel-custom`, Type `wheel`, 5 axes (X/Y/Z/Rx/Slider, 16-bit),
32 buttons, 1 hat (4 directions, null state). Descriptor: 124
bytes; derived input report: 15 bytes (5×16-bit axes + 32×1-bit
buttons + 8-bit hat = 120 bits). VID `0x1209` (pid.codes
open-source pool), PID `0x5253` (ASCII "RS") — PROVISIONAL until a
pid.codes PR claiming it for "RSH — Real Sim Hub Wheel" is merged
(release gate before v1.0.0). The old 26-byte assumption (a
128-button profile that was never built) is wrong for this profile
and is deliberately not implemented anywhere.

## Lifecycle

- stdin EOF / TCP disconnect / `exit` → the sidecar disposes the
  controller and context (the virtual device is removed) and exits.
- A crash of the Rust side closes the pipe, so the device is
  removed — no zombie wheel.
- The Rust side sends `neutral` before `dispose` on a clean stop.
