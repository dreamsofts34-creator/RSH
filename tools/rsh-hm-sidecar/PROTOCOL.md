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
  `rsh-hm-sidecar.exe --connect-back 127.0.0.1:<port> --token <hex>`.
  The same JSON framing runs over the socket. The token (128-bit
  hex) is echoed in `hello`; RSH refuses a hello whose token does
  not match.
  > Sandbox-blocked: in the Linux dev sandbox, a .NET process's
  > raw TCP connect is redirected to the egress proxy, which
  > refuses it ("Other TCP connections is turned off"). The
  > connect-back code is identical in shape to a minimal .NET TCP
  > client that fails the same way, while Python→Python loopback
  > works — so this is a sandbox restriction, not a sidecar bug.
  > The TCP path must be exercised on Windows (CI / owner PC,
  > gate H-HID-5). The stdio path is fully verified in the sandbox
  > instead (see `verify-linux.sh` + the Rust smoke test).

Logging goes to stderr only. stdout / the socket is the protocol.

## Hello (first line, before any command)

```json
{"ok":true,"hello":"rsh-hm-sidecar","sidecar":"rsh-hm-sidecar","sdk":"2.1.0","protocol":1,"pid":1234,"token":"…","transport":"stdio","os":"…","admin":false,"driver_installed":false,"dll_sha256":"f13e…","dll_sha256_ok":true}
```

RSH refuses the sidecar when `sidecar` is not `rsh-hm-sidecar`,
`protocol` > 1, `sdk` is not the pinned `2.1.0`, `dll_sha256_ok`
is false, or (TCP mode) the token does not match.
`driver_installed` is the state at startup (null when the context
could not be created yet); `version` re-reports it live.

## Commands

| Command | Request fields | Reply (success) | Notes |
|---|---|---|---|
| `ping` | — | `{"ok":true,"pong":true,"t_us":N}` | Heartbeat. |
| `version` | — | `sdk`, `driver_installed`, `driver`, `profile`, `transport_index`, `os`, `admin`, `dll_sha256`, `dll_sha256_ok`, `transport`, `submit_count`, `on_submit_latency_us` | `driver`/`profile`/`transport_index` null when no controller. |
| `describe-profile` | — | `profile`, `vid`, `pid_value`, `vid_pid_provisional`, `descriptor_len`, `descriptor_hex`, `input_report_size`, `input_report_size_legacy_assumption`, `button_count`, `axis_count`, `has_hat`, `backend` | Pure (no driver/context). The input report size is derived from the descriptor — 15 bytes — never the old 26-byte assumption. |
| `install-driver` | — | `was_installed`, `now_installed`, `outcome` | Fast path: when already installed, returns `outcome:"AlreadyCurrent"` without installing. `not-admin` when not elevated. |
| `create` | `profile` (optional, default `rsh-wheel-custom`) | `created`, `profile`, `transport_index`, `driver`, `create_ms`, `input_report_size`, `capabilities` | Only `rsh-wheel-custom` is accepted; catalog brand profiles are refused (`profile-refused`) — creating one would spoof a brand. Installs the driver first when needed. |
| `submit` | `axes` object (`X`/`Y`/`Z`/`Rx`/`Slider` → 0..1 floats), `buttons` uint | `submit_us`, `on_submit_latency_us`, `submit_count` | Buttons are masked to 25 bits (buttons 1–25). Unknown axis names are ignored. |
| `submit-raw` | `bytes` hex string | `submit_us`, `on_submit_latency_us`, `submit_count` | Length must equal the profile's derived input report size (15); otherwise `bad-report-size`. |
| `neutral` | `raw` bool (optional) | `submit_count` | `SubmitState` neutral (X=0.5) by default; `raw:true` sends the 15-byte neutral raw report instead. |
| `dispose` | — | `disposed` | Disposes controller + context; the device is removed. |
| `exit` | — | `bye` | Replies, disposes, exits 0. |

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
| `dll-hash-mismatch` | (Rust-side refusal) the loaded DLL's SHA-256 is not the pinned v2.1.0 hash. |
| `profile-refused` | The requested profile is not `rsh-wheel-custom`, or the built profile fails the SEC-H-06 rules (usbip / self-reporting / not deployable). |
| `create-failed` | `CreateController` threw. |
| `no-controller` | `submit`/`submit-raw`/`neutral` before `create`. |
| `bad-report-size` | `submit-raw` length ≠ the profile's derived input report size. |
| `internal-error` | Any other per-command exception (loop stays alive). |

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
