# Release Checklist — H4: Packaging + installer + signing PREP

**Version:** RSH Rust v1.0.0 (app) · C++ firmware v4.0.1 · protocol v2.1
**Date:** 2026-11-14 · **Status: PREP COMPLETE — release NOT executed.**
The parent (coordinator) executes the release by pushing a `v1.*` tag after the
Phase H gate is green. This file is the H4 evidence + the Windows-only test plans.
Every item is ticked, explicitly deferred with a reason, or marked Windows-only.

> Honesty rule: an item is ticked only if it was actually done/verified in this
> sandbox. Anything that needs Windows is marked **[Windows-only]** and unticked.

## 1. Decisions (S-14 + SW-R8.4)

### 1.1 Installer format — NSIS ships, MSI ships, MSIX deferred

- [x] **DECISION: NSIS (primary) + MSI (secondary). MSIX DEFERRED.**
- [x] Evidence for the decision (spike, as far as the Linux sandbox allows):
  1. **Tauri bundler has no MSIX target.** `BundleTarget` in `tauri-utils` 2.10.2
     (the exact version Tauri 2.12.5 bundles) is `deb | rpm | appimage | nsis | msi |
     app | dmg` — no `msix`. Shipping MSIX would need a separate packaging
     pipeline (MSIX Packaging Tool / `makeappx` from the built exe), not the
     Tauri bundler. SW-R8.4 already recorded this as a deliberate change from
     D22 ("Tauri bundler MSI/NSIS deta hai, MSIX nahi").
  2. **MSIX cannot package drivers.** Microsoft's own forum answer: "We don't
     support packaging drivers in the MSIX Packaging Tool" — RSH's whole output
     path is a driver (HIDMaestro UMDF2) installed by an admin step.
  3. **The elevated sidecar is unproven from inside MSIX (R-4).** The sidecar
     (`rsh-hm-sidecar.exe`, `requireAdministrator` manifest) must spawn elevated
     on every HIDMaestro session. From an MSIX package that needs the restricted
     `allowElevation` capability — which (a) only *allows* elevation, the exe
     manifest must still request it, (b) is "very unlikely to be approved" for
     Store submission, and (c) MSIX conversion scanners flag elevated shortcut
     exes as not recommended. No Windows machine was available in this sandbox
     to prove the spawn → UAC → driver-install chain from a packaged app, so
     MSIX cannot be proven here.
  4. **NSIS/MSI need no such proof.** Both install a plain Win32 app; the
     already-designed runtime elevation (sidecar UAC prompt per HIDMaestro
     session, firewall rule via COM API) works identically installed or not.
- [x] MSIX deferred with reason (not dropped silently): revisit only if a
  Windows test proves, on Win10 + Win11, (a) packaged app spawns the elevated
  sidecar, (b) `HMContext.InstallDriver()` succeeds from the packaged context,
  (c) the firewall rule can be created, (d) per-machine install works without
  the Store. Until then MSIX stays out — a second installer format that cannot
  install the driver would be worse than none.
- [x] **NSIS `installMode: currentUser`** — no install-time elevation; the app
  installs to the user profile, Start Menu shortcut, HKCU metadata. Driver /
  firewall elevation happens at runtime, where the app already handles it
  (sidecar UAC prompt, COM firewall API with loud failure). `perMachine` was
  rejected: it forces install-time admin for an app whose admin steps are all
  runtime steps. `both` was rejected: it requires admin even for a
  current-user choice (tauri-utils doc).
- [x] **MSI (WiX) ships as the secondary installer** — same app, same
  `upgradeCode` (below), for managed/enterprise installs. No MSI-only options
  were set; scope/elevation behaviour is WiX-default and must be recorded by
  the Windows test (§8).
- [x] **Rollback (R-M9) = `bundle.windows.allowDowngrades: true` (explicitly
  set, default is also true) + previous installers stay on GitHub Releases +
  profile 3-gen backup.** Tauri blocks downgrades when this is false (MSI
  refuses; NSIS refuses in silent/passive/update mode and forces uninstall
  first in interactive mode). With it true, an older installer installs over
  a newer one — that is the rollback mechanism. There is no in-installer
  "keep previous version" copy: the previous version is the previous GitHub
  Release asset. Firmware downgrade is NOT part of rollback — the board
  rejects it by anti-downgrade (see §9).

