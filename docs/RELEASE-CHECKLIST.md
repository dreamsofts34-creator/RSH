# Release Checklist — RSH Rust v1.0.0 (FINAL, Phase H5)

**Version:** Rust app **v1.0.0** · C++ firmware **v4.0.1** · protocol **v2.1**
**Date:** 2026-11-14 · **Status: PREP — release NOT executed.**
The coordinator executes the release by pushing a `v1.*` tag after the
Phase H gate is green. This is the **final** checklist: it merges the
Phase H4 packaging evidence
([RELEASE-CHECKLIST-H4.md](RELEASE-CHECKLIST-H4.md), kept as the H4
record — **not deleted**) with the Phase H5 docs + release-prep items.
Where this file and the H4 file disagree, this file wins and the
disagreement is noted. H4 §1–§12 remain the detailed evidence; this
file is the gate list the coordinator ticks before tagging.

> Honesty rule (from H4, kept): an item is ticked only if it was
> actually done/verified. Anything needing Windows is marked
> **[Windows-only]** and unticked until it runs.

## 1. Version triple (frozen)

| Piece | Version | Source of truth |
|---|---|---|
| App | Rust **v1.0.0** | `Cargo.toml` `[workspace.package] version`, `src-tauri/tauri.conf.json` `version` — both `1.0.0`, checked by `release.yml` |
| Firmware | C++ **v4.0.1** | `FIRMWARE-COMPAT.md`; merged bin `RSH-Firmware-DevKitV1-v4.0.1.bin`, 955,984 B |
| Protocol | **v2.1** (frozen) | Wire contract; Rust app talks to the same board — **no reflash needed** for wheels already on v4.0.1 |

## 2. What H4 already proved (evidence in RELEASE-CHECKLIST-H4.md)

- [x] **Installer format:** NSIS (primary, `installMode: currentUser`)
      + MSI (secondary, same `upgradeCode`
      `9719670d-2902-517d-a1ca-712bb1bf9c3f`). **MSIX deferred with
      reason** (no Tauri MSIX target; MSIX cannot package drivers;
      elevated sidecar unproven from MSIX) — H4 §1.1.
- [x] **Firewall:** installer does nothing; the app applies the rule at
      runtime (`RSH`, inbound, program-scoped, all profiles) via the
      Windows Firewall COM API — H4 §1.2.
- [x] **Sidecar shipping:** HIDMaestro sidecar via Tauri
      `bundle.externalBin` → `rsh-hm-sidecar.exe` beside `RSH.exe`;
      NativeAOT single-file publish, no .NET runtime needed — H4 §1.3.
- [x] **Tauri bundler config** verified key-by-key against the bundled
      crate sources (NSIS/MSI targets, icons exist, upgrade code is a
      valid UUID, updater pubkey decodes to 42 bytes starting `Ed`) —
      H4 §2.
- [x] **SBOMs + licenses:** `sbom/rust.cdx.json` (CycloneDX 1.6, **626
      packages** — full `Cargo.lock` resolve; deep-scan Round 2 recount:
      the lockfile has 626, the old SBOM listed 625 because `rsh-rig`
      was missing), `sbom/frontend.cdx.json` (CycloneDX 1.5, **320
      components** — 318 unique name@version; `magic-string@0.30.21`
      appears 3× at different install paths, which is legitimate),
      `sbom/sidecar.cdx.json` (hand-written, pins re-verified);
      `cargo deny check licenses` PASS; **no GPL/AGPL/LGPL
      anywhere** — H4 §3 + `THIRD-PARTY-LICENSES.md`.
- [x] **Updater keypair (NEW, separate from the firmware keypair):**
      minisign public key
      `RWRU7xMj5S9Eodnz8HNKEz8LacETwEFxkttjZLnCbopqmAdFYT2S7DGb`
      (keynum `54ef1323e52f44a1`, fingerprint
      `ce5f3210927e7ada3efc2ce843c2ac1dc81762fd2d09e4f46f8ccabf94f260bd`),
      in `tauri.conf.json`; crate-level sign/verify cross-check PASS —
      H4 §4. **Updater is check-only in v1.0.0** (no
      download/install code path); `active: false` in the config is a
      Tauri-v1 leftover the plugin ignores — the real gates are "no
      `latest.json` published yet" + "check-only app code".
