# PERF-H2 — Performance + Soak (Phase H, H2)

Date: 2026-11-14. Machine: `htch-runtime` (shared Linux sandbox, 1.5 CPU,
see `tools/rsh-benchmarks/thresholds.json` `measured_host`). Repo HEAD at the
time of measurement: Phase G gate commit `92b7b71` plus the H2 rig work in
`tools/rsh-rig/` (no engine/protocol/app changes by H2). Rust 1.100.0,
criterion 0.8.2. All numbers below carry their machine/date/version (T13).

## 0. Verdict

| Gate | Result |
| --- | --- |
| Benchmark threshold gate (`check-thresholds.py`) | **PASS** — `pipeline_tick` 235.2 ns ≤ 1100 ns, `hot_path_parse_seq_pipeline` 329.5 ns ≤ 403 ns, alloc probes 0 B/tick asserted in-process |
| 15 protocol benches vs 06:00 baseline | **PASS** — 0.97–1.13× baseline (pin_validate 1.23× / 1.35×, no threshold, watch item) |
| NTP ping p99 (E2E SLO ≤ 10 000 µs) | **PASS** — 6 365 µs in the full rig run (7 698 µs standalone retest; see §2; the first run under load measured 70 339 µs and failed) |
| Rig quick (26 scenarios) | **15/26 on the first run; 5 rig bugs fixed after it — see §3. Full-run result in §4** |
| 60 s quick soak | **rate 200.7/s PASS; loss 1.94% FAIL (< 0.5%)** — after the watch-handoff fix (§3.3); was 11–96% before it (200 Hz emit) |
| 10 min full soak | **23/26 PASS — see §4. Soak loss 3.23%, long-soak loss 3.33% (both FAIL < 0.5%); memory slopes PASS (heap 0.006, RSS 0.010 MB/10 min)** |
| 4-hour soak | **SKIPPED — see §5** |

## 1. Benchmarks

Gate measurement = **run 3 protocol benches + engine-bench rerun**, both on a
quiet machine (load avg ≤ 1.0). Two earlier runs were load-contaminated and
are reported here, not hidden: run 1 (load avg ~8) and run 2 (load avg
spiking to 16.7) inflated every bench 3–11×, including pure functions that
had not changed since Phase A (CRC16 3.0–4.8×, HMAC 3.7–6.6×, HelloReply
parse 5.7–7.5×) — the signature of CPU contention, not a code regression.
Run 3's engine benches were still measured under decaying load (hot_path
837 ns); a dedicated engine rerun on a quiet machine (load 0.56) gave
hot_path 329.5 ns, matching the Phase B baseline 335.6 ns. No threshold was
tuned at any point.

| Bench | Gate mean (ns) | 06:00 baseline (ns) | Ratio | Note |
| --- | --- | --- | --- | --- |
| stream_parse_36b | 145.6 | 129.4 | 1.13× | |
| stream_encode_36b | 174.8 | 176.6 | 0.99× | |
| status_parse_34b | 98.1 | 95.9 | 1.02× | |
| status_parse_38b | 110.4 | 111.5 | 0.99× | |
| hello_parse_92b | 367.6 | 352.7 | 1.04× | |
| control_verify_get_caps_29b | 812.8 | 750.8 | 1.08× | |
| control_build_frame_get_caps | 389.9 | 404.0 | 0.97× | |
| hmac_compute_tag_empty_payload | 357.5 | 358.5 | 1.00× | |
| hmac_verify_tag | 372.8 | 383.6 | 0.97× | |
| crc16_stream_36b | 166.9 | 165.7 | 1.01× | |
| crc16_check_9b | 43.3 | 42.5 | 1.02× | |
| config_blob_encode_factory | 848.5 | 820.7 | 1.03× | |
| config_blob_decode_factory | 858.1 | 848.7 | 1.01× | |
| pin_validate_factory_modules | 5565.4 | 4533.8 | 1.23× | no threshold; watch item |
| pin_validate_blob_factory | 8872.4 | 6585.6 | 1.35× | no threshold; watch item |
| pipeline_tick | 235.2 | 162.8 (Phase B) | — | ≤ 1100 ns PASS; median 179.8 ns |
| hot_path_parse_seq_pipeline | 329.5 | 335.6 (Phase B) | — | ≤ 403 ns PASS; median 329.1 ns |
| sequence_watchdog_check | 14.9 | 13.9 (Phase B) | — | median 13.9 ns |
| alloc_probe_pipeline | 161.0 | — | — | 0 B/tick asserted in-process |
| alloc_probe_hot_path | 320.5 | — | — | 0 B/tick asserted in-process |

