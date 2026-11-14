# FFB Extension Point (G4 — documented only)

> **Status: DOCUMENTED ONLY. Force feedback is NOT in v1.0.0.**
>
> There is no force-feedback motor in the RSH wheel hardware, no FFB
> UI, no FFB command, and no FFB code path. Nothing in this document
> is implemented; it records exactly where a future FFB implementation
> would hook in, so that work starts from the measured extension
> points instead of from a fresh spike. Do not present any of this as
> a shipped feature.

## Why FFB is out of v1.0.0

1. **No motor.** The wheel has no FFB motor or motor driver — there
   is nothing to drive, so any FFB work starts with hardware, not
   software.
2. **No HID PID surface declared.** The `rsh-wheel-custom` profile
   declares 5 axes (X/Y/Z/Rx/Slider), 32 buttons, and 1 hat
   (`tools/rsh-hm-sidecar/PROTOCOL.md` §"The custom profile"). Its
   124-byte descriptor contains no Physical Interface Device (PID)
   usage page, so games currently see a wheel with no force-feedback
   capability at all — correctly, since there is no motor.
3. **No protocol for it.** The sidecar protocol (v1) has no FFB
   command (see §2). Adding one is a protocol-version change, not a
   new line in an existing command.

## 1. The HIDMaestro path (the only path)

FFB can only ever arrive through HIDMaestro. The G1 spike measured
the extension points (`docs/HIDMAESTRO-SPIKE.md` §3/§6); all of them
are real, and all of them are currently unused:

| Extension point | What it is | Where it lives |
|---|---|---|
| HID PID 1.0 state mirror | `Global\HIDMaestroPidState<N>` — 40 B (36 used), both directions: HID PID 1.0 state mirror + EBI (effect-block-index) bitmap | Shared-memory section, per transport index `<N>` (names from `HMController.SectionNames` / `TransportIndex` — never hardcode the slot) |
| FFB capture ring | `Global\HIDMaestroOutput<N>` — ~16.5 KB, 64-slot ring, driver/companion → SDK: rumble / haptics / FFB capture | Shared-memory section, same naming rule |
| SDK publish calls | `HMController.PublishPidPool / PublishPidBlockLoad / PublishPidState` — the SDK calls that publish PID pool / block-load / state | `HIDMaestro.Core.dll` v2.1.0 (pinned); measured by reflection in the G1 spike |
| Output reports | The UMDF2 driver delivers game FFB output reports through the same sections above; the SDK surfaces them via the publish calls | Driver → SDK direction (opposite to the input frames RSH submits today) |

### What a future FFB implementation would need

Ordered, because each step gates the next:

1. **Hardware first.** A motor + motor driver on the wheel, and a
   firmware path to drive it (torque command, current limit, thermal
   limit). Without this, steps 2–6 produce effects no one can feel.
   The firmware side is a firmware-plan item, not an app item.
2. **Profile rebuild.** The custom profile must gain a PID usage
   page (HID PID 1.0: effect types, axes, and the effect-block
   collection) alongside the existing axes/buttons. This changes the
   descriptor (124 bytes today), the derived input-report size
   (15 bytes today), and the VID/PID identity story — the game
   matrix (Phase F) must be re-run, because games will see a
   *different device* (a wheel with FFB) and bindings must be remade.
   `PublishPid*` is explicitly "stubbed/documented only (FFB not in
   v1.0)" in the G2 build list — stubbing it further is not the work;
   declaring the usages is.
3. **Sidecar protocol v2.** The JSON-lines protocol
   (`tools/rsh-hm-sidecar/PROTOCOL.md`, v1) has no FFB command. The
   current commands are `ping`, `version`, `describe-profile`,
   `install-driver`, `create`, `submit`, `submit-raw`, `neutral`,
   `dispose`, `exit` — all input-direction except `neutral`/`dispose`.
   A future version needs at least:
   - a way for Rust to **read** PID state (effect-block load, active
     effects, device gain) — e.g. a `ffb-state` command returning the
     PID state mirror contents;
   - a way for Rust to **publish** PID pool/block-load/state through
     the SDK calls above — e.g. a `publish-pid` command;
   - `hello` negotiation: the sidecar must report whether its SDK
     build exposes the publish calls, and Rust must refuse a hello
     whose protocol version is newer than it speaks (the v1 rule
     already refuses `protocol > 1`; FFB needs `protocol = 2`).
   A protocol bump also needs the sidecar + Rust crate to change
   together — the manager refuses a mismatched hello today, and that
   rule must be kept.
