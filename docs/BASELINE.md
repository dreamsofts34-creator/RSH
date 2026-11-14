# RSH Rust — Performance Baseline (Phase A6, R-H17)

> **Status: BASELINE CAPTURED 2026-11-14.** These are **sandbox** numbers, not
> release gates. No threshold is set from this file — `thresholds.json`
> final gate values are set in **Phase B after first measurement**
> (SW-R8.1). Nothing here was tuned to match the C# numbers; the C#
> column is context only.

## 0. Capture identity (T13 lesson — machine + date + version)

| Field | Value |
|---|---|
| Date | 2026-11-14 |
| App version | v1.0.0 (Rust recreation, fresh start — no C# port) |
| Protocol | v2.1 (wire frozen) |
| Firmware | C++ v4.0.1 (unchanged; Rust app talks to the same board) |
| Repo commit | `741480328049e53c079038bac38716b737656de8` (`7414803`, "Remove C# codebase — full Rust recreation") |
| Machine | Linux sandbox VM — **1.5 shared CPU**, 6 GB RAM (`Linux htch-runtime 7.0.0-44-generic x86_64`) |
| Toolchain | cargo 1.100.0 (`3acc79c0c 2026-11-05`), rustc 1.100.0 (`0ef1bc386 2026-11-09`), stable |
| Bench harness | criterion **0.8.2** (exact pin, `Cargo.lock`; A3 decision — criterion2 3.0.5 is the fallback, not used) |
| Bench crate | `tools/rsh-benchmarks` (`benches/protocol.rs`, `harness = false`) |
| Bench command | `cargo bench -p rsh-benchmarks` |

## 1. Method

- **What is benched:** `rsh-protocol` only — everything that exists today
  (A1). Pipeline tick / engine E2E benches do **not** exist yet (engine is
  Phase B) and were **not** written; see §4 TBD.
- **Inputs:** golden vectors copied verbatim from
  `crates/rsh-protocol/tests/golden_vectors.rs` (themselves copied from
  `reference/tests-host/protocol_v2_vectors.py`). No bench input was
  invented. Test fixtures only: `TEST_KEY = 00..1f`,
  `TEST_NONCE = a0..af` are **not** real secrets.
- **Runs:** 3 full `cargo bench` runs (A, B, C), each with criterion
  defaults — 3 s warm-up, 5 s measurement, **100 samples per bench per
  run**. The "mean" column below is the mean of the three run means.
  Median and p99 are from run C; p99 is computed from run C's 100
  per-sample times (`sample.json`: per-iteration time = `times[i] /
  iters[i]`, 99th percentile, nearest-rank) — criterion 0.8.2 itself
  reports mean/median/std-dev only, not p99.
- **Raw data:** per-run criterion estimates are kept in
  `tools/rsh-benchmarks/baseline/` (`runC-*.json` estimates,
  `runs-summary.json` with all three runs' means/medians/std-devs/p99,
  `run3.log` full console output of run C).
- **Build profile:** criterion default = release with debug assertions
  off (criterion builds benches in release mode).

## 2. Results — `rsh-protocol` (all numbers nanoseconds unless noted)

| Bench | Input | Mean (of 3 runs) | Run A / B / C means | Median (run C) | p99 (run C) | C# v2.0.4 reference (context only) |
|---|---|---|---|---|---|---|
| `stream_parse_36b` | stream golden vector, 36 B | **129.4 ns** | 130.6 / 129.8 / 127.8 | 127.6 ns | 135.9 ns | parse ~205 ns |
| `stream_encode_36b` | parsed golden stream frame | **176.6 ns** | 176.5 / 176.6 / 176.8 | 172.8 ns | 217.8 ns | — (no C# equivalent measured) |
| `status_parse_34b` | status golden vector, 34 B | **95.9 ns** | 95.4 / 99.2 / 93.1 | 92.8 ns | 94.7 ns | — |
| `status_parse_38b` | status extended, 38 B (`btnsEx`) | **111.5 ns** | 110.7 / 112.2 / 111.6 | 110.3 ns | 129.6 ns | — |
| `hello_parse_92b` | HELLO unpaired, 92 B | **352.7 ns** | 351.3 / 352.1 / 354.6 | 350.1 ns | 431.2 ns | — |
| `control_verify_get_caps_29b` | GET_CAPS frame, 29 B (tag verify + parse) | **750.8 ns** | 753.4 / 752.1 / 746.8 | 739.0 ns | 846.5 ns | — |
| `control_build_frame_get_caps` | GET_CAPS, empty payload (tag build + encode) | **404.0 ns** | 397.1 / 427.6 / 387.3 | 387.1 ns | 404.3 ns | — |
| `hmac_compute_tag_empty_payload` | `compute_tag`, 16 B tag | **358.5 ns** | 359.3 / 360.0 / 356.2 | 356.0 ns | 368.7 ns | — |
| `hmac_verify_tag` | `verify_tag` (`verify_truncated_left`), 16 B tag | **383.6 ns** | 372.4 / 381.4 / 397.0 | 372.1 ns | 698.4 ns | — |
| `crc16_stream_36b` | CRC16-CCITT over 36 B stream vector | **165.7 ns** | 165.7 / 165.8 / 165.7 | 165.1 ns | 175.7 ns | — |
| `crc16_check_9b` | CRC16-CCITT over `"123456789"` (check `0x29B1`) | **42.5 ns** | 42.5 / 42.1 / 43.0 | 42.2 ns | 52.7 ns | — |
| `config_blob_encode_factory` | factory-default blob, 11 records | **820.7 ns** | 822.7 / 820.4 / 819.0 | 817.3 ns | 835.4 ns | — |
| `config_blob_decode_factory` | factory-default blob bytes | **848.7 ns** | 851.6 / 841.3 / 853.1 | 835.3 ns | 1079.3 ns | — |
| `pin_validate_factory_modules` | 11 factory modules (axis/btn/enc/LED/battery) | **4.53 µs** | 4.30 / 4.25 / 5.06 µs | 5.07 µs | 5.83 µs | — |
| `pin_validate_blob_factory` | factory blob → `validate_blob` | **6.59 µs** | 6.32 / 6.19 / 7.25 µs | 6.98 µs | 11.18 µs | — |

Sample count: 100 samples per bench per run (300 total per bench).
All three runs completed with **0 errors**.

## 3. Repeatability — read this before quoting any number

- **Most benches are stable:** 13 of 15 benches vary ≤ 6.4% (max−min of the
  three run means, as % of the 3-run mean); 10 of 15 vary ≤ 2.5%.
  `crc16_stream_36b` is the most stable (0.1% spread).
- **The two PIN-RULES benches are noisy:** `pin_validate_factory_modules`
  spread **17.8%** (4.25 → 5.06 µs) and `pin_validate_blob_factory`
  spread **16.1%** (6.19 → 7.25 µs), and their run-C p99s (5.83 µs /
  11.18 µs) sit far above their means. Both benches allocate (a `HashMap`
  per `validate` call, `Vec<String>` results) and run longest, so they
  are the most exposed to VM scheduling jitter. Treat their numbers as
  order-of-magnitude only until re-measured on Windows.
- **Outliers are normal here:** criterion reported 2–10% high outliers
  per bench per run (mostly "high severe"), consistent with a 1.5-shared-CPU
  VM, not with a defect in the code. Criterion's statistical comparison
  absorbs this; the 3-run spread above is the honest uncertainty.
- **Sandbox jitter inflates numbers.** These are Linux sandbox
  measurements on shared vCPUs. They are **not** Windows numbers, not
  release numbers, and not gates. The same benches on the owner PC
  (Windows 10 22H2, real hardware) will differ — re-measure there before
  any gate decision (Phase B / SW-R8.1).

## 4. TBD — baseline pending (not measured, not faked)

| Item | Status | C# reference (context only) |
|---|---|---|
| Pipeline tick (`ProcessingPipeline`, per-module) | TBD — baseline pending (engine is Phase B) | ~1.1 µs/tick |
| Engine E2E latency (packet-in → output-out, p99) | TBD — baseline pending (engine is Phase B; E2E is only measured by NTP-style ping, protocol v2.1 §9) | p99 ≈ 4.8 ms |
| Packet inter-arrival jitter (PC receive, p99) | TBD — baseline pending (transport is Phase C) | p99 ≤ 1 ms SLO |
| Windows startup (launch → window) | TBD — baseline pending (Windows-only, owner PC) | ≤ 1200 ms SLO |
| Tray / single-instance behaviour | TBD — baseline pending (Windows-only, owner PC) | — |
| Real-board numbers (sent/s, overruns, maxCycleUs) | TBD — baseline pending (real board only at final production release, user rule) | sent/s ≈ 200, maxCycleUs < 3500 µs |
| 4-hour soak (alloc growth, loss, stability) | TBD — baseline pending (rig + engine, Phase B/H) | 200/s sustained |
| 0-allocation hot-path proof | TBD — baseline pending (alloc-counting harness, Phase B / SW-R8.1) | 0 B/tick |

No relative gates ("as fast as C#") are set anywhere in this file — per
R-H17, numbers or "TBD — baseline pending" only.

## 5. Notes for the Phase B gate

- Stream parse (~129 ns mean, 136 ns p99 on this sandbox) sits below the
  SOFTWARE-PLAN §1.4 parse target (p99 ≤ 1 µs) with headroom, but that
  target is a **target, not a gate** — the gate values go into
  `thresholds.json` in Phase B after first measurement.
- HMAC tag build (~359 ns) + verify (~384 ns) dominate a control-frame
  round trip (~751 ns verify+parse); both are cold-path operations
  (control channel, not the 200 Hz hot path), so no optimisation is
  warranted from these numbers.
- `pin_validate_*` are cold-path too (config push / SET_CFG time), and
  their noise means they should never gate a release on sandbox numbers.