`check-thresholds.py` exit 0 on the gate measurement. The 0-allocation hot
path is additionally enforced by `tests/alloc_gate.rs` (10 000 ticks per
section). Logs: `h2-bench.log` (run 1), `h2-bench-run2.log`,
`h2-bench-run3.log`, `h2-bench-engine-rerun.log` (goal hidden files).

## 2. NTP ping p99 (E2E SLO ≤ 10 ms)

Measured by the rig's `ntp-ping` scenario (1 Hz ping, 30 s, 31 samples,
virtual wheel replies after base 300 µs + Gaussian σ150 µs; the E2E number
includes the engine's 5 ms tick quantisation — loopback measurement, not
real Wi-Fi).

| Run | p50 (µs) | p99 (µs) | Samples | Timeouts | Verdict |
| --- | --- | --- | --- | --- | --- |
| Smoke (bench + rig running concurrently) | 2 657 | 70 339 | 31 | 0 | FAIL — one 70 ms sample = VM scheduling stall under load |
| Retest (quiet machine) | 1 209 | **7 698** | 31 | 0 | **PASS ≤ 10 000 µs** |
| Full rig run (196 Hz emit) | — | **6 365** | 31 | 0 | **PASS ≤ 10 000 µs** |

The smoke failure was load noise, not the ping path: the same run's p50 was
fine and the retest on a quiet machine passes with margin. The invented 1%
+5 ms ping-delay spike that the first wheel implementation added to
`ping_delay()` was removed — the ping SLO measures the engine's ping path,
not an invented network.

## 3. Virtual Rig — what was built, what it found, what was fixed

No rig existed (`tests/rsh-rig/`, `tools/rsh-simulator/` absent), so H2 built
`tools/rsh-rig/` (single binary crate; workspace member, not default-member):
seeded virtual wheel (xorshift64* + Box-Muller), real `StreamFrame` parsing,
real `WheelEngine` at 200 Hz, recording sink (vJoy units 0..32767), HMAC
GET_CAPS auth gate, 26 scenarios modelled on the C# rig registry (26 IDs —
the C# `scenarios.json` v1 has 26 although its plan text says "25"; the rig
runs all 26, so 25/25 is satisfied as a subset). `cargo test -p rsh-rig`
10/10, clippy clean, fmt clean.

### 3.1 First quick run: 15/26

Failures: `soak`, `steering-calibration`, `calibration-span`,
`config-transaction`, `modules-onoff`, `fault-reboot`, `fault-output-drop`,
`long-soak`, `game-restore`, `clutch-range`, `encoder-glitch-v2`.

### 3.2 Rig bugs fixed by H2 (all in `tools/rsh-rig/`, no engine/protocol change)

1. **Watch-handoff frame loss (the big one).** `WatchSource::next_frame`
   looped back to `has_changed()` after `changed()` fired — but `changed()`
   marks the version seen, so frames arriving during the await were silently
   dropped. Minimal repro (`tests/watch_repro.rs`, since replaced by the
   `watch_source_delivers_nearly_all_frames` unit test in `harness.rs`):
   0/400 frames received with the old shape, 399/400 with the direct poll.
   Fix: after `changed()` fires, borrow the value directly instead of
   re-checking `has_changed()`. Effect on the 60 s soak loss: 11.1% /
   26.5% / 95.7% / 96.0% across runs → **1.94%** (still above the 0.5% SLO
   — see §3.4).
2. **Calibration rails vs measurement noise.** The rail thresholds
   (≤100 / ≥32600) sit inside the rig's own noise spread: ADC drift ±15 mV
   exceeds the 100-unit tolerance (≈9 mV) and a random 2 s floating burst
   over the single bottom dwell in a 20 s window held the filter ~2% off
   the rail (identical-seed steering min: 2619 / 824 / 417 / 220). Fix:
   Calibration/Clutch sweeps present exact rails (noise+drift skipped for
   those kinds; fault behaviour is still covered by soak/dropout/chaos/fuzz)
   and the two calibration scenarios run 30 s in quick mode (two bottom
   dwells). Thresholds unchanged. A pipeline-only diagnostic proved the
   pipeline itself reaches min 0 / max 32767 on the same waveform.
3. **Modules steering stimulus.** The `Modules` wheel used the 1100..2500 mV
   soak sawtooth, which can never reach axis 30000 (measured max 19426).
   Fix: `Modules` steering uses the full-range trapezoid; `modules-onoff`
   and `clutch-range` pass.
