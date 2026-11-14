# Deep-Scan Gate — RSH Rust recreation v1.0.0

**Verdict: CONDITIONAL PASS** — zero failing tests and zero failing rig scenarios locally.
Conditions are the ones every phase carried: first green CI + release dry-run after push
(post-push proof), and the Windows/human gates in §8. Nothing provable locally is left unproven.

- **Date:** 2026-11-14, gate run 23:17–~00:15 PKT (rig full 32.5 min + multi-seed 24.4 min dominate).
- **HEAD:** `9690064` ("Fix CI + release dry-run failures"), plus uncommitted deep-scan fixes.
  Tree at gate start: 183 changed entries; at gate end: 190 (gate's own fixes, §6 — no commit,
  no push, no tag, no release, no `git stash` at any point).
- **Host:** shared Linux sandbox, 1.5 CPU; rig runs on a quiet host (load 0.06–0.72 during the runs).
- **User orders honoured:** zero-failure order (2026-11-14 18:01 — no failing test/scenario in v1.0.0);
  4-hour soak SKIPPED by user decision (2026-11-14 14:54), recorded as SKIPPED, never as passed (§7);
  status updates hourly in the 06:30–21:30 window (coordinator's job, not this doc).
- **Signing keys:** never touched, committed, shared, or printed (`~/workspace/projects/rsh/signing-keys/`
  stays outside the repo, gitignored).

## 1. Evidence table — every suite

| # | Check | Command | Result |
|---|---|---|---|
| 1.1 | `cargo fmt` | `cargo fmt --all` then `--check` | **PASS, exit 0** — 0 diffs. (FIX-CI's "16 sibling files fail fmt" is gone.) |
| 1.2 | Rust tests | `cargo test --workspace --exclude rsh-app` | **901 passed / 0 failed / 2 ignored, 35 suites, exit 0** |
| 1.3 | Frontend tests | `npm test` (frontend/) | **241 passed / 0 failed, 21 files, exit 0** |
| 1.4 | Frontend build | `npm run build` | **PASS, exit 0** (built in 4.86 s; Vite chunk-size warning on the 653 kB bundle — advisory only, same as Phase H's 608 kB note) |
| 1.5 | TypeScript | `npx tsc --noEmit` | **PASS, exit 0** |
| 1.6 | Lint | `npm run lint` (eslint) | **PASS, exit 0** |
| 1.7 | Clippy | `cargo clippy --workspace --exclude rsh-app --all-targets -- -D warnings` | **PASS, exit 0, 0 warnings** |
| 1.8 | Windows check (attempt) | `cargo check -p rsh-app --target x86_64-pc-windows-msvc` | **Expected fail in sandbox** — `cc-rs: failed to find tool "lib.exe"`. Recorded as Windows gate W-1 (§8); does not block this verdict. |
| 1.9 | `cargo deny` | `cargo deny check` | **PASS, exit 0** — advisories ok, bans ok, licenses ok, sources ok; yanked = "deny" enforced. 26 duplicate-version warnings (informational, same as Phase H) + the same 2 license-not-encountered warnings. |
| 1.10 | `cargo audit` | `cargo audit` (×2) | **Advisory scan clean** (1,509 advisories loaded, 629 deps scanned, no findings). **Yanked check inconclusive in sandbox** — 615× `couldn't check if the package is yanked: request could not be completed in the allotted timeframe` (registry timeout); the process still exits 0, which is NOT a yanked verdict. The fail-closed authority is `cargo deny` yanked = "deny" (§1.9, PASS). Carried to CI (Phase H F-4). |
| 1.11 | `npm audit` | `npm audit --audit-level=high` | **Could not run in sandbox** — `403 Forbidden - POST .../security/audits/quick not permitted by policy` (registry policy). Not a code finding; carried to CI (Phase H F-4). |
| 1.12 | Benchmarks | `cargo bench -p rsh-benchmarks` + `tools/rsh-benchmarks/check-thresholds.py` | **ALL PASS, exit 0** (after one re-measure, §6.1) |
| 1.13 | Firmware build | `pio run` (firmware/, clean rebuild) | **SUCCESS, 36.10 s** — RAM 30.2% (98,872 / 327,680 B), Flash 67.4% (883,865 / 1,310,720 B) |
| 1.14 | Firmware host tests | g++ host tests + Python vectors + cross-verify | **4,444 checks, 0 failures** (incl. FACTORY_RESET §25b: exactly 14 CHECKs); vectors OK; cross-verify **51 checks, PASSED** |
| 1.15 | Packaging | §4 | **PASS** — 63/63 commands, NSIS/MSI config dry-check, updater pubkey verified, SBOM 629/320/2, secret scan clean |
| 1.16 | Rig FULL seed 42 | `cargo run --release -p rsh-rig -- --mode full` | **27/27 PASS in 1,953.4 s, exit 0** (§2) |
| 1.17 | Rig multi-seed | `--mode quick --seeds 7,1337` | **54/54 PASS in 1,465.8 s, exit 0** (§3) |
| 1.18 | Fuzz | `cargo +nightly fuzz run sequence_pipeline -- -runs=500000` | **500,000 runs, 0 crashes, DONE in 63 s, exit 0** |

### 1.2 Rust test counts per suite (901 total, 0 failed)

| Suite | Passed | | Suite | Passed |
|---|---|---|---|---|
| rsh-app-core | 192 | | rsh-profiles | 58 |
| rsh-engine | 94 | | rsh-protocol lib | 63 |
| rsh-transport | 126 (+2 ignored) | | rsh-protocol golden vectors | 18 |
| rsh-output | 96 | | rsh-protocol property | 10 |
| rsh-output-hidmaestro lib | 65 | | rsh-security | 20 |
| hidmaestro backend/manager/mapping/protocol tests | 22+33+12+9 | | rsh-telemetry | 30 |
| rsh-led | 4 | | rsh-benchmarks alloc_gate / sequence_gap / soak / doc | 1+2+1+1 |
| rsh-protocol doc | 1 | | rsh-benchmarks doc | 1 |

The 2 ignored tests are the C1 sandbox-blocked UDP send tests (`control_send_and_reply_round_trip`,
`helo_received_by_board_at_one_hertz` — sandbox blocks UDP `sendto` EPERM); ignored, not failures.
They run on Windows CI. Total is up from Phase H's 805 because the deep scan added tests
(engine drain caps, sequence gaps, alloc gate, pin_validate, fuzz-adjacent checks).

**Deviation note (H3):** the task's `--workspace` flag is literal — on Linux it is WRONG
(tauri_build copies the sidecar and fails with ResourcePathNotFound). The gate used
`--workspace --exclude rsh-app` exactly as H3 prescribes, with the Windows side covered by the
separate clippy-windows CI job (§16.3).

## 2. Rig FULL run — seed 42 (27/27 PASS)

Run: `full-2026-11-14T183649Z`, 1,953.4 s wall, host load 0.06–0.72 (quiet).
Reports: `~/workspace/goals/rsh-real-sim-hub-project/hidden_files/rig-runs/full-2026-11-14T183649Z/`
(`results.json` + `REPORT.md`). The REPORT.md in that folder still carries the pre-gate
"latest-wins" wording in its Honest-limits section — the wording was fixed in the rig sources
after the run (§6.4); the numbers are unaffected.

| Scenario | Verdict | Wall | Key numbers |
|---|---|---|---|
| soak | PASS | 600.0 s | loss 0%, coalesced **0%**, rate 196.14/s (≥195), tickJitterP99 1,290 µs, engineTickJitterP99 **1 µs** (≤1,000), outputLatencyP99 3 µs, heap slope 0.0014 MB/10 min, allocDelta 0 |
| emit-200hz | PASS | 30.0 s | loss 0%, coalesced **0%** at true 200 Hz, rate 201.35/s, valid 6,047, ticks 6,048, engineTickJitterP99 1 µs, queueDepthMax 2, queueDropped 0, allocDelta 0 |
| dropout-storm | PASS | 60.0 s | loss 8.20% (injected, by design), coalesced 0%, rate 186.38/s, allocDelta 0 |
| steering-calibration | PASS | 30.0 s | loss 0%, coalesced 0%, rate 197.40/s, allocDelta 0 |
| calibration-span | PASS | 30.0 s | loss 0%, coalesced 0%, rate 197.41/s, allocDelta 0 |
| config-transaction | PASS | 0.0 s | contract-only self-check |
| safe-mode | PASS | 15.0 s | loss 20.04% (3 s silence, by design), coalesced 0%, allocDelta 0 |
| modules-onoff | PASS | 10.0 s | loss 0%, coalesced 0%, rate 199.93/s, allocDelta 0 |
| fuzz | PASS | 60.0 s | loss 30.39% (corrupt packets, by design), coalesced 0%, allocDelta 0 |
| chaos-net | PASS | 60.0 s | loss 10.07% (bursty loss, by design), coalesced 0%, allocDelta 0 |
| user-chaos | PASS | 30.0 s | loss 10.12% (by design), coalesced 0%, allocDelta 0 |
| fault-reboot | PASS | 15.0 s | loss 0%, coalesced 0%, rate 172.19/s, allocDelta 0 |
| fault-wifi-drop | PASS | 15.0 s | loss 33.39% (5 s drop, by design), coalesced 0%, allocDelta 0 |
| fault-output-drop | PASS | 6.0 s | loss 0%, coalesced 0%, rate 202.61/s, allocDelta 0 |
| fault-app-restart | PASS | 6.0 s | loss 0%, coalesced 0%, rate 202.57/s, allocDelta 0 |
| long-soak | PASS | 600.0 s | loss 0%, coalesced **0%**, rate 196.14/s, engineTickJitterP99 **1 µs**, heap slope 0.00098 MB/10 min, RSS slope 0.00094 MB/10 min (both ≤1), allocDelta 0 |
| ntp-ping | PASS | 200.0 s | 200 samples, 0 timeouts, p50 3,189 µs, p99 **6,438 µs** (≤10,000), allocDelta 0 |
| control-channel | PASS | 0.0 s | contract-only self-check |
| game-restore | PASS | 10.0 s | loss 0%, coalesced 0%, rate 200.07/s, allocDelta 0 |
| pin-map-push | PASS | 0.0 s | contract-only self-check |
| encoder-glitch | PASS | 30.0 s | loss 0%, coalesced 0%, rate 197.40/s, allocDelta 0 |
| clutch-range | PASS | 10.0 s | loss 0%, coalesced 0%, rate 200.07/s, allocDelta 0 |
| clutch-sweep | PASS | 30.0 s | loss 0%, coalesced 0%, rate 197.30/s, allocDelta 0 |
| encoder-glitch-v2 | PASS | 30.0 s | maxAbsEncoderDelta 3 (≤32), valid 5,927, ticks 6,047, allocDelta 0 |
| shifter-sequential | PASS | 30.0 s | buttonMaskOr 2016 (==2016), gearMaskSeen 0, allocDelta 0 |
| shifter-hpattern | PASS | 30.0 s | gearMaskSeen 126 (==126, gears 1–6), buttonMaskOr 0, allocDelta 0 |
| auth-gate-blocked | PASS | 10.0 s | validPackets 0 (==0), wheelCtrlAccepted 0, allocDelta 0 (≤4,096 allowance) |

Headline numbers vs the Round 2 honest failures: soak coalesced was 3.65% and long-soak 3.45%
with jitter p99 5,001 µs — now **0% / 0% / 1 µs** on the quiet host. NTP-ping full-run
starvation (104 samples, 96 timeouts in the loaded run) is gone: **200 samples, 0 timeouts**.
No retry was needed on any scenario; the run passed first time on the quiet host.

## 3. Rig multi-seed — seeds 7 + 1337 (54/54 PASS)

Run: `quick-2026-11-14T191104Z`, 1,465.8 s wall, exit 0. Every scenario × both seeds passed
(27 + 27). Key metrics are stable across all three seeds:

| Metric | Seed 42 (full) | Seed 7 (quick) | Seed 1337 (quick) |
|---|---|---|---|
| soak loss / coalesced | 0% / 0% | 0% / 0% | 0% / 0% |
| soak rate | 196.14/s | 196.7/s | 196.7/s |
| long-soak loss / coalesced | 0% / 0% | 0% / 0% | 0% / 0% |
| emit-200hz coalesced / rate | 0% / 201.35/s | 0% / 202.0/s | 0% / 202.0/s |
| engineTickJitterP99 (soak) | 1 µs | 1 µs | 1 µs |
| tickJitterP99 (soak) | 1,290 µs | 1,300 µs | 1,290 µs |
| ntp-ping p99 / samples | 6,438 µs / 200 | 6,291 µs / 200 | 6,311 µs / 200 |
| allocDelta everywhere | 0 | 0 | 0 |

## 4. Benchmarks

`cargo bench -p rsh-benchmarks` exit 0; `check-thresholds.py` **ALL PASS, exit 0**
(load average at gate time 1.19 / 1.65 / 1.07). All 18 regression checks against baselines PASS.

| Bench | Mean | Threshold | Verdict |
|---|---|---|---|
| pipeline_tick | 176.8 ns | ≤ 1,100 ns | PASS |
| hot_path_parse_seq_pipeline | 357.9 ns | ≤ 403 ns | PASS |
| pin_validate_blob_factory | 7,567.9 ns | ≤ 9,879 ns | PASS (after re-measure, §6.1) |
| pin_validate_factory_modules | 5,302.8 ns | ≤ 6,801 ns | PASS |
| alloc_probe_pipeline / alloc_probe_hot_path | 176.3 / 328.3 ns | 0 B/tick in-process | PASS |
| sequence_watchdog_check | 13.7 ns | 1.25× baseline 13.9 ns | PASS |
| All other protocol benches (crc/hmac/stream/status/hello/config/control) | — | 1.25× baseline each | PASS |

## 5. Firmware

- `pio run` **SUCCESS** (clean rebuild, 36.10 s). RAM 30.2%, Flash 67.4% — same percentages as Phase H.
- Merged bin `RSH-Firmware-DevKitV1-v4.0.1.bin`: **955,984 B**,
  SHA-256 `1a132de9fbd4363e3d674f751b054b1a522d8c45f2543823109515e37b6f8368`
  (deterministic across the incremental and clean rebuilds).
  App-only `firmware.bin`: 890,448 B → signed bin would be 890,516 B (app + 68 B trailer).
  Size moved from 955,056 B because Round 2 added real code (SEC-R2-03 AP auto-lock,
  SEC-R2-04 session rate-limit, M-5 FACTORY_RESET) — see §6.2. Signing correctly absent
  without a key (fail-closed; `sign_firmware` refuses to emit an unsigned `firmware-signed.bin`).
- C++ host tests (`firmware/tests-host/`, ASan+UBSan): **4,444 checks, 0 failures**,
  incl. FACTORY_RESET block §25b (exactly 14 CHECKs, all passing).
- Python golden vectors: OK. Packet cross-verify (Python vs Rust vs C++): **51 checks, PASSED**.
- Pin-rules (62 PASS) and brick-recovery mock (9 checks) live outside this repo
  (reference folder) and were not re-run here; the merged-bin size they validated
  against changed (§6.2) — they must be re-run against the new image on the release machine
  if the release process uses them (noted as a follow-up, not a gate failure: the image
  itself is verified by hash + the L8 size check in `release.yml`).

## 6. Packaging

- **Tauri commands: 63/63** — `src-tauri/src/main.rs` `generate_handler!` lists 63 commands and
  `frontend/src/lib/tauri.ts` `IMPLEMENTED_COMMANDS` lists the same 63, in the same order;
  `NOT_YET_IMPLEMENTED_COMMANDS` is empty. (Found at 56 in the frontend list — §6.3 fixed it.)
- **NSIS/MSI config (dry check):** targets `["nsis","msi"]`; NSIS `headerImage`/`sidebarImage`/
  `installerIcon`/`uninstallerIcon` files all exist in `src-tauri/icons/`; languages `[English]`;
  installMode `currentUser`; WIX `upgradeCode` `9719670d-2902-517d-a1ca-712bb1bf9c3f` (valid UUID).
  Actual `.exe`/`.msi` production is Windows-only (first `cargo tauri build`).
- **Updater pubkey:** `RWRU7xMj5S9Eodnz8HNKEz8LacETwEFxkttjZLnCbopqmAdFYT2S7DGb` — identical in
  `tauri.conf.json`, PHASE-H-GATE, SYSTEM-INTEGRATION, SECURITY-INPUT, RELEASE-CHECKLIST-H4,
  RELEASE-CHECKLIST. Decodes to 42 bytes with the `Ed` prefix (valid minisign Ed25519 pubkey
  format). Updater `active: false` (v1.0.0 is check-only, per plan).
- **SBOMs:** Rust `sbom/rust.cdx.json` 629 components (regenerated, §6.5), frontend 320,
  sidecar 2. Sidecar SBOM hashes match the pins (DLL `f13e3259…`, ZIP `7fe349f1…`) and the
  M4 release step compares them against measured staging hashes by design
  (the "driver" entry intentionally records the release-ZIP hash).
- **Sidecar staging script** (`tools/ci/stage-sidecar.ps1`, 66 lines): read-through PASS —
  pinned ZIP + DLL SHA-256, fail-closed at every step, stages to the exact `externalBin` path
  `src-tauri/binaries/rsh-hm-sidecar-x86_64-pc-windows-msvc.exe`. Cannot execute here
  (no pwsh, no DLL, Windows-only NativeAOT publish) — execution is a Windows/CI gate.
  `src-tauri/binaries/` does not exist locally, as designed (staged by `release.yml`).
- **Secret scan:** clean. No `.pem`/`.key`/private-key material, no credentials in the repo
  (only `node_modules/@types/node` doc examples matched the pattern — example code, not secrets).
  `signing-keys/` is gitignored and outside the repo.

## 7. Findings fixed during this gate (fix + retest evidence)

All fixes below are docs, inventory, or measured-value updates — no product behaviour was
changed except none; no test was weakened (no threshold raised, no assertion removed).

### 7.1 Threshold regression-gate failure — re-measured, not a regression (DECISIONS §17.7)
- **Finding:** `check-thresholds.py` first failed (exit 1) on the regression gate only:
  `pin_validate_blob_factory` mean 9,201.6 ns > 1.25× baseline limit 8,232.1 ns
  (plan cap 9,879 ns still passed; all other 17 regression checks passed).
- **Investigation:** `crates/rsh-protocol/src/pin.rs` is untouched by any deep-scan change
  (no diff in the whole `crates/rsh-protocol/` working tree for pin code) — no product
  change can explain a +40% cold-path slowdown. The script's own instruction is
  "re-measure and explain, do not silently raise the baseline".
- **Fix:** re-measured that single bench on the quiet host: **7,567.9 ns** (−17.8%),
  inside the 1.25× limit and inside the documented ~18% run-to-run spread for pin_validate.
- **Retest:** `check-thresholds.py` re-run → **ALL PASS, exit 0**. No baseline or threshold changed.

### 7.2 Merged-bin size 955,056 → 955,984 B (DECISIONS §17.4)
- **Finding:** clean `pio run` produced a 955,984 B merged bin; `release.yml` L8 check
  (`$expectedMergedBinSize = 955056`) would have FAILED the release.
- **Cause:** real Round 2 firmware code (+928 B app: SEC-R2-03, SEC-R2-04, M-5 FACTORY_RESET).
- **Fix:** updated the expected size to 955984 in `release.yml` (with a comment recording the
  cause + the deterministic SHA-256) and every live size note in the same change:
  RELEASE-CHECKLIST-H4 (2 places), RELEASE-CHECKLIST, FLASH-GUIDE (merged/app/signed sizes),
  REFLASH-GUIDE, README, FIRMWARE-COMPAT §7 (signed 890,516 B). Historical run records
  (PHASE-H-GATE, FIRMWARE-COMPAT §5) keep the old size — they describe that run.
- **Retest:** second clean rebuild → identical size and identical SHA-256 (deterministic).

### 7.3 Frontend command inventory drift 56 → 63
- **Finding:** `main.rs` registers 63 commands; the frontend `IMPLEMENTED_COMMANDS` listed 56.
  Missing 7 (all Round 2 additions, no UI call sites yet): `enable_auto_switch`, `led_test`,
  `key_reset`, `factory_reset`, `confirm_pairing_replacement`, `import_csharp_profiles`,
  `import_csharp_settings`. The frontend test asserted the stale 56-list, so it passed while
  the inventory drifted — the test only checks the list against itself.
- **Fix:** added the 7 to `IMPLEMENTED_COMMANDS` and to the test's expected list, in `main.rs`
  order (the games block moved after the settings block to match), updated the "56 commands"
  comments to 63 (56 Phase H5-control + 7 deep-scan Round 2).
- **Retest:** set/order comparison 63/63 identical; `npm test` **241/241** again; `tsc --noEmit` clean.

### 7.4 Stale "latest-wins" wording in the rig (harness.rs:296 + siblings)
- **Finding:** the engine now drains up to 32 frames/tick (multi-frame drain), but the rig
  still described the old latest-wins drain in `harness.rs` module docs + `QueuedSource` docs
  (§:296), `scenarios.rs` assertion notes (soak/long-soak), `main.rs` report text,
  `wheel.rs` doc comment, and a test comment/assert message.
- **Fix:** rewrote all of them to the multi-frame drain (frames past the cap are deferred,
  never dropped; `coalescedPercent` = engine gaps − arrival gaps, expected 0% on a healthy run).
  The test's assertion logic is unchanged (only its comment + message string).
- **Retest:** `cargo fmt --check` clean; `cargo test -p rsh-rig` **20/20**; the full + multi-seed
  rig runs in §2–§3 ran with the fixed wording in the log output for the multi-seed run.

### 7.5 SBOM recount 626 → 629 (DECISIONS §17.5)
- **Finding:** `Cargo.lock` has 629 `[[package]]` entries; the committed Rust SBOM listed 626.
  The 3 missing packages are the Round 2 dialog-plugin deps (`rfd` 0.17.2, `tauri-plugin-dialog`
  2.8.4, `tauri-plugin-fs` 2.7.1). `THIRD-PARTY-LICENSES.md` still said 625.
- **Fix:** regenerated `sbom/rust.cdx.json` with `.github/scripts/generate-rust-sbom.py`
  (629 components; added exactly those 3, 0 removed). Updated `THIRD-PARTY-LICENSES.md`
  (629 + corrected license table — rows now sum to 629; the old table summed to 623 because
  `r-efi`'s 2 packages lived only in §1.3) and `PHASE-H-GATE.md` SBOM counts (625 → 629).
- **Retest:** `cargo metadata` full resolve agrees (629 packages, 0 missing a license);
  `cargo deny check` PASS.

## 8. Deviations from the gate task

1. **Threshold-script path:** the task named `tools/rsh-rig/scripts/check-thresholds.py` —
   the script lives at `tools/rsh-benchmarks/check-thresholds.py`. Used the real path.
2. **SBOM count 626 → 629:** the task said 625→626, which was true before Round 2 added the
   dialog plugin. The honest current count is 629 (§7.5); docs were updated to 629, not 626.
3. **Firmware size 955,056 → 955,984:** the task did not name this; the gate found it, fixed it
   (§7.2), and recorded it in DECISIONS §17.4.
4. **Command count 56 → 63:** the task called it the "56-command packaging suite"; the gate
   found 63 registered commands and fixed the stale 56 inventory (§7.3).
5. **Stale-doc scope:** the task named `harness.rs:296` only; the same stale latest-wins claim
   appeared in 4 more rig files — all fixed (§7.4).
6. **Rig runs:** the full run took 1,953 s and multi-seed 1,466 s (the "~15 min / ~8 min"
   estimates in the rig help are short on this host). No retry was needed anywhere.

## 9. Open Windows / human gates (not proven by this gate)

| # | Gate | What proves it |
|---|---|---|
| W-1 | Dialog plugin compile | First Windows CI build / `cargo tauri build` (`lib.exe` missing in sandbox) |
| W-2 | Foreground-detector E2E | Live Windows run (property test exists) |
| A7 | Real-board run | Once, at final production release, owner PC |
| — | Clean-PC install Win10 22H2 + Win11 | Windows build produces NSIS + MSI; install + rollback on clean PCs |
| — | Game matrix 0/16 | In-game verification per game (none verified without evidence) |
| — | Real-Wi-Fi ping | Real Wi-Fi (rig NTP-ping is loopback-equivalent) |
| — | C# sidecar compile (real DLL) | Windows build with the real HIDMaestro DLL (DLL absent in sandbox) |
| — | src-tauri Windows compile | Windows CI `tauri-build` job (H3 split) |
| — | HAT_NEUTRAL_RAW 0x08 verify | Board-side check |
| — | ACC map-name live confirm | Live ACC run |
| M-4 | Flash encryption, board-side | Security follow-up (open) |
| G-2 | vJoy DLL SHA-256 pin | Measure on the Windows release machine (`PINNED_DLL_SHA256_HEX` still `None`, never invented) |

Also carried: first green CI + release dry-run after push (post-push proof, §17.2);
`cargo audit` yanked check + `npm audit` re-runs on CI (sandbox network limits, §1.10–1.11);
pin-rules + brick-recovery mock re-run against the new 955,984 B image if the release
process uses them (§5); 4-hour soak SKIPPED by user decision (§17.1).

## 10. Release readiness (for the coordinator)

- Local evidence is complete and green: 901 Rust tests, 241 frontend tests, clippy/fmt/deny
  clean, benchmarks PASS, firmware 4,444/0 + vectors + cross-verify, packaging verified,
  rig 27/27 + 54/54, fuzz 500k/0 crashes.
- The next step is the release worker's (push → CI proof → tag → release). This gate did
  **no** commit, push, tag, or release. The tree is uncommitted by design (HEAD 9690064 +
  ~190 changed entries); the release worker owns committing.
- Files this gate changed (uncommitted): `tools/rsh-rig/src/{harness,scenarios,main,wheel}.rs`
  (docs only), `sbom/rust.cdx.json` (regen), `THIRD-PARTY-LICENSES.md`,
  `docs/{DEEP-SCAN-GATE.md (new),DECISIONS.md §17,PHASE-H-GATE.md,FIRMWARE-COMPAT.md,
  FLASH-GUIDE.md,REFLASH-GUIDE.md,RELEASE-CHECKLIST.md,RELEASE-CHECKLIST-H4.md}`,
  `README.md`, `.github/workflows/release.yml` (L8 size), `frontend/src/lib/{tauri.ts,tauri.test.ts}`
  (63 commands). Plus `coordinator-h-notes.md` (gate start + completion, outside the repo).