- [x] **Firmware signing CI flow verified from source:** `pio run` +
      `sign_firmware.py` → `firmware-signed.bin` = `firmware.bin` + 68 B
      trailer; key unset → signing skipped and stale signed bin deleted;
      key set but bad → build fails hard; existing keypair reused
      (public-key fingerprint
      `387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411`) —
      H4 §5.
- [x] **App is UNSIGNED (OV cert deferred, D-R11)** — SmartScreen
      warning expected. Firmware signing is unaffected — H4 §5.
- [x] **Release workflow created** (`.github/workflows/release.yml`),
      YAML-valid — H4 §6, re-verified read-only in H5 (§6 below).

## 3. Release asset list (exact — 11 assets)

What `release.yml` stages and publishes (H4 §7):

| # | Asset | Source |
|---|---|---|
| 1 | `RSH_1.0.0_x64-setup.exe` | Tauri NSIS bundle (primary installer) |
| 2 | `RSH_1.0.0_x64-setup.exe.sig` | Updater signature for the NSIS installer |
| 3 | `RSH_1.0.0_x64_en-US.msi` | Tauri WiX bundle (secondary installer) |
| 4 | `RSH_1.0.0_x64_en-US.msi.sig` | Updater signature for the MSI |
| 5 | `latest.json` | Generated in `release.yml` from the real `.sig` + real release URLs |
| 6 | `RSH-Firmware-DevKitV1-v4.0.1.bin` | `pio run` merged image, 955,984 B |
| 7 | `firmware-signed.bin` | `firmware.bin` + 68 B trailer, Ed25519-signed |
| 8 | `SHA256SUMS` | SHA-256 of every asset above, re-verified in-workflow |
| 9 | `sbom-rust.cdx.json` | `cargo cyclonedx` (release-time) |
| 10 | `sbom-frontend.cdx.json` | `npm sbom` (release-time) |
| 11 | `sbom-sidecar.cdx.json` | Committed `sbom/sidecar.cdx.json` (hand-written) |

Installer filenames follow Tauri 2.12 defaults — the first dry-run
confirms them; if they differ, the workflow's globbing fails loudly
rather than publishing the wrong file.

## 4. Open release gates (all carried honestly — none is silently passed)