4. **Rust manager methods.** `rsh-output-hidmaestro`'s
   `SidecarManager` needs FFB counterparts to `submit` /
   `submit_raw`: a poll for PID state (bounded rate — the PID state
   section is 40 B and cheap to read, but polling it at 200 Hz would
   be a second hot path; the poll rate is a design decision for the
   FFB phase) and a publish path. Both go through the same
   JSON-lines transport (stdio in tests, TCP connect-back on
   Windows) and the same heartbeat / crash-detection rules as
   `submit` — a dead sidecar during FFB must still fall back cleanly,
   and a stale effect must go neutral.
5. **A separate sink, not `IJoystickOutput`.** The output trait
   (`try_connect` / `update` / `send_neutral`) is input-direction
   only: it carries processed wheel state *to* the game. FFB flows
   the other way (game → motor). It belongs in a new trait (e.g. an
   `FfbSink`: `poll_effects() -> …`, `set_gain(…)`,
   `stop_all_effects()`), owned by whoever owns the motor driver —
   not bolted onto `IJoystickOutput`, and not sent through the
   200 Hz engine loop as if it were an axis. Effect state must never
   be allowed to keep a stale torque applied: the same 250 ms
   watchdog rule as the input path applies (stale → neutral/stop).
6. **Effect model (HID PID 1.0).** The effects a game can request
   are the HID PID 1.0 set the descriptor declares: constant force,
   ramp, periodic (sine/square/triangle/sawtooth), spring, damper,
   inertia, friction — plus device gain and the EBI bitmap that
   tracks which effect blocks are loaded. The FFB phase must decide
   which subset the motor can actually render (a small motor cannot
   render all of them honestly) and **reject the rest with a clear
   status** instead of silently mapping them onto the wrong effect.
7. **Safety + testing (Windows-only).** Torque limits, a
   user-visible FFB strength cap, and an emergency stop (game exit /
   app exit / sidecar death must all stop effects — the sidecar
   already removes the device on exit, ≤ 10 s sweep; FFB must not
   outlive that). Verification is Windows-only: the game matrix
   re-run per game with FFB on, plus a soak with effects active.
   None of this is measurable in the Linux sandbox.

### What is deliberately NOT an extension point

- `SubmitState` / `SubmitRawReport` (the 15-byte input report) carry
  input only. They cannot carry FFB, and must not be extended to try.
- The `HIDMaestroStatus` diagnostics (`submit_count`,
  `last_submit_latency_us`) measure input submits. FFB latency /
  effect counts need their own counters when FFB lands.
- The UAC/elevation story does not change for FFB: the same elevated
  sidecar session already submits input; reading PID state goes
  through that same session.

## 2. vJoy has no such path

**vJoy cannot do force feedback for RSH, and there is no extension
point to document for it.**

- The vJoy FFI surface (`crates/rsh-output/src/vjoy.rs`) is ten
  `extern "C"` functions, all input-direction: `vJoyEnabled`,
  `GetvJoyVersion`, `DriverMatch`, `GetVJDStatus`, `AcquireVJD`,
  `RelinquishVJD`, `ResetVJD`, `UpdateVJD`, `GetVJDAxisExist`,
  `GetVJDButtonNumber`. There is no output-report function, no FFB
  function, and no callback for game-requested effects.
- `UpdateVJD` sends a `JOYSTICK_POSITION` *to* the driver. vJoy
  exposes no channel in the opposite direction, so a game has no way
  to tell a vJoy device to apply a force, and RSH has nothing to read.
- Consequence: **if FFB ever ships, it ships on HIDMaestro only,
  and the vJoy fallback stays input-only.** A user on the vJoy
  backend gets no FFB — the Output page must say so when FFB lands
  (it does not need to say so today, because FFB does not exist yet).
  Do not design an FFB feature that assumes the fallback backend can
  carry it.

## 3. Sources

- `docs/HIDMAESTRO-SPIKE.md` §3 (shared-memory sections, SDK surface
  incl. `PublishPidPool/BlockLoad/State`), §6 (G2 build list:
  `PublishPid*` stubbed/documented only).
- `tools/rsh-hm-sidecar/PROTOCOL.md` (protocol v1 commands, custom
  profile, 15-byte input report).
- `crates/rsh-output-hidmaestro/src/mapping.rs`
  (`build_raw_report` — input report layout, 15 bytes).
- `crates/rsh-output/src/vjoy.rs` (ten-function FFI surface, all
  input-direction; no FFB function).
- HID PID 1.0 (USB-IF Device Class Definition for Physical Interface
  Devices) — the effect model the descriptor would declare. Not
  re-verified here; the FFB phase must verify the exact usage pages
  against the HIDMaestro SDK's `HidDescriptorBuilder` PID support
  before declaring them.
