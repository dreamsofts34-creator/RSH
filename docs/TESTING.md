# RSH — Testing

> **Versions:** App **v2.0.4** · Firmware **v4.0.1** · Protocol **v2.1**
> (2026-11-14).
> **Rule: every test runs in the Virtual Test Rig on local builds.
> Real-board tests happen once, at the final production release
> (A7). Never wait on GitHub Actions for testing.**

## The Virtual Test Rig (standard environment)

Location: `~/workspace/projects/rsh/test-env/` (outside this repo —
it is a test harness, not shipped code).

```
./run-virtual-rig.sh            # quick gate (60 s soak)
./run-virtual-rig.sh --full     # + 10-minute soak
./run-virtual-rig.sh --long     # 4-hour soak
./run-virtual-rig.sh --only <scenario|unit>
./run-virtual-rig.sh --seed N  # deterministic run
```

What it is:

- **Virtual wheel** (`virtual-wheel/wheel.py`, Python stdlib): a
  200 Hz UDP board that speaks protocol v1 + v2.1, with ADC noise
  (σ 3/5 mV + drift), floating sensors, button bounce, encoder
  glitches, network chaos (jitter, Gilbert–Elliott loss, reorder,
  duplicates), clock drift, faults (reboot / Wi-Fi drop / power cut),
  sequence/timestamp wraps, fuzzing and seeded runs.
- **PC harness** (C#, net10.0): runs the **real** `WheelEngine` and
  `ProcessingPipeline` from `RSH.Core`; vJoy is a recording stub.
- **Firmware host stage:** C++ host tests + pin rules + protocol
  vectors + a `pio run` check.
- **Scenarios:** 25 registered scenarios (`scenarios/scenarios.json`),
  including fault-reboot, auth-gate, control-channel and soak.

Honest limits: no real ADC, no real Wi-Fi, no real vJoy, no ESP32
timing, no Windows WPF. Sandbox jitter is inflated by VM scheduling
(threshold 5 ms in the sandbox vs 1 ms real). The rig has caught real
bugs — e.g. **P3**, the `SequenceTracker` reboot bug (after a 2 s
board reboot, ~1000 valid packets were counted as duplicates).

## Test suites in this repo

| Suite | Command | v2.0.4 gate |
|---|---|---|
| `RSH.Tests` (core) | `dotnet test` | **783 pass / 0 fail / 5 skip** |
| `RSH.VJoy.Tests` | `dotnet test` | **56 / 56** |
| `RSH.HIDMaestro.Tests` | `dotnet test` | **78 / 78** |
| `RSH.App.Tests` | `dotnet test` (CI) | Windows-only — does not run on Linux |
| Firmware host tests | `tests-host/run_tests.sh` | **4430 checks, 0 failures** |
| Pin rules | `tests-host/pin_rules_ref.py` | 50 checks pass |
| Protocol vectors | `tests-host/protocol_v2_vectors.py` | 51 checks pass |
| Benchmarks | `tools/RSH.Benchmarks` | **15 / 15 PASS** (threshold-gated) |
| Rig (full) | `./run-virtual-rig.sh` | **25 / 25 scenarios**, soak 200/s |

**Total at the v2.0.4 gate: 917 .NET tests pass, 0 fail**
(783 + 56 + 78). Build: 0 errors. `pio run`: SUCCESS, 0 warnings.
Merged firmware image: 955,056 B (RAM 30.2%, Flash 67.4%).

## Gates

Work is built phase by phase: **build → rig test → find bugs →
fix → gate → next phase**. No step is skipped, even under time
pressure; speed comes only from parallel work, never from lighter
verification. A gate is PASS only when the build, all suites, the
rig, the host tests, the firmware build and the benchmarks are
green. Non-blockers may be deferred only when they genuinely cannot
be verified in the sandbox (e.g. the real-board A7 run) — and they
are documented as conditions, not silently dropped.

## CI (GitHub Actions)

- **`.github/workflows/ci.yml`** ("RSH — CI"): on every push/PR —
  `dotnet` build + test (windows-latest), host tests (strict
  `-Wall -Wextra -Werror -fsanitize=address,undefined` + Python
  scripts), firmware build (`pio run` + merged-bin check),
  benchmark gate (`thresholds.json` enforced).
- **`.github/workflows/release.yml`** ("Build and Release"): on tag
  `v*` / `fw-v*` (or manual dispatch) — firmware bins (signed) +
  Windows app zip → GitHub Release. The workflow verifies the
  firmware signature and fails closed.

## The A7 real-board run (the one test that needs hardware)

[`A7-RUNBOOK.md`](A7-RUNBOOK.md) — flash the signed merged bin,
run the app, 10-minute run. Pass: sent/s ≈ 200 (195–205), overruns 0,
maxCycleUs < 3500 µs, reboots 0. The development plan freezes only
after A7 passes. Brick recovery:
[`BRICK-RECOVERY.md`](BRICK-RECOVERY.md).

## Known testing gaps (honest)

- **4-hour long soak: never completed.** The only attempt was
  stopped ~16 min in by the user. The 60 s and 10-min soaks pass;
  the 4-hour run needs a full 4 hours via `--long`.
- **Windows-only checks:** theme switching, Narrator/NVDA,
  keyboard-only use, High Contrast, startup ≤ 1200 ms (U-P8),
  real vJoy driver, real Steam game launches (the 8-game
  compatibility matrix is still a draft), HIDMaestro on Windows,
  unsigned-MSIX install on a clean PC.
- **Board-side:** A7, flash encryption, Secure Boot eFuse steps,
  LED strip light check, AS5600 on hardware, mDNS end-to-end on
  Windows (`rsh.local`).