| # | Gate | Status | Owner / next step |
|---|---|---|---|
| G-1 | **pid.codes PR** for VID `0x1209` / PID `0x5253` | **OPEN — release gate** | Must merge before v1.0.0; human gate, not in the workflow (Phase G D-G3) |
| G-2 | **vJoy DLL SHA-256** (`PINNED_DLL_SHA256_HEX`) | **OPEN — not measured** | Measure on the Windows release machine; never invented (Phase G G-G3). `release.yml` only records whether the runner happens to have vJoy (it will not) |
| G-3 | **`firmware/` restore in the repo** | **CLOSED 2026-11-14** — restored from `reference/firmware-v4.0.1/` (flat copy, 44 files + `rsh/signing_pubkey.h`); all 4 `release.yml` fail-closed paths present; local `pio run` SUCCESS 32.96 s, merged bin 955,984 B, signed bin 889,588 B, Ed25519 verify OK |
| G-4 | **Secret `RSH_UPDATER_PRIVATE_KEY`** | **CLOSED 2026-11-14** — created via libsodium sealed-box from `signing-keys/updater-minisign.key` (whole `.key` file, 259 B); verified listed alongside `RSH_SIGNING_KEY_PEM` (value never printed/logged) |
| G-5 | **Secret `RSH_SIGNING_KEY_PEM`** | Pre-existing (C# series) — **verify present** | Verify before the first release dry-run |
| G-6 | **First `release.yml` dry-run** (`workflow_dispatch`, `dry_run: true`) | **OPEN — never run** | Must pass end-to-end before any tag is pushed. It is also the first proof of: `cargo cyclonedx` flags, `cargo tauri build` output filenames, sidecar staging path, `cargo tauri signer sign` E2E of the new updater keypair |
| G-7 | **A7 real-board run** | **OPEN — carried Phase H gate item** | Run once, at final production release, on the owner PC (user decision: real-board tests only at final release) |
| G-8 | **4-hour soak** | **SKIPPED by user decision 2026-11-14 14:54 (time constraint); D-R14 release blocker overridden by the user. Recorded as SKIPPED, NEVER as passed.** 60 s quick soak + 10 min full soak still run (results in `docs/PERF-H2.md`, H2) | User-accepted limitation; carried in the release notes verbatim |
| G-9 | **Game matrix in-game verification** | **OPEN — 0 of 16 backend–game pairs verified** | Windows-only; procedure in [GAME-MATRIX.md](GAME-MATRIX.md). v1.0.0 ships with the matrix honestly at 0/16 — no game is claimed to work until verified |
| G-10 | **H1 security re-scan conditions** | **CONDITIONAL PASS** (0 Critical / 0 High) — see [SECURITY-RESCAN-H1.md](SECURITY-RESCAN-H1.md) §7 | Session-owner activator must land before v1.0.0 ships (SEC-E-14); 3 sandbox-timing test failures must re-run green on CI/Windows; `cargo audit` yanked check + `npm audit` re-run on CI (both blocked/timed out in sandbox) |
| G-11 | **Clean-PC install + rollback plans** | **OPEN — Windows-only** | H4 §8/§9, on Win10 22H2 **and** Win11: NSIS + MSI install, paint check (no blank window), firewall rule, HIDMaestro UAC path, vJoy fallback journey, upgrade-over-install, downgrade rollback (`allowDowngrades: true`), profile restore |
| G-12 | **CI `tauri-build` job broken** | **CLOSED 2026-11-14 — was already fixed in H5** (this checklist entry was stale): `ci.yml` `tauri-build` installs the CLI with `cargo install tauri-cli --locked` and runs `cargo tauri build --no-bundle` from `src-tauri/` — see the H4-finding-#1 comment in `ci.yml`. `release.yml` is unaffected |
| G-13 | **MSIX** | Deferred with reason (H4 §1.1) | Future Windows spike only; not a v1.0.0 gate |
| G-14 | **OV code-signing certificate** | Deferred (user decision D-R11) | Buy later ($150–300/year est.); app ships unsigned, SmartScreen expected |

## 5. Docs delivered by H5 (this phase)

- [x] Professional README (repo root) rewritten for the Rust
      recreation — badges, features, quickstart, architecture summary,
      repo tree, dev commands, contributing, unsigned-app note,
      License: MIT. All relative links verified to resolve (H5
      verification, §7).
- [x] Docs pass — `docs/` reviewed as it stands: all files are
      Rust-series docs; C# mentions are historical context only, never
      current behaviour. New [docs/README.md](README.md) indexes the
      current docs accurately.
- [x] [USER-GUIDE.md](USER-GUIDE.md) — install, first-run pairing
      (60 s window, re-pairing mandatory — C# DPAPI keys cannot be
      read from Rust), per-module tuning, profiles, game launcher,
      output backend choice (HIDMaestro default + UAC note, vJoy
      fallback), troubleshooting (blank-window lesson, page-load
      failure, log location, diagnostics export).
- [x] [FLASH-GUIDE.md](FLASH-GUIDE.md) — firmware v4.0.1 reflash
      procedure (new board / brick recovery), signed-only,
      anti-downgrade, BOOT-hold safe mode; detailed companion
      [REFLASH-GUIDE.md](REFLASH-GUIDE.md) kept.
- [x] [GAME-MATRIX.md](GAME-MATRIX.md) — Phase H final note added;
      **0/16 verified, unchanged** — no pair marked verified without
      evidence.
- [x] [SUPPORT-PERIOD.md](SUPPORT-PERIOD.md) — support window for
      v1.0.0 (12 months after release, or 90 days after v1.1.0,
      whichever is later).
- [x] This file — final checklist merging H4 (H4 file kept, referenced).

## 6. Release workflow verification (H5, read-only)

Read `.github/workflows/release.yml` (361 lines) and `.github/workflows/ci.yml`
(513 lines) on 2026-11-14:

- [x] **YAML parses** — both workflows (`yaml.safe_load` OK).
- [x] **Triggers:** push of tag `v1.*` + `workflow_dispatch` with
      `dry_run` (default `"true"`) and optional `tag` input. Tag
      validated `^v1\.\d+\.\d+$`. Nothing runs on a normal push to
      main (CI covers that).
- [x] **Secret names:** `RSH_SIGNING_KEY_PEM` (firmware Ed25519 PEM)
      and `RSH_UPDATER_PRIVATE_KEY` (minisign private key) — both
      checked present (fail closed) before any build step; values
      never printed; temp firmware-key file deleted in an
      `if: always()` step.
- [x] **11-asset list** (§3) matches the staging + verify steps in the
      workflow: NSIS + MSI + both `.sig` files + `latest.json` +
      merged firmware bin + `firmware-signed.bin` + `SHA256SUMS` +
      3 SBOMs. The verify step re-checks every staged asset,
      re-verifies `SHA256SUMS`, and re-verifies the signed firmware.
- [x] **Fail-closed steps:** version consistency (tag == Cargo
      workspace version == `tauri.conf.json` version,
      `createUpdaterArtifacts` true) → firmware presence → secrets
      presence → gates (`cargo test --workspace`, `cargo deny check`,
      frontend tests + build) → firmware build + sign + verify
      (`--verify` + image-region byte-compare + size `firmware.bin
      + 68`) → sidecar fetch with ZIP + DLL SHA-256 pins →
      `cargo tauri build` (updater key via `TAURI_SIGNING_PRIVATE_KEY`
      env on that step only) → stage → verify → `gh release create`
      (skipped on dry-run).
- [x] **Defects found:** none functional. One trivial doc-level error
      in a workflow comment was fixed by H5: the header's asset-list
      pointer said "see docs/RELEASE-CHECKLIST-H4.md §5" — the asset
      list is H4 **§7** (§5 is firmware signing CI). No workflow logic
      was touched.
- [ ] **Not verifiable from the sandbox:** the workflow has never run.
      First dry-run (G-6) is the proof of the installer filenames,
      `cargo cyclonedx` flags, sidecar staging path, and updater
      signing E2E.

## 7. H5 verification log (what was actually checked on disk)

- All relative links in the rewritten README, `docs/README.md`,
  `USER-GUIDE.md`, `FLASH-GUIDE.md`, `SUPPORT-PERIOD.md` and this
  file were checked against the repo tree — every target exists
  (script run 2026-11-14; see the H5 final report for the count).
- `release.yml` + `ci.yml` YAML-parse verified (above).
- `Cargo.toml` workspace version `1.0.0` and `tauri.conf.json`
  version `1.0.0` confirmed on disk (release.yml's consistency check
  has matching values to compare).
- `docs/` inventory: 16 pre-H5 docs + H4/H1 gate docs, all Rust-era;
  no stale C# doc presented as current. `docs/README.md` did not
  exist — created by H5.
- Numbers cited in H5 docs come only from files read on disk in this
  session (H4 evidence, Phase G gate doc, `tauri.conf.json`,
  `release.yml`, `KEY-LIFECYCLE-RUST.md`, `REFLASH-GUIDE.md`,
  reference `BRICK-RECOVERY.md` / `SIGNED-OTA.md`). No test result
  was invented; Phase H final numbers belong to the coordinator's
  gate doc, not to this checklist.

## 8. Pre-tag human gates (coordinator checks before pushing `v1.*`)

- [ ] Phase H gate green (H1–H5, final testing, A7 real-board run).
- [ ] G-1 pid.codes PR merged.
- [ ] G-2 vJoy DLL SHA-256 measured on the Windows release machine.
- [x] G-3 `firmware/` restored in the repo (2026-11-14; local pio build + signed verify OK).
- [x] G-4 + G-5 secrets set (G-5 `RSH_SIGNING_KEY_PEM` verified present 2026-11-14; G-4 created 2026-11-14, sealed-box, verified listed).
- [ ] G-6 `release.yml` dry-run (`workflow_dispatch`, `dry_run: true`)
      green, including the `cargo cyclonedx` step and the installer
      filename assumptions (§3).
- [ ] G-11 clean-PC install + rollback plans (§8/§9 of H4) executed on
      Win10 + Win11, report filed.
- [x] G-12 `ci.yml` `tauri-build` job fixed in H5 (cargo-installed Tauri CLI; checklist entry was stale).
- [ ] Release notes written: unsigned-app SmartScreen note; firmware
      anti-downgrade note; HIDMaestro UAC note; **re-pairing mandatory**
      (C# → Rust) on the first page; game matrix status (0/16);
      **4-hour soak SKIPPED** (user decision, not passed).