### 1.2 Firewall — installer does NOTHING; the app does it at runtime

- [x] **Verified (not invented):** the rule spec is `FirewallRuleSpec::inbound_allow`
  (`crates/rsh-app-core/src/system.rs`, `FIREWALL_RULE_NAME = "RSH"`):
  **inbound allow, program-scoped (no port named), all profiles, protocol ANY,
  outbound untouched.** Applied by `ensure_firewall_rule` (`src-tauri/src/system.rs`)
  through the Windows Firewall COM API (`INetFwPolicy2` / `INetFwRule`, `windows`
  0.62.2 — verified against the crate source in `docs/SYSTEM-INTEGRATION.md` §4).
  Idempotent (existing `RSH` rule is re-pointed at the current exe).
- [x] **No NSIS installer hook was added, deliberately.** An installer-side
  firewall step would duplicate the verified runtime path and would need
  install-time elevation (contradicting `installMode: currentUser`). The
  runtime path needs elevation and fails loudly when non-elevated; the app
  keeps working without the rule and Settings tells the user to allow it.
- [ ] **[Windows-only]** COM call sequence actually run (init → create → set →
  add) + Settings page shows present/enabled/exe path. Owner-PC test (§8).

### 1.3 Sidecar shipping — Tauri `externalBin` (verified mechanism)

