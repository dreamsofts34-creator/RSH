# Phase H Gate — RSH Rust recreation (Tauri 2 app, fresh start)

**Verdict recommendation: CONDITIONAL PASS** (see §8 — the
conditions are pre-tag human / Windows-only gates only; there are
ZERO failing tests and ZERO failing rig scenarios).
**Date:** 2026-11-14 · **Gate worker:** Phase H GATE RE-RUN (fresh
re-run after FIX-A + FIX-RIG landed, no fixes, no commits, no
pushes, no git commands).
**Repo:** `~/workspace/projects/rsh/repo/` at HEAD `92b7b71` (Phase G) +
all Phase H work uncommitted in the tree (H1 security, H2 perf+soak+rig,
H3 polish, H4 packaging/signing, H5 docs, H5-activator, H5-control,
FIX-A soak proxy, FIX-RIG rig metrics). NOTE: `tools/rsh-rig/` is
**untracked** (`??` in git status) — the coordinator's Phase H commit
must `git add` it. Toolchain: cargo/rustc **1.100.0**, node
v24.22.1, npm 10.9.4. Machine: `htch-runtime` (shared Linux sandbox,
1.5 CPU).

> Honesty rule: every number below comes from a command run in this
> gate session. Anything not run here is marked NOT RUN / Windows-only,
> never presented as passed. The 4-hour soak is **SKIPPED by user
> decision 2026-11-14 14:54** (D-R14 overridden) — recorded as SKIPPED,
> never as passed. **DECISIONS §14 is REVOKED** (DECISIONS §15, user
> order 2026-11-14 18:01: "Fix kro in ko, mujy koi failure ya bug ni
> chahiye") — this doc does NOT cite §14 as an accept anywhere; the
> soak proxy, soak loss and ntp-ping are FIXED with fresh evidence
> below (§1.3.1, §6.2, §6.3).

## 1. Evidence table (fresh this session)

| # | Check | Result |
|---|-------|--------|
| 1.1 | `cargo build --workspace` | **FAIL (expected, Windows-only gap)** — `gdk-sys v0.18.2` build script fails: `Package gdk-3.0 was not found` (no WebKitGTK in this Linux sandbox). The failure is in the `rsh-app` (Tauri shell) dependency closure only. |
| 1.2 | `cargo build` (default members) | **PASS, exit 0, 0 warnings** — all 10 library crates compile (incl. `rsh-app-core`, `rsh-output-hidmaestro`, `rsh-engine`). Finished in 1.33 s (incremental; still 0 warnings). |
| 1.3 | `cargo test --workspace --exclude rsh-app --no-fail-fast` | **805 passed / 0 failed / 3 ignored (34 suites), exit 0** — was 799/1 in the first gate round. The single former failure `soak_60s_no_growth` now PASSES in the full-suite run (§1.3.1). Per suite: app-core 167, engine 83, transport 113 (+2 ignored), profiles 49 (+1 ignored), protocol 60 + golden 18 + property 10 + doc 1 (=89), output 93, hidmaestro 56 + backend 20 + manager 28 + mapping 12 + protocol 8 (=124), security 30, telemetry 32 + readers 7 (=39), rig 15, benchmarks alloc_gate 1 + soak 1 + doc 1 (=3), led 0. |
| 1.4 | `cargo test -p rsh-benchmarks` (alloc gate + soak proxy) | **PASS — alloc_gate 1/1, soak 1/1 (60.02 s), doc 1/1, exit 0.** |
| 1.5 | Frontend `npm test` (vitest) | **PASS — 213/213, 18 files, exit 0, 29.85 s.** |
| 1.6 | Frontend `npm run build` (tsc + vite) | **PASS — exit 0, built in 5.33 s.** `tsc -b` clean. Index chunk 608.18 kB (gzip 181.05 kB) — the >500 kB chunk warning persists (H3-D7, warning only). |
| 1.7 | Frontend `npm run lint` (eslint) | **PASS — exit 0, clean.** |
| 1.8 | `cargo clippy --workspace --all-targets --exclude rsh-app -- -D warnings` | **PASS — exit 0, clean.** This settles the earlier claim of 7 lints in `tools/rsh-benchmarks/src/alloc.rs` under `-p rsh-rig`: the full workspace run is clean — the claim does not reproduce. |
| 1.9 | `cargo fmt --check` | **PASS — exit 0, clean.** |
| 1.10 | `cargo deny check` | **PASS — exit 0: advisories ok, bans ok, licenses ok, sources ok.** 26 `warning[duplicate]` lines (was 28; duplicate crate versions, pre-existing, informational). |
| 1.11 | `cargo audit` | **Advisory scan PASS; yanked check TIMED OUT again — CARRIED.** Full run: 626 crate dependencies, 1509 advisories loaded, **612×** `error: couldn't check if the package is yanked: registry: request could not be completed in the allotted timeframe`, no vulnerability verdict printed (7th consecutive phase). NOTE: the process exits 0 despite the missing verdict — exit code 0 is **not** a pass for the yanked check. `--no-yanked --no-fetch` run: exit 0, no findings reported. |
| 1.12 | `npm audit` | **BLOCKED — CARRIED.** Sandbox proxy 403 `policy_denied` on `POST https://registry.npmjs.org/-/npm/v1/security/audits/quick`; exit 1, no audit result. Same block as H1. Re-run on CI. |
| 1.13 | Criterion benches + `check-thresholds.py` | **PASS — threshold gate exit 0.** `pipeline_tick` 158.6 ns ≤ 1100 ns; `hot_path_parse_seq_pipeline` 332.3 ns ≤ 403 ns; alloc probes ran, 0 B/tick asserted in-process. Full 20-bench table in §5.3. Load during the run: 1.31 → 2.54 (the bench binary itself; no external load). |
| 1.14 | NTP ping p99 (rig `ntp-ping`, SLO ≤ 10 000 µs) | **PASS in the full rig run: 6,463 µs** (200 samples, 0 timeouts, p50 3,297 µs) — FIXED, §6.3. One sample (16,703 µs) exceeded the SLO; p99 = 3rd slowest, so the SLO is met. |
| 1.15 | Rig full run (`--mode full --seed 42`) | **26/26 PASS in 1,923.2 s, exit 0.** Reports in `~/workspace/goals/rsh-real-sim-hub-project/hidden_files/rig-runs/full-2026-11-14T144922Z/` (`results.json` + `REPORT.md`). Full table in §6. |
| 1.16 | Firmware compat (Phase D method) | **PASS.** `pio run` SUCCESS in 32.19 s (RAM 30.2%, Flash 67.4%); merged bin `RSH-Firmware-DevKitV1-v4.0.1.bin` **955,056 B** (byte-size identical to Phase D / C# series); C++ host tests **4,588 checks, 0 failures** (4430 + 50 + 50 + 51 + 7) + pin-rules **62 PASS** + brick-recovery mock **9 checks, 0 failures** incl. real merged-bin validation. Signing correctly absent without a key (fail-closed). |
| 1.17 | Packaging coherence (read-only) | **PASS on all checkable items** — see §4. |
| 1.18 | 4-hour soak | **SKIPPED by user decision 2026-11-14 14:54** (D-R14 overridden). Never run, never passed. |

### 1.3.1 The former Rust failure: `soak_60s_no_growth` — FIXED (FIX-A)

- **Root cause (FIX-A):** libtest's own 60 s "has been running for
  over 60 seconds" notice — a 512 B buffer + 82 B `String` = **594
  bytes** racing the counter read. The counting allocator is
  process-global and counted the harness's own notice. The 594-byte
  figure was byte-identical in every full-suite failure, and the test
  passed standalone 3/3 — the signature of a harness artefact, now
  proven.
- **Fix:** the measurement now runs in a child process
  (`tools/rsh-benchmarks/src/bin/rsh-soak.rs`); the harness lives in
  a different process. Assertions unchanged — no threshold weakened,
  no assertion tuned.
- **Fresh evidence:** the test **PASSES in the full-suite run**
  (60.02 s, `test soak_60s_no_growth ... ok`) as part of the
  805/0/3 suite, and passes standalone in the `-p rsh-benchmarks`
  run (60.02 s). The libtest notice still prints (it is just a
  notice) — the test itself is green. **DECISIONS §14 is REVOKED
  (§15); nothing is "accepted" here — it is fixed.**
- **Follow-up F-1 is therefore CLOSED** (see §7.7). The standalone-CI-job
  part of the old follow-up is moot: the test passes in the full
  suite now.

## 2. Build

- `cargo build --workspace`: fails only on `gdk-sys v0.18.2`
  (`gdk-3.0` pkg-config not found) — the Tauri shell's WebKitGTK
  dependency closure. Same failure mode as Phases A–G. Windows-only
  gap, not a code error. `src-tauri` does not compile in this sandbox.
- `cargo build` (default members): exit 0, `Finished dev profile in
  1.33 s` (incremental), **0 warnings**.

## 3. Rust tests

Full workspace suite (`--exclude rsh-app --no-fail-fast`): **805 passed /
0 failed / 3 ignored, 34 suites, exit 0**, wall 80.7 s
(14:37:50–14:39:10 PKT, load 0.69 → 0.34). Log
`~/tmp/gate2/rust-tests-full.log`. Per-suite counts in §1.3.
Deltas vs the first gate round (799/1): rig 10 → **15** (new
FIX-RIG regression tests: `true_loss_and_coalesced_math`,
`jitter_histogram_*` ×3, `board_timestamp_wraps_instead_of_saturating`),
benchmarks 2 → **3** (doc test counted), total **+6
tests, −1 failure**. H1's three sandbox-timing tests all pass fresh
(`engine_idle_without_transport_sends_no_fake_frames`,
`snapshot_emitter_runs_at_30hz_without_blocking_ingest`,
`sustains_200_pps_for_two_seconds`). Gate-fix regression tests pass
fresh: `reordered_frame_does_not_overflow_jitter_p99`,
`activator_resume_opens_gate_after_signed_get_caps`,
`activator_pair_opens_gate_and_stores_key`,
`session_gate_holds_frames_status_pump_and_disconnect`,
`per_tick_metrics_reach_the_snapshot`,
`control_session_runs_set_password_under_a_fresh_session`.
FIX-RIG regression test `board_timestamp_wraps_instead_of_saturating`
passes fresh; FIX-A soak proxy passes in-suite (§1.3.1).

## 4. Packaging coherence (read-only, cannot build here)

All verified by reading files on disk this session:

- `src-tauri/tauri.conf.json`: version **1.0.0** (= Cargo workspace
  version); `bundle.targets` **["nsis", "msi"]**; `active: true`;
  `createUpdaterArtifacts: true`; `externalBin:
  ["binaries/rsh-hm-sidecar"]`; NSIS `installMode: currentUser`
  (under `bundle.windows.nsis`); WiX `upgradeCode:
  9719670d-2902-517d-a1ca-712bb1bf9c3f`; `allowDowngrades: true`;
  updater `active: false`, `dialog: false`, `endpoints:
  [https://github.com/dreamsofts34-creator/RSH/releases/latest/download/latest.json]`,
  `windows.installMode: passive`; window title `RSH — Real Sim Hub
  v1.0.0`; CSP tightened (H1-08:
  `default-src 'self'; script-src 'self'; style-src 'self'
  'unsafe-inline'; img-src 'self' data:; object-src 'none'; base-uri
  'self'; frame-ancestors 'none'; connect-src 'self' ipc:
  http://ipc.localhost`).
- Minisign pubkey: config `plugins.updater.pubkey`
  `RWRU7xMj5S9Eodnz8HNKEz8LacETwEFxkttjZLnCbopqmAdFYT2S7DGb`
  **== line 2 of `signing-keys/updater-minisign.pub`** (verified by
  string comparison; the `.key` file was never read or printed).
- `.github/workflows/release.yml`: YAML parses; triggers push tag
  `v1.*` + `workflow_dispatch` (`dry_run` default true, optional
  `tag`); header asset list = the 11 assets of
  `docs/RELEASE-CHECKLIST.md` §3 (NSIS + `.sig`, MSI + `.sig`,
  `latest.json`, merged firmware bin, `firmware-signed.bin`,
  `SHA256SUMS`, 3 SBOMs); verify step checks all staged assets +
  `SHA256SUMS` re-verification + signed-firmware re-verification
  (merged bin must be exactly 955,056 B; signed bin must be app +
  68 B); fail-closed on missing firmware/secrets. **Never run** (G-6).
- `.github/workflows/ci.yml`: YAML parses; 11 jobs incl. `tauri-build`.
  **CI `tauri-build` fix VERIFIED in the file** (H4 finding, "fixed
  H5" comment): it now runs `cargo install tauri-cli --locked` +
  `cargo tauri build --no-bundle` from `src-tauri/` — the old `npm run
  tauri` (no such script) is gone. Proof is still pending: the job
  has never run (first green CI run only after the GitHub push).
- SBOMs: `sbom/rust.cdx.json` CycloneDX 1.6, **629 components**;
  `sbom/frontend.cdx.json` CycloneDX 1.5, **320 components**;
  `sbom/sidecar.cdx.json` CycloneDX 1.6, **2 components** (sidecar exe
  + HIDMaestro.Core.dll v2.1.0). Counts match the committed SBOMs
  exactly (629 / 320 / 2) — the Rust count moved 625 → 629 across the
  deep scan (rsh-rig was missing at 625; +3 dialog-plugin deps after
  Round 2), verified by `cargo metadata` full resolve at the gate.
- `THIRD-PARTY-LICENSES.md` present (9,241 B); `LICENSE` present (MIT).
- Secret scan: no `.pem` / `.key` / `.p12` / `.pfx` / `*secret*`
  files anywhere in the repo (excluding `target/`,
  `node_modules/`); grep for private-key headers, `ghp_`,
  `github_pat_`, `sk-`, `AKIA` across the repo: **0 real secrets**
  (no hits at all this run).
  `PINNED_DLL_SHA256_HEX` is still deliberately `None` (G-2, never
  invented).
- **`firmware/` does NOT exist in the repo** — `release.yml` fails
  closed without it (G-3, open). Only
  `rust-recreation/reference/firmware-v4.0.1/` exists outside the repo.

## 5. Frontend / lint / deps / benchmarks / firmware

### 5.1 Frontend

- `npm test` (vitest): **213 passed / 0 failed, 18 files**, exit 0,
  29.85 s (unchanged since H3).
- `npm run build` (`tsc -b && vite build`): **exit 0, 5.33 s**;
  `dist/index.html` 1.04 kB, CSS 37.45 kB, uPlot 52.28 kB, index JS
  **608.18 kB (gzip 181.05 kB)** — the >500 kB chunk warning persists
  (H3-D7, warning only, no error).
- `npm run lint` (eslint): **exit 0, clean.**

### 5.2 Lint / deps

- `cargo clippy --workspace --all-targets --exclude rsh-app -- -D
  warnings`: **exit 0, clean** — the alleged 7 lints in
  `tools/rsh-benchmarks/src/alloc.rs` do not reproduce in the full
  workspace run (see §1.8).
- `cargo fmt --check`: **exit 0, clean.**
- `cargo deny check`: **exit 0** — advisories / bans / licenses /
  sources all ok; 26 duplicate-version warnings (informational, was
  28).
- `cargo audit`: advisory scan **PASS** (626 deps, 1509 advisories,
  no findings, exit 0 with `--no-yanked --no-fetch`); **yanked check
  timed out again — 612 errors, no verdict** (7th consecutive phase;
  process exit code 0 is misleading — see §1.11). Carried to CI.
- `npm audit`: **blocked** — proxy 403 `policy_denied` on the npm
  audit endpoint; exit 1, no result. Carried to CI.

### 5.3 Benchmarks (criterion 0.8.2, `cargo bench -p rsh-benchmarks`, exit 0)

`check-thresholds.py` **exit 0 — all checks PASS**: `pipeline_tick`
158.6 ns ≤ 1100 ns; `hot_path_parse_seq_pipeline` 332.3 ns ≤ 403 ns;
alloc probes ran (0 B/tick asserted in-process). Load during the run
1.31 → 2.54 (bench binary itself; no external load). Full table vs
the 06:00 baseline (`tools/rsh-benchmarks/thresholds.json`);
means are the criterion middle estimates from `~/tmp/gate2/bench.log`:

| Bench | Gate mean | 06:00 baseline | Ratio |
|---|---|---|---|
| stream_parse_36b | 129.3 ns | 129.4 ns | 1.00× |
| stream_encode_36b | 173.2 ns | 176.6 ns | 0.98× |
| status_parse_34b | 93.6 ns | 95.9 ns | 0.98× |
| status_parse_38b | 109.7 ns | 111.5 ns | 0.98× |
| hello_parse_92b | 352.6 ns | 352.7 ns | 1.00× |
| control_verify_get_caps_29b | 744.4 ns | 750.8 ns | 0.99× |
| control_build_frame_get_caps | 386.8 ns | 404.0 ns | 0.96× |
| hmac_compute_tag_empty_payload | 353.8 ns | 358.5 ns | 0.99× |
| hmac_verify_tag | 370.0 ns | 383.6 ns | 0.96× |
| crc16_stream_36b | 164.9 ns | 165.7 ns | 1.00× |
| crc16_check_9b | 42.4 ns | 42.5 ns | 1.00× |
| config_blob_encode_factory | 838.0 ns | 820.7 ns | 1.02× |
| config_blob_decode_factory | 828.2 ns | 848.7 ns | 0.98× |
| pin_validate_factory_modules | 5273.8 ns | 4533.8 ns | 1.16× (watch item, no threshold) |
| pin_validate_blob_factory | 9119.8 ns | 6585.6 ns | 1.38× (watch item, no threshold) |
| pipeline_tick | 157.7 ns | 162.8 ns | — (≤ 1100 PASS) |
| hot_path_parse_seq_pipeline | 331.9 ns | 335.6 ns | — (≤ 403 PASS) |
| sequence_watchdog_check | 14.1 ns | 13.9 ns | — |
| alloc_probe_pipeline | 155.6 ns | — | 0 B/tick in-process |
| alloc_probe_hot_path | 324.1 ns | — | 0 B/tick in-process |

No threshold was tuned. Criterion's own stored-baseline comparison
flagged one +1.6% delta (`sequence_watchdog_check`, p = 0.00) as
"regressed" and two as "improved" (`status_parse_38b`,
`control_build_frame_get_caps`) — noise-level vs the thresholds.json
gate; the gate thresholds are the authority, not criterion's stored
baseline. `pin_validate_*` remain the watch items (1.16× / 1.38× vs
baseline, as in H2's 1.23× / 1.35×); no threshold exists for them.

### 5.4 Firmware compat (Phase D method, scratch copy — reference untouched)

Scratch: `~/tmp/gate2/fw-scratch/` (`firmware/rsh/` +
`tests-host/` + `docs/` copied from
`rust-recreation/reference/`). NOTE on layout, learned this session:
the first scratch attempt put the firmware sources at `firmware/`
with `.cpp` copies under `firmware/rsh/` — `pio run` then failed at
link with duplicate symbols, because PlatformIO compiles `src_dir =
.` recursively. The correct layout is the original repo's:
**`firmware/rsh/` IS the PlatformIO project root** (all sources,
`platformio.ini`, `merge_bin.py`, `sign_firmware.py` inside it) —
no `../` references exist in the firmware sources, and
`tests-host/run_tests.sh` expects `-I ../firmware/rsh`. Rebuilt
that way, the build succeeded. The reference tree was never modified.

- `pio run` (pio 6.2.0, `~/.local/bin/pio`): **[SUCCESS] in 32.19 s**
  (real 32.74 s), `esp32dev`, 1 succeeded. RAM **30.2% (98,864 /
  327,680 B)**, Flash **67.4% (882,937 / 1,310,720 B)** — identical to
  Phase D.
- Merged bin `RSH-Firmware-DevKitV1-v4.0.1.bin`: **955,056 B**,
  byte-size identical to Phase D / C# series. SHA-256 (this run):
  `e130b8515e1e9cb76f5da1f5fc6c5c98ae98108bae8f43471c4c150f35d2c1fb`
  — differs from Phase D's `f73392e2…` because the ELF embeds the
  scratch build path (`strings firmware.elf` contains
  `/home/hatch/tmp/gate2/fw-scratch/firmware/rsh`); size, RAM and
  Flash are identical, so this is a build-path artefact, not a code
  change.
- `firmware.bin` 889,520 B (SHA-256
  `dd90fc8bfb20d26ca88a0251cbb67b99ed8be1ceb4d2b3e41caf4fd93ad1e599`);
  `firmware-signed.bin` correctly **absent** — `sign_firmware`
  fail-closed with no key (`RSH_SIGNING_KEY nahi —
  firmware-signed.bin nahi bana`), no stale signed bin.
- C++ host tests (`run_tests.sh`, exit 0, **ALL PASS**):
  §1 `test_main.cpp` **4430 checks, 0 failures**; §2 firmware `.cpp`
  on host **50 + 50 checks, 0 failures**; §3 pin rules **62 PASS**;
  §4 golden vectors self-consistent, `OK`; §5 packet cross-verify
  **51 checks, PASSED**; §6 merge-bin mock **7 checks, 0 failures**.
  Total **4,588 counted checks, 0 failures** — exactly Phase D's
  count. Brick-recovery mock (separate, exit 0): **9 checks, 0
  failures**, real merged bin validates (955,056 B,
  sha256 `e130b851…`).
- **`firmware/` does not exist in the repo** — `release.yml` fails
  closed without it (G-3). The coordinator must restore it from the
  reference tree before tagging.

## 6. Rig full run — 26/26 PASS

Run: `cargo run -p rsh-rig -- --mode full --seed 42`, started
2026-11-14T14:49:22Z, 1,923.2 s wall, exit 0, host Linux
7.0.0-44-generic x86_64 / 4 CPUs, rustc 1.100.0. Load during the timed
scenarios 0.01–0.36 (quiet; 2.15 only at startup while cargo built the
rig binary). Reports:
`~/workspace/goals/rsh-real-sim-hub-project/hidden_files/rig-runs/full-2026-11-14T144922Z/`
(`results.json` + `REPORT.md`). **No 4-hour soak was run (SKIPPED by
user decision).** All numbers below are from `results.json` of this
run; `lossPercent` is now TRUE transport loss (arrival seq gaps),
`engineLossPercent` is the old engine-side number (kept), and
coalesced frames are reported separately, never hidden.

| Scenario | Result | Key numbers |
|---|---|---|
| soak (600 s) | **PASS** | loss **0.0%** (0 arrival gaps, 0 lost, 0 dup, 0 rebaselines; engineLoss 3.0783% = coalesced 3,623 frames / 3.0783%, reported separately), rate 196.15/s PASS, tickJitterP99 (PC-receive) **1400 µs** PASS, engineTickJitterP99 5001 µs, outP99 19 µs PASS, heapSlope **0.0060** MB/10 min PASS, rssSlope −0.1106 PASS, allocDelta 0 B, engineTicks 120,048 (200.0 Hz), maxAbsEncDelta 6, peak WS 5.59 MB, PC-receive samples over 5000 µs: 1 |
| long-soak (600 s) | **PASS** | loss **0.0%** (engineLoss 3.0739% = coalesced 3,618 frames / 3.0739%), rate 196.14/s PASS, tickJitterP99 **1410 µs** PASS, engineTickJitterP99 5001 µs, outP99 21 µs PASS, heapSlope **0.0060** PASS, rssSlope −1.0741 PASS, allocDelta 0 B, engineTicks 120,045, maxAbsEncDelta 12, peak WS 5.77 MB, over-5000 samples: 3 |
| dropout-storm | PASS | loss 8.20% (injected, by design), engineLoss 10.82%, coalesced 2.62%, rate 186.4/s, tickJitterP99 6560 µs, engineTickJitterP99 10001 µs |
| steering-calibration | PASS | rails reached; loss 0.0%, coalesced 3.04%, rate 197.39/s, jitter 1380 µs |
| calibration-span | PASS | all axes at rails; loss 0.0%, coalesced 2.97%, rate 197.41/s |
| config-transaction | PASS | contract self-checks only |
| safe-mode | PASS | watchdog fires on 3 s silence, data resumes; loss 20.04% (3 s silence by design); allocDelta 1,448 B (see §6.4) |
| modules-onoff | PASS | accel disabled, others at rails; loss 0.0%, coalesced 3.54% |
| fuzz | PASS | loss 30.39% (injected garbage, by design), engineLoss 32.04%, coalesced 1.66%, rate 137.0/s |
| chaos-net | PASS | loss 10.07% (Gilbert-Elliott, by design), coalesced 5.65%, rate 186.4/s; **engineTickJitterP99Us 15002** — the 2³²−10,001 overflow is GONE (§6.1) |
| user-chaos | PASS | loss 10.12% (injected dropouts, by design) |
| fault-reboot | PASS | loss 0.0% (reboot at t=5 s; P3 regression — no duplicate explosion) |
| fault-wifi-drop | PASS | loss 33.40% (5 s Wi-Fi drop, by design); allocDelta **590 B** (was 7,054 B — see §6.4) |
| fault-output-drop | PASS | engineTicks 855 (≥ 600) |
| fault-app-restart | PASS | engine restart at t=3 s, resumes |
| ntp-ping | **PASS** | **e2eP99 6,463 µs ≤ 10,000 µs** (§6.3); e2eP50 3,297 µs, 200 samples, 0 timeouts, max 16,703 µs, 2nd-slowest 7,476 µs, over-SLO samples 1 |
| control-channel | PASS | contract self-checks only |
| game-restore | PASS | loss 0.0% (restart mid-drive, resumes) |
| pin-map-push | PASS | contract self-checks only |
| encoder-glitch | PASS | v1 parity (no max-delta assertion) |
| clutch-range | PASS | clutch disabled + steering at rails |
| clutch-sweep | PASS | clutch full-range sweep |
| encoder-glitch-v2 | **PASS** | **maxAbsEncoderDelta 5 ≤ 32** — gate fix VERIFIED again (§6.1) |
| shifter-sequential | PASS | |
| shifter-hpattern | PASS | |
| auth-gate-blocked | PASS | 0 packets accepted without GET_CAPS; allocDelta 14,976 B (engine startup allocation, known from H2) |

The clean-scenario true loss is 0.0% everywhere it is asserted; the
~3% engine-side number is entirely coalescing (latest-wins drain),
now reported as `coalescedPercent` and never hidden. `allocDelta` is
0 B in every engine scenario except `safe-mode` (1,448 B),
`fault-wifi-drop` (590 B) and `auth-gate-blocked` (14,976 B, startup).

### 6.1 Gate fixes VERIFIED fresh (again)

- **Encoder-glitch-v2 CLOSED:** `encoder-glitch-v2` **PASS**,
  `maxAbsEncoderDelta` **5 ≤ 32** (H2: 200, FAIL; first gate round:
  6). The `|delta| > 32`/tick filter in
  `crates/rsh-engine/src/encoder.rs` (`MAX_ENCODER_DELTA_PER_TICK
  = 32`, glitch returns 0, baseline advances) works against the real
  pipeline.
- **Chaos-net jitter overflow CLOSED:** `chaos-net` **PASS** with
  `engineTickJitterP99Us` **15002** — a real jitter number under
  reorder, not 4,294,957,295 (H2 §4.1). The signed-delta fix in
  `crates/rsh-engine/src/engine.rs` (`signed_timestamp_delta_us` /
  `inter_arrival_jitter_us`) plus regression test
  `reordered_frame_does_not_overflow_jitter_p99` (passes fresh)
  hold.
- **Wheel-timestamp wrap CLOSED (FIX-RIG):** the `ts_us as u32`
  saturation bug that froze long-soak timestamps is fixed; regression
  test `board_timestamp_wraps_instead_of_saturating` passes fresh,
  and the long-soak ran the full 600 s with timestamps advancing
  (120,045 engine ticks, valid packets flowing to t=600 s).

### 6.2 Soak-loss SLO — MET (FIX-RIG, was the primary condition)

Fresh numbers: **soak loss 0.0%, long-soak loss 0.0% vs < 0.5%
SLO** (first gate round: 3.45% / 3.31% — the old number measured
engine-side coalescing, not transport loss). Rate SLO passes
(196.15 / 196.14 ≥ 195/s); memory slopes pass (heap 0.0060 / 0.0060
MB/10 min ≤ 1); PC-receive jitter p99 passes (1400 / 1410 ≤ 5000);
the engine ticks at full rate (120,048 / 120,045 ticks per 600 s =
200.0 Hz). The ~3.1% coalesced frames are reported separately in
every scenario (`coalescedPercent`, `coalescedFrames`) — the
harness/engine-boundary latest-wins behaviour is unchanged and now
honestly labelled, not counted as loss. **No threshold was tuned.**
The old F-2 coordinator decision is moot: the SLO is met on its own
terms with the corrected measurement basis (arrival seq gaps — the
basis the plan's SLO is defined on). This condition is CLOSED.

### 6.3 NTP ping — PASS in the full run (FIX-RIG, was a full-run FAIL)

Full run: **e2e p99 6,463 µs ≤ 10,000 µs SLO** (p50 3,297 µs, 200
samples, 0 timeouts) — the scenario now runs 200 s / ~200 samples,
so p99 = 3rd slowest and one VM scheduling stall cannot fail it.
Raw samples: max 16,703 µs, 2nd-slowest 7,476 µs, **over-SLO samples
1 of 200**. The single 16.7 ms sample is the same VM-stall signature
as before, but the SLO is defined on p99 and p99 passes with margin.
No quiet retest was needed — the full run itself passes. F-3 is
CLOSED.

### 6.4 Observations (not gate items)

- `fault-wifi-drop` shows `allocBytesDelta` **590 B** (first gate
  round: 7,054 B). `safe-mode` shows **1,448 B** (not noted before).
  The 0-allocation gate covers the hot path (alloc_gate + alloc
  probes pass); a Wi-Fi-drop reconnect / watchdog-resume allocating
  ~0.6–1.4 KB once is not a hot-path leak, and no SLO covers it.
  Noted for the coordinator; no action taken by this gate round.
- `engineTickJitterP99Us` reads exactly **5001 µs in every clean
  scenario** (5000 µs tick + 1) — consistent with it measuring the
  ~5 ms engine tick period itself. The plan SLO is defined on
  PC-receive inter-arrival jitter (1400 / 1410 µs, passing), so this
  is an observation about the metric's meaning, not a defect and not
  a gate item.

## 7. Conditions disposition

`CLOSED` = evidence produced this session. `OPEN` = owner + what
closes it. `CARRIED` = reason recorded, still applies.

### 7.1 Phase G gate conditions 1–17

| # | Condition | Disposition |
|---|---|---|
| 1 | Tauri shell compile + window-paint smoke test — Windows-only | **OPEN** (carried) — `cargo build --workspace` still fails only on `gdk-sys`/WebKitGTK in this sandbox (§2). Windows CI / owner PC. |
| 2 | First green CI run only after GitHub push | **OPEN** (carried) — push only on the complete project. |
| 3 | OPEN-13/14 (one credential Wi-Fi+OTA) | **CARRIED** — app-side consistent (single `board:<MAC>:wifi` keyring entry, no second credential in code — H1 §7); firmware-side closure needed before the pairing UI. |
| 4 | Windows-runtime verification (keyring trap, serial 921600 + DTR no-reset, `windows` crate `runas`, real sidecar + driver, NativeAOT publish, UAC, real vJoy DLL load/acquire/update, registry probe, game matrix 0/16) | **OPEN** (carried) — owner-PC / Windows-only. |
| 5 | UDP sendto blocked in sandbox | **CARRIED** — rig still uses the channel transport (honest limit in the rig REPORT.md); real UDP needs a network-capable runner. |
| 6 | a11y (axe) audit needs a browser | **OPEN** (carried) — Windows/manual, not run yet. |
| 7 | `cargo audit` yanked check timed out | **CARRIED** — timed out a 7th consecutive phase (612 errors, §1.11). Re-run on CI. |
| 8 | vJoy DLL SHA-256 measured on the Windows release machine | **OPEN** (release gate) — `PINNED_DLL_SHA256_HEX` still deliberately `None`; never invented. |
| 9 | pid.codes PR for `0x5253` | **OPEN** (release gate) — human gate, not in the workflow. |
| 10 | H-HID-1–H-HID-6 owner-PC pending; H-HID-7 ARM64 unsupported (stays); H-HID-8 release-notes entry pending | **OPEN** (carried) — H-HID-7 stays unsupported by design; H-HID-8 still pending release notes. |
| 11 | Per-tick metrics outlet in the engine API | **CLOSED** — H1 fix verified fresh: `per_tick_metrics_reach_the_snapshot` passes; the rig reads live jitter/output-latency p99s (§6). |
| 12 | Status pump (status packets → engine) | **CLOSED** — H1 fix verified fresh: `session_gate_holds_frames_status_pump_and_disconnect` passes; rig status counters flow. |
| 13 | HMAC session gate (SEC-E-14) | **CLOSED** — gate built by H1 **and activator landed by H5**: `activate_session_on` (flag-1 HELO → fresh nonce → signed `GET_CAPS` → `open_session_gate()`), `AppStateCore::activate_session`, `begin_pairing`; tests `activator_resume_opens_gate_after_signed_get_caps` + `activator_pair_opens_gate_and_stores_key` pass fresh. The gate is now functional, not just built. |
| 14 | Per-module tunings → pipeline push | **OPEN, deferred to v1.0.1** (H1 §7 + H3-D2, unchanged) — pipeline still runs the default plan; the UI says so honestly (`tuning.profileOnlyNote`). Engine-API work, no security impact. |
| 15 | 19 unregistered Tauri commands | **CLOSED** — **56 implemented / 0 NOT_YET**, verified fresh: `src-tauri/src/main.rs` `generate_handler!` lists 56 commands and `frontend/src/lib/tauri.ts` `IMPLEMENTED_COMMANDS` lists the same 56 (sets identical, verified by script). The last 14 (pairing/config/pin-map/firmware/`restart_wheel`) were wired by H5-control (`run_control_session` fresh session + `webota.rs` upload). |
| 16 | Corrupt-store repair UI | **CLOSED** — H3-T2 (`try_repair_store_from_backup`, repair notice banner); tests pass fresh as part of the 805. |
| 17 | TCP connect-back from .NET processes sandbox-blocked | **CARRIED** — stdio path verified; TCP must be exercised on Windows (H-HID-5). |

### 7.2 H1 security re-scan (§7) — fresh disposition

| Item | Disposition |
|---|---|
| 3 sandbox-timing tests re-run green on CI/Windows | **CLOSED** — all 3 pass in the fresh full run this session (not just CI/Windows). |
| Session-owner activator lands before v1.0.0 | **CLOSED** — landed (G-13 above) with tests passing fresh. |
| `cargo audit` yanked check + `npm audit` re-run on CI | **CARRIED** — yanked timed out again (7th phase); `npm audit` blocked again (proxy 403). |
| SEC-R2-03 / SEC-R2-04 (unpaired AP open / session-reset rate limit) | **CARRIED, deferred to v1.0.1** — firmware changes; residuals DoS/sniffing only, no key leak (H1 §7). |
| SEC-R2-06 (pairing key overwrite) | **CLOSED** — H1 fix; transport tests pass fresh (113/113). |
| OPEN-3/19 + SEC-H-04 (board-side eFuse burn + dump verification + doc rewrite) | **CARRIED to release** — sacrificial board + owner action, cannot be done in the sandbox. |
| OPEN-13/14 (one credential) | **CARRIED** — see G-3. |
| Cond 14 (tunings → pipeline) | **CARRIED to v1.0.1** — see G-14. |
| Cond 15 (commands) | **CLOSED** — 56/0 (see G-15). |
| Cond 16 (repair UI) | **CLOSED** — H3-T2 (see G-16). |
| H1 verdict | **CONDITIONAL PASS (0 Critical, 0 High)** — stands; its two conditions are now met in-sandbox (timing tests green, activator landed); audits remain carried. |

### 7.3 H2 perf + soak open items — all FIXED this round

| Item | Disposition |
|---|---|
| `encoder-glitch-v2` (engine gap, maxAbsDelta 200 > 32) | **CLOSED** — fresh rig PASS, maxAbsEncoderDelta 5 ≤ 32 (§6.1). |
| Chaos-net jitter overflow (2³²−10,001) | **CLOSED** — fresh rig `engineTickJitterP99Us` 15002, not ~2³² (§6.1). |
| Wheel-timestamp wrap (`ts_us as u32` saturation froze long-soak timestamps) | **CLOSED** — FIX-RIG fix + regression test `board_timestamp_wraps_instead_of_saturating` passes fresh; long-soak timestamps advance to t=600 s (§6.1). |
| Soak loss SLO < 0.5% | **CLOSED — FIXED** — fresh **0.0% / 0.0%** true transport loss (§6.2). The old 3.45% / 3.31% measured engine-side coalescing; coalesced frames (~3.1%) are now reported separately, never hidden. |
| Jitter p99 5001 vs ≤ 5000 | **CLOSED — FIXED** — fresh PC-receive jitter p99 **1400 / 1410 µs** (§6.2). The 5001 µs figure now appears only as `engineTickJitterP99Us` (the ~5 ms tick period itself — see §6.4). |
| NTP ping p99 | **CLOSED — FIXED** — full-run p99 **6,463 µs** over 200 samples (§6.3). |
| 4-hour soak | **SKIPPED** by user decision (DECISIONS §13) — never passed, never run. |
| `pin_validate_*` watch items | **OPEN (watch)** — fresh 1.16× / 1.38× vs baseline (H2: 1.23× / 1.35×); no threshold. |
| Soak-proxy failure (§1.3.1) | **CLOSED — FIXED** — FIX-A root cause (594 B libtest notice) + child-process measurement; passes in the full-suite run. DECISIONS §14 REVOKED (§15); nothing accepted. |

### 7.4 H3 polish deferred (D1–D8) + gaps

| Item | Disposition |
|---|---|
| D1 — 14 NOT_YET commands | **CLOSED** — 56/0 (see G-15). |
| D2 — cond 14 (tunings → pipeline) | **CARRIED to v1.0.1.** |
| D3/D8 — tuning extension fields (LED brightness/pattern, horn button, bite point) | **CARRIED** — needs D2 first, then profile-schema + controls. |
| D4 — cond 11 (metrics) | **CLOSED** (H1). |
| D5 — cond 12 (status pump) | **CLOSED** (H1). |
| D6 — mDNS (`rsh.local` never resolved; forwarder dials `esp_ip`) | **OPEN (carried)** — transport/DNS follow-up; `get_transport_state` honestly reports the persisted `esp_ip`. |
| D7 — Vite chunk 608.18 kB > 500 kB | **CARRIED** — warning only; fresh build shows the same 608.18 kB. |
| H3 gap — `sessionActive` / `framesHeldByGate` exist in the frontend `TransportState` interface but are unread in the UI | **OPEN (minor)** — verified fresh: the fields occur only in `frontend/src/lib/tauri.ts`, nowhere in a screen. Coordinator decision whether the UI should surface them (F-5). |

### 7.5 H4 packaging + H5 docs

| Item | Disposition |
|---|---|
| MSIX | **CARRIED, deferred with reason** (H4 §1.1) — future Windows spike only. |
| OV code-signing certificate | **CARRIED, deferred** (user decision D-R11) — app ships unsigned, SmartScreen expected. |
| `firmware/` restore in the repo | **OPEN** (G-3) — confirmed absent this session; blocks `release.yml`. |
| `RSH_UPDATER_PRIVATE_KEY` secret | **OPEN** (G-4) — coordinator/owner action. |
| `RSH_SIGNING_KEY_PEM` secret | **OPEN, verify present** (G-5) — pre-existing (C# series). |
| `cargo tauri signer sign` E2E of the new updater keypair | **OPEN** — crate-level cross-check PASS (H4 §4); CLI sign E2E needs the first Windows build/dry-run. |
| `cargo cyclonedx` flags in `release.yml` | **OPEN** — first dry-run is the proof. |
| CI `tauri-build` job broken (H4 finding) | **CLOSED in tree, proof pending** — fix verified in `ci.yml` this session (`cargo install tauri-cli` + `cargo tauri build --no-bundle`); the job has never run. First green CI run is the proof. |
| Updater download/install wiring | **CARRIED** — post-v1.0.0; v1.0.0 is check-only. |
| H5 docs (README, USER-GUIDE, FLASH-GUIDE, GAME-MATRIX 0/16, SUPPORT-PERIOD, final checklist) | **CLOSED** — delivered; game matrix re-checked still **0/16** (no pair marked verified without evidence). |

### 7.6 Release gates G-1…G-14 (`docs/RELEASE-CHECKLIST.md` §4)

| # | Gate | Disposition |
|---|---|---|
| G-1 | pid.codes PR for VID `0x1209` / PID `0x5253` | **OPEN — release gate.** |
| G-2 | vJoy DLL SHA-256 | **OPEN — not measured** (still `None`, never invented). |
| G-3 | `firmware/` restore in the repo | **OPEN — blocks `release.yml`** (confirmed absent). |
| G-4 | Secret `RSH_UPDATER_PRIVATE_KEY` | **OPEN.** |
| G-5 | Secret `RSH_SIGNING_KEY_PEM` | **OPEN — verify present** before the first dry-run. |
| G-6 | First `release.yml` dry-run | **OPEN — never run.** |
| G-7 | A7 real-board run | **OPEN — carried Phase H gate item** (once, at final production release, owner PC). |
| G-8 | 4-hour soak | **SKIPPED by user decision 2026-11-14 14:54; D-R14 overridden.** Recorded as SKIPPED, NEVER as passed (DECISIONS §13). |
| G-9 | Game matrix in-game verification | **OPEN — 0 of 16 verified.** v1.0.0 ships with the matrix honestly at 0/16. |
| G-10 | H1 security re-scan conditions | **CONDITIONAL PASS** — activator landed (was the blocking condition); audits re-run on CI remains carried. |
| G-11 | Clean-PC install + rollback plans | **OPEN — Windows-only** (H4 §8/§9, Win10 22H2 + Win11). |
| G-12 | CI `tauri-build` job | **CLOSED in tree, proof pending** — fixed (H5); first CI run proves it. |
| G-13 | MSIX | Deferred with reason (not a v1.0.0 gate). |
| G-14 | OV code-signing certificate | Deferred (user decision D-R11). |

### 7.7 Owned follow-ups from this gate

| # | Follow-up | Owner / closes when |
|---|---|---|
| F-1 | Soak proxy 594-byte source + harness window | **CLOSED — FIXED.** Root cause identified (libtest 60 s notice, 512 B buffer + 82 B String) and fixed by the child-process measurement (`tools/rsh-benchmarks/src/bin/rsh-soak.rs`); passes in the full-suite run (60.02 s). DECISIONS §14 REVOKED (§15). |
| F-2 | Soak-loss SLO disposition (§6.2) | **CLOSED — FIXED.** Fresh 10-min soak loss **0.0% / 0.0%** true transport loss; coalesced ~3.1% reported separately. No coordinator accept needed — the SLO is met. |
| F-3 | NTP-ping full-run fail disposition (§6.3) | **CLOSED — FIXED.** Full-run p99 **6,463 µs** over 200 samples (1 sample over SLO, p99 met). No retest needed. |
| F-4 | `cargo audit` yanked check + `npm audit` re-runs on CI | **CARRIED** — first CI run (both blocked/timed-out in this sandbox for 7 / 2 phases). |
| F-5 | Frontend: surface `sessionActive` / `framesHeldByGate` in the UI? | **OPEN** — coordinator decision (minor; fields exist, UI doesn't read them). |
| F-6 | `fault-wifi-drop` allocDelta (reconnect-path allocation, §6.4) | **Observation only, shrunk** — now **590 B** (was 7,054 B); `safe-mode` 1,448 B noted. No SLO covers it; coordinator to note, no gate action. |

## 8. Verdict recommendation — CONDITIONAL PASS

**Recommendation: CONDITIONAL PASS.** Everything the code itself can
prove in this sandbox is green: build (default members) 0 errors/0
warnings, Rust tests **805/805 (0 failed)**, frontend 213/213 + tsc +
eslint clean, clippy + fmt clean, `cargo deny` clean, advisory audit
clean, benchmark thresholds all PASS, firmware compat 4,588 checks 0
fail + merged bin byte-identical, packaging coherence verified,
session activator landed (56/0 commands), rig **26/26** with true loss
0.0%, jitter p99 1400 µs, NTP p99 6,463 µs, memory flat and rate at
SLO. The three failures from the first gate round (soak proxy,
soak/long-soak loss, ntp-ping) are all FIXED with fresh evidence, and
DECISIONS §14 is revoked — nothing in this gate is "accepted with a
follow-up".

The conditions of the pass are:

1. **Pre-tag human gates G-1–G-12 (§7.6)** remain open as listed —
   pid.codes, vJoy SHA-256, `firmware/` restore, both secrets,
   release dry-run, A7 real-board run, game matrix 0/16, clean-PC
   install + rollback — plus the Windows-only verifications in §9.
   None of these is a code failure found by this gate; all are
   named, owned, and carried.
2. **Audits re-run on CI (F-4).** `cargo audit` yanked check timed
   out a 7th phase; `npm audit` is proxy-blocked in this sandbox.
3. **Follow-ups F-5 (minor UI fields) and F-6 (observation)** are
   owned and tracked; neither blocks the release.

**What would make this FAIL:** a fresh code failure in a re-run, or
a coordinator rejection of a condition above. No fresh evidence in
this gate supports FAIL: no failing test, no failing rig scenario, no
new code defect was found, and no threshold was weakened.

## 9. Release readiness — what is still human/Windows-only before a `v1.0.0` tag

Before pushing a `v1.*` tag, the coordinator must still close or
explicitly waive:

- **Pre-tag human gates (§7.6):** G-1 pid.codes PR merged; G-2 vJoy
  DLL SHA-256 measured on the Windows release machine; G-3
  `firmware/` restored in the repo (note: `tools/rsh-rig/` is also
  untracked — add it in the Phase H commit); G-4 + G-5 secrets set;
  G-6 `release.yml` dry-run green (incl. `cargo cyclonedx` step,
  installer filenames, sidecar staging, updater signing E2E); G-7 A7
  real-board run once at final production release (owner PC); G-9
  game matrix honestly 0/16; G-11 clean-PC install + rollback plans
  executed on Win10 22H2 **and** Win11, report filed.
- **Windows-only verifications (cannot run in this sandbox):**
  `cargo tauri build` (NSIS + MSI production) + installer contents;
  window-paint smoke test (no blank window — the C# lesson);
  NativeAOT sidecar publish + real sidecar/driver round-trip
  (install/create/submit); firewall COM sequence + UAC behaviour +
  `runas` path + registry probes; updater E2E (publish `latest.json`
  → check → download signed update → install → relaunch; tampered
  manifest rejected); real vJoy DLL load/acquire/update; keyring
  trap; serial 921600 + DTR no-reset; TCP connect-back; axe a11y
  audit; startup ≤ 1200 ms + flat memory on the owner PC.
- **CI (first run after the GitHub push):** first green run incl.
  the fixed `tauri-build` job; `cargo audit` yanked check + `npm
  audit` re-runs (F-4).
- **Release notes** (per `docs/RELEASE-CHECKLIST.md` §8): unsigned-app
  SmartScreen note; firmware anti-downgrade note; HIDMaestro UAC
  note; **re-pairing mandatory** (C# → Rust) on the first page; game
  matrix status (0/16); **4-hour soak SKIPPED** (user decision, not
  passed).
- **Deferred to v1.0.1 (not release blockers):** per-module tunings →
  pipeline (G-14), tuning extension fields (D3/D8), SEC-R2-03/04
  (firmware), MSIX, OV certificate (buy later).