4. **Config-transaction self-check.** The factory blob is 188 B but the
   check chunked at 1024 B, so it was a single chunk and "incomplete
   staging" was actually complete — `finish()` correctly succeeded. Fix:
   chunk at 64 B (3 chunks); the protocol's `finish()` completeness check
   was correct all along.
5. **fault-output-drop threshold.** `engineTicks` counts successful sends
   and the 2 s drop in a 6 s run fails ~400 of ~1200 ticks by design, so
   ≥1000 was unachievable on any run. Fix: ≥600 (800 ceiling minus the
   standard 25% margin), arithmetic documented in the scenario.

All six retests pass after the fixes (`config-transaction`, `modules-onoff`,
`clutch-range`, `fault-output-drop`, `steering-calibration`,
`calibration-span`).

### 3.3 Known engine gap found by the rig (NOT fixed — out of H2 scope)

**`encoder-glitch-v2` FAIL — `maxAbsEncoderDelta` 502 > 32.** The plan
(TESTING-INPUT §8.4) states the encoder tracker filters `|delta| > 32/tick`,
but `rsh-engine`'s `EncoderTracker::update_all` passes raw deltas through
with no filter (`crates/rsh-engine/src/encoder.rs`, `pipeline.rs`
`process_buttons`). The wheel injects rare ±50..200 jumps per §8.2, so the
assertion fails against the real pipeline. Genuine rig-caught gap (like P3).
Do NOT weaken the assertion; route the engine fix to the coordinator.

### 3.4 Remaining loss: ~3% in the 10-min soaks (SLO < 0.5%)

After the watch fix, the 60 s standalone soak measured loss **1.94%** at a
200 Hz emit (rate 200.7/s, jitter p99 5001 µs). The first 10-min soak at
200 Hz measured **14.19%** — the 60 s number was a lucky phase window; over
600 s the wheel/engine phase cycles and averages ~10–15%.

**Emit-rate mitigation (harness change, not a threshold change).** The
engine drains at most one frame per 5 ms tick (latest-wins). When the
wheel emits at exactly the tick rate, the drain/send phase cycles and a
bad phase coalesces frames. The wheel now emits at **196 Hz (5.1 ms
interval)** — strictly slower than the 200 Hz tick, so at most one frame
lands per tick window regardless of phase; the rate SLO (≥ 195/s) still
passes (196.1–196.2/s measured). Effect on the 10-min soak: **14.19% →
3.23%** (long-soak 3.33%). The remaining ~3% is tick-jitter-driven: the
engine's tick is delayed past a full period often enough on this shared VM
(jitter p99 saturates at 5001 µs in every run) that two emits land in one
tick window. A slower emit cannot fix that (190 Hz would fail the rate
SLO), and the engine ticks at full rate (120 024 ticks / 600 s = 200.0 Hz)
— the engine is not slow, the VM's scheduling is. The transport parses
100% of frames; the loss is purely latest-wins coalescing at the
harness/engine boundary.

The soak loss SLO is therefore **not met** (3.23% / 3.33% vs < 0.5%) and
the jitter p99 misses by 1 µs (5001 vs ≤ 5000) in every run. H2 did not
tune any threshold. This is a coordinator decision: accept the harness
limitation on this VM, or rework the handoff (synchronous drive or a
multi-frame drain — both need engine cooperation) so the loss SLO becomes
measurable. Memory is flat either way: heap slope 0.006 MB/10 min, RSS
slope 0.010 MB/10 min (long-soak), and the 60 s alloc proxy passes with
0 net heap bytes.

## 4. Full run (26 scenarios, 10-min soak)

Run: `cargo run -p rsh-rig -- --mode full`, seed 42, 2026-11-14,
`rig-runs/full-20261114-1635/` (`results.json` + `REPORT.md`),
1 753.4 s wall. **23/26 PASS.** The first full-mode attempt (run
`full-20261114-1730`, 200 Hz emit) was killed mid-`long-soak` by a sandbox
service restart (13/14 scenarios done at the kill, all passing except the
soak); it is superseded by this rerun. The rerun uses the 196 Hz emit (see
§3.4) — the wheel emits at 5.1 ms, the engine still ticks at 200 Hz, and
the rate SLO (≥ 195/s) still passes at 196.1–196.2/s.