- [x] **Sidecar ships beside `RSH.exe` via `bundle.externalBin`.**
  Mechanism verified in `tauri-bundler` 2.10.4 + `tauri-utils` 2.10.2 source:
  `externalBin: ["binaries/rsh-hm-sidecar"]` makes the bundler look for
  `binaries/rsh-hm-sidecar-x86_64-pc-windows-msvc.exe` and copy it beside the
  main exe with the triple stripped → `rsh-hm-sidecar.exe`. That is exactly
  where `resolve_sidecar_path()` looks (`crates/rsh-output-hidmaestro/src/manager.rs`:
  `$RSH_HM_SIDECAR` → exe's own directory → cwd).
- [x] **No DLL ships beside the sidecar in the AOT form.** The shipping publish
  is NativeAOT single-file (`dotnet publish -p:PublishAot=true` → "a single
  `rsh-hm-sidecar.exe`, no .NET runtime needed"); the SDK DLL is linked in.
  The sidecar's `CheckDllHash` fallback (DLL beside the exe) only matters for
  the non-AOT fallback publish — if AOT ever breaks, the release must stage
  `HIDMaestro.Core.dll` beside the exe and re-verify its hash.
- [x] `release.yml` stages the sidecar (fetch pinned ZIP → verify hashes →
  AOT publish → copy to `src-tauri/binaries/rsh-hm-sidecar-x86_64-pc-windows-msvc.exe`)
  before `cargo tauri build`.
- [ ] **[Windows-only]** Installed app actually spawns the sidecar
  (UAC prompt, hello, install-driver, create, submit) — H-HID-1…H-HID-6.

## 2. Tauri bundler config — `src-tauri/tauri.conf.json`

- [x] `bundle.targets: ["nsis", "msi"]`; `active: true`; `category: "Utility"`;
  `publisher: "dreamsofts34-creator"`; `homepage: https://github.com/dreamsofts34-creator/RSH`;
  `license: "MIT"`; `licenseFile: "../LICENSE"` (repo-root LICENSE — bundler
  canonicalizes the path; Windows build is the proof, §8);
  `copyright: "Copyright © 2026 RSH contributors. MIT licensed."`
- [x] `bundle.createUpdaterArtifacts: true` — build produces `.sig` + `latest.json`
  (the workflow regenerates `latest.json` with the real release URLs; see §6).
- [x] `bundle.windows.webviewInstallMode: {"type": "downloadBootstrapper", "silent": true}`
  (the Tauri default, set explicitly — small installer; needs internet at install).
- [x] `bundle.windows.wix.upgradeCode: "9719670d-2902-517d-a1ca-712bb1bf9c3f"`
  — UUIDv5 (DNS namespace, `"RSH.exe.app.x64"`), i.e. exactly the value Tauri
  derives by default, now pinned so a future `productName` change can never
  silently change it (a changed upgrade code = Windows treats the update as a
  different app = duplicate installs).
- [x] `bundle.windows.nsis`: `installMode: currentUser`,
  `displayLanguageSelector: false`, `languages: ["English"]`,
  `installerIcon` / `uninstallerIcon: icons/icon.ico`,
  `headerImage: icons/header.bmp` (150×57), `sidebarImage: icons/sidebar.bmp` (164×314).
- [x] `bundle.icon`: `icons/32x32.png`, `icons/128x128.png`,
  `icons/128x128@2x.png`, `icons/icon.png`, `icons/icon.ico` — all exist.
- [x] **Icons are H4-generated interim assets** (PIL, violet `#6C4CF1` rounded
  square + white `R`, header/sidebar bitmaps in the same brand). **No icon
  existed anywhere in the repo** (no `src-tauri/icons/`, no `frontend/public/`) —
  without them the bundler has nothing to embed. They are a clean placeholder
  brand mark, not a final logo: if the UI plan ever ships a real logo, replace
  the files in `src-tauri/icons/` and rebuild. No file associations
  (`bundle.fileAssociations` omitted — profiles open via dialog, nothing
  registers a file type).
- [x] Every config key was checked against the bundled-crate source
  (`tauri-utils` 2.10.2 `BundleConfig`/`WindowsConfig`/`WixConfig`/`NsisConfig`,
  `tauri-plugin-updater` 2.14.1 `Config`): all keys exist, enum values are the
  right case (`downloadBootstrapper`, `currentUser`, `passive`), JSON parses,
  upgrade code is a valid UUID, pubkey decodes to 42 bytes starting `Ed`.
- [ ] **[Windows-only]** `cargo tauri build` on windows-latest actually
  produces the NSIS + MSI installers (config proof). Linux cannot run it
  (no WebView2/webkit here — not a code error).

## 3. SBOM + licenses

- [x] `sbom/rust.cdx.json` — CycloneDX 1.6, **626 packages** (full `Cargo.lock`
  resolve via `cargo metadata`). Generated by an H4 script because
  `cargo-cyclonedx` was still installing when the SBOM was needed; the
  release workflow regenerates it with
  `.github/scripts/generate-rust-sbom.py` — NOT `cargo cyclonedx`
  (deep-scan Round 2, M1: proven locally, cyclonedx writes per-crate
  files, defaults to spec 1.3, and its union tops out at 589 packages
  because dev-deps are excluded — it can never reproduce this file).
  Recount (L7): `Cargo.lock` has 626 `[[package]]` entries; the old
  SBOM listed 625 because `rsh-rig` (the test rig, not shipped) was
  missing. Regenerated 2026-11-14 with the checked-in script: 626
  components, 0 changed vs the old file except the added `rsh-rig`.
  (`THIRD-PARTY-LICENSES.md` still says 625 — it is outside the
  deep-scan Round 2 file ownership; the coordinator must update it
  to 626 in the same change.)
- [x] `sbom/frontend.cdx.json` — CycloneDX 1.5, **320 components**,
  `npm sbom --sbom-format cyclonedx` (npm 10.9.4). Split is deliberate
  (M1): npm 10.9.4 caps at CycloneDX 1.5 — `--sbom-version 1.6`
  silently still emits 1.5 (proven locally 2026-11-14); 1.6 needs
  npm 11, which is not pinned. The release workflow requests 1.5
  explicitly so it never silently claims 1.6. Fresh `npm sbom` output
  matches the committed file exactly (320 entries, same set).
- [x] `sbom/sidecar.cdx.json` — CycloneDX 1.6, hand-written (the sidecar has
  no lockfile / no NuGet packages): sidecar exe + `HIDMaestro.Core.dll` v2.1.0
  + UMDF2 driver v2.1.0, both MIT, both SHA-256 pinned.
- [x] `THIRD-PARTY-LICENSES.md` — policy + full inventory + the four
  special cases, each named:
  - `cargo deny check licenses`: **PASS** (only 2 warnings — allow-list
    entries `BSD-2-Clause` / `Unicode-DFS-2016` currently unused).
  - MPL-2.0 (9 crates, incl. direct dep `serialport` 4.10.1 + Tauri CSS chain)
    — file-level weak copyleft, documented exception in `deny.toml`, no effect
    on RSH's MIT code. In the Windows release tree (299 packages).
  - `r-efi` (MIT-option of `MIT OR Apache-2.0 OR LGPL-2.1-or-later`) and
    `unescaper` (MIT-option of `MIT OR GPL-3.0-only`) — permissive option taken.
  - `webpki-root-certs` 1.0.12 (`CDLA-Permissive-2.0`) — lockfile-only, not in
    the Windows app tree, permissive data license.
  - Frontend extras not in the Rust allow-list but permissive: BlueOak-1.0.0 (5),
    0BSD (2), CC-BY-4.0 (1, `caniuse-lite` data). **No GPL/AGPL/LGPL anywhere
    (Rust, npm, or sidecar).**
- [x] Not bundled, therefore not in any SBOM: vJoy driver (D18 — user installs
  it), WebView2 (ships with Windows), NSIS/WiX + .NET SDK (build-time only),
  firmware C++ libraries (unchanged v4.0.1 tree — out of H4 scope).
- [ ] **[Windows-only / release-time]** Regenerated SBOMs in `release.yml`
  diffed against this snapshot (Rust 626 / frontend 320); any delta explained.
  The release step also verifies the sidecar SBOM against the measured
  staging hashes (M4) — a drifted hash fails the run.

## 4. Updater + minisign keypair

- [x] **NEW keypair generated 2026-11-14, SEPARATE from the firmware Ed25519
  keypair (never reused, D-R10).**
  - Private: `~/workspace/projects/rsh/signing-keys/updater-minisign.key` (0600)
  - Public: `~/workspace/projects/rsh/signing-keys/updater-minisign.pub` (0644)
  - Public key (also `tauri.conf.json → plugins.updater.pubkey`):
    `RWRU7xMj5S9Eodnz8HNKEz8LacETwEFxkttjZLnCbopqmAdFYT2S7DGb`
  - Keynum (hex): `54ef1323e52f44a1`
  - Fingerprint (SHA-256 of raw 32-byte pubkey):
    `ce5f3210927e7ada3efc2ce843c2ac1dc81762fd2d09e4f46f8ccabf94f260bd`
  - Public key (hex): `d9f3f0734a133f0b69c113c0417192db6364b9c26e8a6a980745613d92ec319b`
  - No password. Recorded in `signing-keys/README.md` (public key + fingerprint
    + purpose only — **no key material anywhere in the repo, chat, or docs**).
  - Self-check PASS: minisign format (74 B secret / 42 B public, `Ed` prefix,
    keynum + pubkey consistent between the two files), seed derives the pubkey,
    Ed25519 sign/verify round-trip OK (openssl).
- [x] **Updater configured but NOT live.** `tauri.conf.json`:
  `plugins.updater.pubkey` = the key above; `endpoints` =
  `https://github.com/dreamsofts34-creator/RSH/releases/latest/download/latest.json`;
  `active: false`, `dialog: false`; `windows.installMode: passive`.
- [x] **Honesty note — `active: false` does NOT disable anything in Tauri 2.**
  Verified in `tauri-plugin-updater` 2.14.1 source: its `Config` has no `active`
  or `dialog` field (they are Tauri v1 leftovers, silently ignored). The real
  gates that keep updates off before v1.0.0 ships are:
  1. **No `latest.json` is published** until the v1.0.0 GitHub Release creates it
     (the updater checks that endpoint; a 404 = "could not check", never an update).
  2. **The app is check-only.** `check_for_update` (`src-tauri/src/main.rs`) calls
     `.check()` and never `download_and_install()` — no code path in the app can
     install an update today. Download/install wiring is a post-v1.0.0 step, not
     an H4 silent change.
  3. With the real pubkey now configured, a forged `latest.json` cannot install
     anything even after go-live — the signature must verify.
  The `active: false` keys are kept as intent markers; this note is the truth.
- [x] Updater E2E is Windows-only — test plan in §8.9.
- [x] **Crate-level cross-check PASS (H4):** a test program using the exact
  libraries Tauri uses (`minisign` 0.9.1 — the signer — and
  `minisign-verify` 0.3.0 — the updater plugin's verifier) parses the secret
  key (`from_unencrypted_box`), confirms the public-key file's base64 equals
  the `tauri.conf.json` pubkey, signs a probe and verifies it (prehashed
  format), and parses the config pubkey with the verifier. (A first
  hand-rolled draft in the wrong 74-byte secret format was rejected by the
  crate with `UnexpectedEof` and wiped with `shred -u` — it was never used
  to sign anything. That rejection is exactly why the format check mattered.)
- [ ] **[Windows-only]** `cargo tauri signer sign` end-to-end check on the
  first Windows dry-run (sign a probe file with the key, verify the `.sig`).
  Fail-closed: a wrong key fails the release build loudly at the signing step.
- [ ] **GitHub secret `RSH_UPDATER_PRIVATE_KEY` must be created** (whole
  `.key` file content, 2 lines) before the first release. Owner/coordinator
  action — H4 cannot create repo secrets from the sandbox.

## 5. Firmware signing CI — verified (read-only; no secret touched, no key printed)

- [x] **Flow verified from source** (`reference/firmware-v4.0.1/sign_firmware.py`,
  `sign-image.py`, `docs/FIRMWARE-COMPAT.md` §7, `docs/REFLASH-GUIDE.md`):
  - `pio run` → `sign_firmware.py` post-action: `RSH_SIGNING_KEY=<PEM path>`
    set → `sign-image.py --sign` → `firmware-signed.bin` = `firmware.bin` +
    **68 B trailer** (64 B Ed25519 sig over SHA-256 of the image + 4 B magic).
    Size must be exactly `firmware.bin + 68` or the build fails.
  - Key unset → signing skipped (dev build unsigned) **and any stale
    `firmware-signed.bin` is deleted** (BUG-W1-1) — a keyless build can never
    ship an old signed image.
  - Key set but file missing / sign fails → **build fails hard**.
  - Existing keypair reused (NOT new): private key in `signing-keys/`
    (0600), CI secret `RSH_SIGNING_KEY_PEM` (whole PEM); public-key
    fingerprint `387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411`.
  - Board + app reject unsigned images (SEC-C-06, verify-before-apply +
    anti-downgrade). Merged factory image `RSH-Firmware-DevKitV1-v4.0.1.bin`,
    **955,984 B**.
- [x] **`release.yml` adds the fail-closed verify the post-action cannot do:**
  `sign-image.py --verify <key> firmware-signed.bin` (Ed25519 verify, non-zero
  exit fails) **plus** image-region byte-compare against the fresh
  `firmware.bin` (guards a stale signed bin surviving a no-op `pio run`,
  where the SCons post-action does not run).
- [x] **Gap found and carried:** `firmware/` does **not** exist in the repo
  (only `reference/firmware-v4.0.1/` outside it). `release.yml` fails closed
  with restore instructions if `firmware/platformio.ini`,
  `firmware/sign_firmware.py`, `firmware/sign-image.py`, or
  `firmware/rsh/signing_pubkey.h` is missing. **The coordinator must restore
  `firmware/` from the reference tree before tagging** — a firmware-less
  release is never built.
- [x] **App is UNSIGNED (OV cert deferred, D-R11)** — SmartScreen warning
  expected. Firmware signing is unaffected. No `certificateThumbprint` /
  `digestAlgorithm` in the bundler config, deliberately.

## 6. Release workflow — `.github/workflows/release.yml` (NEW, prep only)

- [x] Created, YAML-valid. Triggers: push of tag `v1.*` (+ `workflow_dispatch`
  with `dry_run` default `true` and optional `tag` input).
- [x] Single `windows-latest` job: version-consistency check (tag == Cargo
  workspace version == `tauri.conf.json` version, `createUpdaterArtifacts` true)
  → firmware presence + secrets presence (fail closed) → gates
  (`cargo test --workspace`, `cargo deny check`, frontend tests + build) →
  firmware build + sign + verify → sidecar fetch/verify/AOT/stage → SBOM
  regen → `cargo tauri build` (updater key via `TAURI_SIGNING_PRIVATE_KEY`
  env on that step only) → stage assets → `latest.json` generated from the
  real `.sig` + real release URLs → `SHA256SUMS` → verify pass → `gh release
  create` (skipped on dry-run) → temp signing-key file deleted in an
  `if: always()` step.
- [x] The workflow never prints a secret; the temp key file lives in
  `$RUNNER_TEMP` and is deleted even on failure.
- [ ] **[Windows-only]** First dry-run (`workflow_dispatch`, `dry_run: true`)
  must pass end-to-end before any tag is pushed. That run is also the first
  proof of: the SBOM generator script (`.github/scripts/generate-rust-sbom.py`),
  `cargo tauri build` output filenames, and the sidecar staging path.
  (Deep-scan Round 2: the old proof item "`cargo cyclonedx` flags" is gone —
  cyclonedx is no longer used by the workflow, see §3 / DECISIONS §16.)
- [x] **Coordinator finding from H4 — RESOLVED (H5 / 9690064, not open
  anymore):** the CI `tauri-build` job used to run `npm run tauri build`
  in `frontend/` with no `tauri` script; it now installs the CLI with
  `cargo install tauri-cli --version 2.12.4 --locked` and runs
  `cargo tauri build --no-bundle` from `src-tauri/`, same as `release.yml`.
  (Deep-scan Round 2: the old "Finding for the coordinator" text was
  stale and is corrected here.)

## 7. Release asset list (exact — what `release.yml` stages and publishes)

| Asset | Source | Notes |
|---|---|---|
| `RSH_1.0.0_x64-setup.exe` | Tauri NSIS bundle | Primary installer |
| `RSH_1.0.0_x64-setup.exe.sig` | Tauri updater artifacts | Minisign signature (updater) |
| `RSH_1.0.0_x64_en-US.msi` | Tauri WiX bundle | Secondary installer |
| `RSH_1.0.0_x64_en-US.msi.sig` | Tauri updater artifacts | Minisign signature (updater) |
| `latest.json` | Generated in `release.yml` | Updater manifest; `windows-x86_64.url` = release download URL of the `-setup.exe`; `signature` = contents of its `.sig` |
| `RSH-Firmware-DevKitV1-v4.0.1.bin` | `pio run` merged image | 955,984 B factory image — `release.yml` FAILS if the size differs (L8); on a firmware version bump update the expected size in the workflow AND this row in the same change |
| `firmware-signed.bin` | `sign_firmware.py` | `firmware.bin` + 68 B trailer, Ed25519-signed |
| `SHA256SUMS` | Generated in `release.yml` | SHA-256 of every asset above, two-column format, re-verified in-workflow |
| `sbom-rust.cdx.json` | `.github/scripts/generate-rust-sbom.py` (release-time) | Full Cargo.lock resolve, CycloneDX 1.6, 626 packages (NOT cargo cyclonedx — see §3) |
| `sbom-frontend.cdx.json` | `npm sbom` (release-time) | CycloneDX 1.5 (npm 10.9.4 cap — see §3) |
| `sbom-sidecar.cdx.json` | Committed `sbom/sidecar.cdx.json` | Hand-written; hashes verified against the measured staging hashes in-workflow (M4) |

Filenames for the installers follow Tauri 2.12 defaults (`<productName>_<version>_x64-setup.exe`,
`<productName>_<version>_x64_en-US.msi`) — the first dry-run confirms them; if they
differ, `release.yml` globbing fails loudly rather than publishing the wrong file.

## 8. Clean-PC install test plan (Windows-only, owner PC)

Run on **both** Windows 10 22H2 (build 19045 — the owner's PC) and Windows 11.
Use a clean VM or a fresh user profile where possible; record machine + build +
installer hashes in the test report. Each step: PASS/FAIL + evidence (screenshot/log).

### 8.1 NSIS installer (primary)

1. [ ] Download `RSH_1.0.0_x64-setup.exe` from the GitHub Release; verify SHA-256
   against `SHA256SUMS` before running.
2. [ ] Double-click → SmartScreen warning appears (expected — app unsigned, D-R11);
   "Run anyway" continues.
3. [ ] Installer shows the RSH header/sidebar branding, English only, no language
   selector; license page shows the MIT license text.
4. [ ] Install completes with **no admin prompt** (`installMode: currentUser`).
   Install location is the user profile; Start Menu shortcut exists; entry in
   "Apps & features" (uninstall) exists.
5. [ ] Launch from the shortcut: main window paints (title `RSH — Real Sim Hub v1.0.0`,
   sidebar + navigation, no blank window — the C# lesson); Dashboard/Tuning/Setup/
   Buttons/Output/Profiles/Monitor/Settings all render with text (no blank screens).
6. [ ] First-run firewall prompt appears (Windows Firewall asking to allow RSH);
   allow it → Settings → Firewall state shows rule present/enabled/exe path
   (`get_firewall_state`); the rule in `wf.msc` is named `RSH`, inbound, program-scoped.
7. [ ] WebView2: on a machine with WebView2 present, no download; on a machine
   without it, the bootstrapper downloads and installs it (silent).
8. [ ] Uninstall from "Apps & features": app removed, shortcut gone, firewall rule
   removed or reported (record which — the app does not own uninstall-time rule
   removal; if the rule survives, that is a finding, not a silent pass).

### 8.2 MSI installer (secondary)

1. [ ] Same SHA-256 check; install the `.msi`; record whether it prompts for admin
   and where it installs (WiX default scope — to be recorded, not assumed).
2. [ ] Launch + paint check as §8.1.5; firewall prompt + rule check as §8.1.6.
3. [ ] Install NSIS **over** MSI (and vice versa) — record what happens (same
   `upgradeCode`/product identity should upgrade, not duplicate; if it duplicates,
   that is a release-blocking finding).

### 8.3 HIDMaestro path (default backend)

1. [ ] Fresh install → Output page → HIDMaestro selected by default → connect:
   UAC prompt appears for `rsh-hm-sidecar.exe` (expected — elevated helper);
   sidecar `hello` succeeds; driver installs (first run, admin);
   `version` reports SDK 2.1.0 + DLL hash OK.
2. [ ] Wheel connected (or simulator): axes/buttons move the game input; disconnect
   → neutral; app exit → output neutral + device released.
3. [ ] vJoy fallback: HIDMaestro disabled → vJoy selected → missing-driver journey
   shows "vJoy not found — [Install vJoy] [Open Diagnostics]" (no crash, no fake output).

### 8.4 Upgrade over a previous install (v1.0.0 → v1.0.1 when it exists; for v1.0.0, install twice)

1. [ ] Install v1.0.0, create a profile + change a setting, close the app.
2. [ ] Install the same (or a newer) version over it: install succeeds, single
   entry in "Apps & features" (no duplicate), profiles + settings survive,
   app launches and paints.

### 8.5 Firewall rule edge cases

1. [ ] Non-elevated first run (standard user): firewall apply fails loudly in-app
   (not silently), app keeps working, Settings explains the manual allow step.
2. [ ] Third-party firewall managing the box: state page reports what Windows says.

## 9. Rollback test plan (Windows-only, owner PC)

1. [ ] **App downgrade:** with v1.0.1 installed, run the v1.0.0 NSIS installer →
   installs over v1.0.1 (allowed by `allowDowngrades: true`), app reports v1.0.0,
   single "Apps & features" entry. Same check for the MSI.
   (If the installer refuses the downgrade, `allowDowngrades` is not effective —
   release-blocking finding.)
2. [ ] **Profile/settings rollback:** after a bad upgrade, use in-app Backup/Restore
   (3-gen backup `current → .bak1 → .bak2`) to restore the previous profile;
   corrupt-store startup stays fail-closed (process exits, backups survive).
3. [ ] **Firmware is NOT rolled back by the app installer.** The board rejects
   downgrades by anti-downgrade; a firmware rollback, if ever needed, is a
   deliberate USB reflash with a signed image (see `docs/REFLASH-GUIDE.md`),
   never an automatic step. Record this in the release notes.
4. [ ] **Uninstall + reinstall previous release:** uninstall v1.0.1, install
   v1.0.0 from GitHub Releases, verify profiles in `%AppData%\RSH` survive the
   uninstall (they live outside the install dir) and load.

## 10. What is Windows-only (could not run in this sandbox)

- `cargo tauri build` (NSIS/MSI production) + installer contents.
- NativeAOT sidecar publish + real sidecar/driver round-trip (install/create/submit).
- Firewall COM sequence, UAC behaviour, `runas` path, registry probes.
- Updater E2E: publish `latest.json` → installed app checks → downloads signed
  update → installs it (passive mode) → relaunches on the new version; tampered
  `latest.json`/signature is rejected.
- Real vJoy DLL load/acquire/update + vJoy SHA-256 measurement (G-G3).
- Game matrix (0/16 verified) + real-board A7 run (carried, Phase H gate).
- `cargo audit` yanked check (timed out in sandbox before; re-run on CI / gate).

## 11. Open items / deferred (H4 scope, owned elsewhere)

| Item | Status | Owner / next step |
|---|---|---|
| MSIX | Deferred with reason (§1.1) | Future spike on Windows; needs allowElevation + driver-install proof |
| OV code-signing cert | Deferred (user decision D-R11) | Buy later ($150–300/year est.); app ships unsigned, SmartScreen expected |
| pid.codes PR for VID `0x1209` / PID `0x5253` | **Release gate, open** (Phase G D-G3) | Must merge before v1.0.0; human gate, not in the workflow |
| vJoy DLL SHA-256 (`PINNED_DLL_SHA256_HEX`) | Open (Phase G G-G3) | Measure on the Windows release machine; never invented |
| `firmware/` restore into the repo | Open, blocks `release.yml` | Coordinator: restore from `reference/firmware-v4.0.1/` before tagging |
| `RSH_UPDATER_PRIVATE_KEY` secret | Open | Coordinator/owner: create from `signing-keys/updater-minisign.key` |
| `RSH_SIGNING_KEY_PEM` secret | Pre-existing (C# series) | Verify present before the first release dry-run |
| `cargo tauri signer sign` E2E check of the new keypair | Crate-level cross-check PASS (§4); CLI sign E2E open | First Windows build / dry-run |
| `cargo cyclonedx` flags in `release.yml` | Resolved from the tool's documented usage (writes `bom.json` next to `Cargo.toml`; no `--output-file` flag — workflow copies `bom.json`); still to prove on first dry-run | First `workflow_dispatch` dry-run |
| CI `tauri-build` job (`npm run tauri`) | Broken, flagged (§6) | Coordinator: fix `ci.yml` (outside H4 file ownership) |
| Updater download/install wiring | Not in v1.0.0 (check-only) | Post-v1.0.0 step; `active: false` note in §4 is the record |
| 4-hour soak, H1 security re-scan, H2/H3/H5 | Other H workers / coordinator | Not H4 scope |

## 12. Pre-tag human gates (coordinator checks before pushing `v1.*`)

- [ ] Phase H gate green (H1–H5, final testing, A7 real-board run).
- [ ] pid.codes PR merged.
- [ ] `firmware/` restored in the repo; `RSH_SIGNING_KEY_PEM` + `RSH_UPDATER_PRIVATE_KEY` secrets set.
- [ ] `release.yml` dry-run (`workflow_dispatch`, `dry_run: true`) green, including
      the `cargo cyclonedx` step and the installer filename assumptions (§7).
- [ ] Clean-PC install + rollback plans (§8, §9) executed on Win10 + Win11, report filed.
- [ ] Release notes written (unsigned-app SmartScreen note, firmware anti-downgrade
      note, HIDMaestro UAC note, game matrix status).