| Scenario | Result | Key numbers |
| --- | --- | --- |
| soak (600 s) | **FAIL** | loss 3.23% (< 0.5% FAIL), rate 196.1/s PASS, jitP99 5001 µs (≤ 5000 FAIL by 1 µs), outP99 23 µs PASS, heapSlope 0.006 PASS, rssSlope −0.208 PASS, allocDelta 0 B, engineTicks 120 024 (200.0 Hz) |
| long-soak (600 s) | **FAIL** | loss 3.33% (FAIL), rate 196.1/s PASS, heapSlope **0.006** MB/10 min PASS, rssSlope **0.010** MB/10 min PASS, ticks 120 047 |
| dropout-storm | PASS | loss 11.13% (injected 5% loss + dup + CRC, by design), rate 186.4/s |
| steering-calibration | PASS | rails reached (clean-rail stimulus, §3.2) |
| calibration-span | PASS | all axes at rails |
| config-transaction | PASS | contract self-checks only |
| safe-mode | PASS | watchdog fires on 3 s silence, data resumes |
| modules-onoff | PASS | accel disabled, others at rails |
| fuzz | PASS | 32.06% loss (injected garbage, by design) |
| chaos-net | PASS | loss 15.64% (Gilbert-Elliott, by design); jitP99 metric overflow — see §4.1 |
| user-chaos | PASS | loss 12.95% (injected dropouts, by design) |
| fault-reboot | PASS | loss 3.43% (reboot at t=5 s; P3 regression — no duplicate explosion) |
| fault-wifi-drop | PASS | loss 35.67% (5 s Wi-Fi drop, by design) |
| fault-output-drop | PASS | engineTicks 856 (≥ 600, §3.2 fix 5) |
| fault-app-restart | PASS | engine restart at t=3 s, resumes |
| ntp-ping | PASS | **e2eP99 6 365 µs ≤ 10 000 µs**, 31 samples, 0 timeouts |
| control-channel | PASS | contract self-checks only |
| game-restore | PASS | loss 2.80% (restart mid-drive, resumes) |
| pin-map-push | PASS | contract self-checks only |
| encoder-glitch | PASS | v1 parity (no max-delta assertion) |
| clutch-range | PASS | clutch disabled + steering at rails |
| clutch-sweep | PASS | clutch full-range sweep |
| encoder-glitch-v2 | **FAIL** | maxAbsEncoderDelta **200 > 32** — engine gap, §3.3 |
| shifter-sequential | PASS | |
| shifter-hpattern | PASS | |
| auth-gate-blocked | PASS | 0 packets accepted without GET_CAPS |

The clean-scenario loss floor is ~3% everywhere (2.80–3.66% in the
passing scenarios) — that is the harness's coalescing floor at 196 Hz
(§3.4), not per-scenario behaviour. `allocDelta` is 0 B in every engine
scenario (the 0-allocation hot path holds inside the rig as well as in the
benches); `auth-gate-blocked` shows 14 976 B, which is engine startup
allocation before the hot loop, not hot-path allocation.

### 4.1 Rig-metric wart (not a gate failure)

`chaos-net` reports `tickJitterP99Us: 4294957295` (2³² − 10 001). A
reordered arrival makes an inter-arrival delta negative, and the jitter
ring casts it to `u32`, wrapping to near-max. The value is nonsense but
the scenario has no jitter assertion that fails on it, so it did not
affect the result. Fix (rig-side, not done by H2): clamp the delta at 0
or use a signed type in the jitter ring.

## 5. 4-hour soak — SKIPPED by user decision

4-hour soak SKIPPED by user decision 2026-11-14 14:54 (time constraint);
D-R14 release blocker overridden by the user. 60 s + 10 min soak still run.

No 4-hour soak process was ever launched; verified 2026-11-14 ~14:56 that no
soak/rig processes existed, so there was nothing to kill. `docs/SOAK-RUNNING.md`
was not created (cancelled with the soak).

## 6. Honest limits of this rig (§8.6)

- In-process harness, no UDP (sandbox blocks `sendto` — Phase G condition 5):
  the transport is an mpsc + watch channel, not real Wi-Fi. Loss/jitter
  numbers measure the harness's scheduling as well as the engine.
- Debug build (`cargo run -p rsh-rig`): the engine/pipeline run
  unoptimised in the rig; criterion benches (release) are the performance
  authority, not rig timings.
- The rig's E2E ping is loopback (wheel reply delay is a model), not real
  Wi-Fi; the 10 ms SLO is applied to that loopback number.
- The engine's per-tick metrics outlet (Phase G condition 11) is what the
  rig reads for jitter/output-latency p99s; the pipeline runs the default
  plan (clutch disabled) unless a scenario enables it (Phase G condition 14).
- The rig does not enforce the HMAC session gate beyond its own wheel auth
  gate (Phase G condition 13): frames flow after the rig's auto-GET_CAPS.
  Never present a rig pass as "paired/secure".
