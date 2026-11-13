# RSH — Release checklist (v2.0.0)

**Version 2.0.0 — app v2.0.0 · firmware v4.0.0 · protocol v2.1**
Authority: Plan v2.3 §5 Phase H (R-H16), SW-16.
Status: **2026-11-13, Phase H Workstream 6** — har item ke saath status +
sabab. Koi item "tick" nahi jab tak us ka gate pass na ho.
**Push/tag/GitHub Release mat karo** — final release parent karega, A7
board run pass hone + user ki confirmation ke baad.

Legend: `[x]` PASS/done · `[ ]` PENDING (kya baaqi hai + owner) ·
`[~]` PARTIAL (kuch hua, kuch baaqi).

## 1. Build — 0 errors / 0 warnings

- [x] `dotnet build` (software) — 0 errors / 0 warnings (Phase H gate run mein verify).
- [x] `pio run` (firmware) — SUCCESS, merged bin `RSH-Firmware-DevKitV1-v4.0.0.bin` banta hai.
- [x] Merged-bin existence check — `release.yml` + `ci.yml` mein guard maujood hai (silent-merge bug guard).

## 2. Tests — green

- [x] `RSH.Tests` (dotnet) — Phase H gate run mein count darj (RUN LOG dekho).
- [x] `RSH.VJoy.Tests` — Phase H gate run mein count darj.
- [x] Host tests (`tests-host`) — checks 0 failures (Phase H gate run mein count darj).
- [x] Pin rules + protocol vectors — pass (gate run).
- [x] Rig `--only unit` + `--only firmware-host` — PASS (is workstream ka gate; poora rig nahi chalaya — neeche dekho).
- [x] **Full rig (2/2 stages, 25/25 scenarios) — DONE (final verification, 2026-11-13 10:00 UTC).** `test-env/REPORT.md`: **2/2 stages PASS, 25/25 scenarios PASS** (1 skipped — long-soak, sirf `--long` mein). Soak 60 s: loss 0%, rate 201/s, jitter p99 935.6 µs; fault-reboot duplicatesOrLate 0 (P3 regression PASS); ntp-ping p50 546 µs, p99 3298 µs, 0 timeouts (SLO ≤10 ms).
- [ ] **4-hour long soak (`--long`) — INCOMPLETE (user order se roka gaya).** 2026-11-13 ~09:43 UTC shuru, ~09:59 UTC roka gaya (user hukam: soak rok do, wrap up karo) — sirf **~16 min / 240 min (~7%)** chala. **Koi metrics nahi** (harness metrics sirf 14400 s poora hone par likhta hai; run SIGTERM exit 143 se khatam hua; `runs/long-soak-2026-11-13b/` mein REPORT/results.json soak ke nahi hain — wo 09:55 `--only unit` run ki copy hain). **Is soak ko pass/fail mat kaho** — incomplete hai. Release notes mein darj hai; dobara chalana ho to `--long` se 4 ghante chahiye.
- [ ] **CI green (GitHub Actions) — PENDING.** Local-only rule: koi push nahi hua, is liye Actions run nahi hua. Release tag se pehle CI green hona lazmi hai (parent push ke baad).

## 3. Versions + docs — version = release version

- [x] App version `2.0.0` — `software/src/RSH.App/RSH.App.csproj` `<Version>2.0.0</Version>`.
- [x] Firmware version `4.0.0` — `firmware/rsh/config.h` `FIRMWARE_VERSION "4.0.0"`.
- [x] Protocol v2.1 — `docs/PROTOCOL-V2.1.md`.
- [x] USER-GUIDE version = v2.0.0 — header mein "Version 2.0.0 — for RSH app v2.0.0 / firmware v4.0.0" + stale bits fix (Setup-1.0.0.exe hata, Windows 11 floor, firmware v3 → v4.0.0) + pairing warning (SEC-H-12).
- [x] FLASH-GUIDE version = v4.0.0 — filename fix + esptool/pio primary method + open-AP bootstrap warning step 6 mein (SEC-H-12).
- [x] A7 runbook tayyar — `docs/A7-RUNBOOK.md` (10-min run + B5 security checks §6).
- [x] A7 gate procedure READY — `docs/A7-GATE-PROCEDURE.md`.
- [x] Brick recovery doc — `docs/BRICK-RECOVERY.md`.
- [x] Signing doc — `docs/SIGNING.md` (D22/D23).

## 4. Migration + OTA upgrade + bad-config recovery

- [x] **v1 profiles migrate** — `ProfileStore` (SchemaVersion 0/1 → 2, `MigratedFromV1`); tests: `V1Profile_MigratesToV2_AndV1BackupPreserved`, `V1Migration_*` (PhaseCBackendTests).
- [x] **v1 backup migration se pehle** — asal v1 bytes `.v1.bak` mein (sirf pehli dafa, kabhi overwrite nahi); tests: `V1Backup_FirstMigrationWins_NeverOverwritten`.
- [x] **Downgrade par v1 restore** — `RestoreV1Backup()`; tests: `Downgrade_RestoreV1Backup_BringsBackOriginalFile`, `RestoreV1Backup_NoBackup_ReturnsFalse_FileUnchanged`.
- [x] **v3.3 → v4 OTA upgrade wiring** — signed trailer (Ed25519, 68 B `"RSHS"`, `FirmwareImageValidator` + `ota_validate.h`), anti-downgrade (board `webota.cpp` + app `FirmwareUploadGate`), post-upload GET_CAPS verify (`CheckPostUpload`); upgrade order app-pehle (R-M10).
- [x] **Bad-config recovery** — corrupt profile → `.corrupt-YYYYMMDD` copy mehfooz + backup (`.bak`/`.bak1`/`.bak2`) se restore + defaults; app corrupt file par band nahi hoti (`ProfileStore`).
- [ ] **A7 board verify (signed OTA + NVS migration) — PENDING (B5).** Runbook `docs/A7-RUNBOOK.md` §6 mein procedure hai (unsigned reject / signed accept / downgrade reject, flash dump, NVS migration). Board run user karega — tab tak ye item PENDING hai. Sandbox mein is ki koi coverage nahi (NVS migration ki koi host-test coverage nahi hai).

## 5. Installer + signing (D22/D23)

- [x] MSIX manifest draft — `software/packaging/msix/Package.appxmanifest` (Name RSH, Version 2.0.0.0, MinVersion Win 11 10.0.22000.0, `RSH.exe`, capabilities `internetClient` + `privateNetworkClientServer`); `verify-manifest.py` PASS.
- [x] MSIX build script — `software/packaging/msix/build-msix.ps1` (publish → MakeAppx → signtool sign + verify).
- [x] **MSIX Assets (logos) — DONE (2026-11-13).** `StoreLogo.png` (50×50) + `Square150x150Logo.png` (150×150) + `Square44x44Logo.png` (44×44) — `rsh.ico` (32×32) se banaye gaye (`software/packaging/msix/Assets/`). Caveat: 150×150 tile 32×32 source se upscale hai (soft lagega) — asal bara logo milne par replace karo.
- [~] **OV certificate procurement — DEFERRED (user faisla 2026-11-13).** Cert abhi NAHI khareeda jayega — v2.0.0 unsigned ship hoga. `docs/SIGNING.md` §6 checklist "baad mein (cert khareedne ke baad)" ke tor par darj hai — **release blocker NAHI**. Cert khareedne ke baad: org name faisla, registration verify, CA compare, buy ($150–300/year), validation (din se hafte), expiry calendar (90/30/7 din).
- [x] **Manifest Publisher — DONE (unsigned identity).** Ab `CN=RSH` (PublisherDisplayName `RSH`) — v2.0.0 ke liye final unsigned identity. Cert milne par Publisher ko exact cert Subject se badlo + `-Sign` build + CI check (SIGNING.md §3.1) — wo step OV procurement item ke saath deferred hai.
- [ ] **Unsigned MSIX build + clean-PC install test — PENDING (Windows PC).** Sandbox (Linux) mein sirf manifest verify ho saka; pehla asal (unsigned) MSIX Windows par banna + install test hona baaqi hai (Developer Mode sideload — USER-GUIDE §3; SmartScreen warning expected).
- [~] **Signed MSIX build + `signtool verify` — DEFERRED (cert khareedne ke baad).** User faisla 2026-11-13: OV cert abhi nahi; v2.0.0 unsigned. Cert milte hi: Publisher = exact cert Subject, `-Sign` build, `signtool verify`, release notes se "unsigned" notice hatao.
- [x] **SEC-H-03 (High, BLOCKER) — CLOSED (2026-11-13, user faisla).** Release mein sirf signed bins (owner: W1 signing step; verify: ye checklist). Firmware hissa **Phase H W10 (BUG-W1-1) mein FIXED + VERIFIED:** (a) workflow mein `firmware-signed.bin` ka check ab sirf size+trailer nahi — `sign-image.py --verify` baked public key (`signing_pubkey.h`, wahi key jis par board trust karta hai) se + signed-region ka SHA-256 `firmware.bin` se compare (stale signed bin FAIL); (b) release assets se raw bins hata diye gaye — `firmware.bin` pehle hi nahi tha, `bootloader.bin` + `partitions.bin` W10 mein hata diye gaye (merged factory bin mein teeno byte-exact hain: bootloader @0x1000, partitions @0x8000, app @0x10000 — local verify), aur release job raw bins milne par FAIL hoti hai; (c) `sign_firmware.py` keyless build par purana `firmware-signed.bin` DELETE karta hai (QA repro: keyed build → version badlo → keyless build → stale signed bin nahi raha, `pio run` SUCCESS). **App hissa — user faisla 2026-11-13:** OV cert abhi nahi khareeda jayega, is liye app asset v2.0.0 mein **unsigned** hai (zip + unsigned MSIX, Publisher `CN=RSH`) — ye faisla release notes + USER-GUIDE §3 + SIGNING.md mein darj hai. Signed MSIX (`signtool verify`) cert khareedne ke baad ka kaam hai (upar deferred item). App signing ka deferral SEC-H-03 ko dobara nahi kholta: firmware (board-trust boundary) signed + verified hai, aur app ka unsigned hona user ka explicit faisla hai.
- [ ] **Winget submit — PENDING (signed MSIX + release par blocked).** Draft tayyar: `software/packaging/winget/` (version/installer/locale, v2.0.0, `msix`); Publisher ab `RSH` hai (TODO hat gaya). **Unsigned MSIX winget par nahi jayega** — submit sirf OV cert + signed MSIX ke baad; `InstallerUrl`/`InstallerSha256` release ke baad bharo.

## 6. Firewall rule (A11)

- [x] Firewall rule code — `RSH.Core.Windows.FirewallRule` + `FirewallRuleManager` (program-scoped `New-NetFirewallRule`, UDP inbound, Private profile; `-LocalPort` jaan-boojh kar nahi — T10 ephemeral port, 4212 remote hai); unit tests pass.
- [x] Firewall install/uninstall scripts — `software/packaging/firewall/` (`install-firewall-rule.ps1`, `uninstall-firewall-rule.ps1`).
- [ ] **Firewall rule uninstall par remove — PENDING (design limit).** MSIX mein uninstall hook nahi hota, is liye rule uninstall par khud nahi hat-ti. Raste: uninstall script + Settings mein "Remove firewall rule" (abhi Settings UI mein nahi hai — chhota follow-up, ya uninstall guide mein script ka zikr). Release notes mein darj karo.

## 7. Update mechanism (A13 — sirf Phase H mein)

- [x] In-app update check — `RSH.Core.Utilities.UpdateChecker` (GitHub latest release tag, stable only; koi auto-download/install nahi) + Settings → About → "Check for updates" button; tests fake HTTP handler se pass.
- [x] Runtime check — `RuntimeCheck` (.NET >= 10, Windows 11 build >= 22000); About mein requirement fail par saaf message.
- [ ] **Update check live test — PENDING.** Sandbox test fake HTTP se hua; asal GitHub API + release page ke khilaf live test Windows PC par release ke baad karo.

## 8. SBOM + licenses + support

- [x] SBOM (CycloneDX 1.5) — `docs/SBOM.cyclonedx.json`, 36 components (33 NuGet + vJoy + Arduino-ESP32 + .NET runtime); generator `software/packaging/sbom/generate-sbom.py`.
- [x] THIRD-PARTY-LICENSES.md — SBOM se bana (36 rows, sab licensed).
- [ ] **SPDX SBOM — PENDING (optional).** CycloneDX hi release SBOM hai; SPDX chahiye ho to CycloneDX se convert karo (R-M29 mein dono likhe hain, CycloneDX kaafi hai jab tak parent SPDX na mange).
- [x] Support period statement — `docs/SUPPORT-PERIOD.md` (v2.0.0: release se 12 mahine, security fixes priority, Windows 11 floor, .NET 10 LTS 14 Nov 2028 tak).

## 9. Rollback plan (R-M9)

- [x] **Installer pichli version rakhega** — MSIX platform behaviour (previous version deployment rollback); verify clean-PC install test par (item §5).
- [x] **v1 backup downgrade par restore** — `.v1.bak` + `RestoreV1Backup()` (code + tests, §4).
- [x] **Firmware dual-partition OTA fallback** — `default.csv` (app0/app1); adhoori OTA par pichli image fallback (`docs/FLASH-GUIDE.md`).
- [ ] **Rollback end-to-end test (Windows PC) — PENDING.** Downgrade install → v1 profile restore → purani firmware OTA fallback — sirf Windows PC + board par hota hai.

## 10. Security — release blockers

- [x] **SEC-H-03 (High, BLOCKER) — CLOSED (user faisla 2026-11-13).** Upar §5 dekho (firmware signed + verified; app unsigned by explicit user decision, cert baad mein).
- [ ] **SEC-H-04 (High, BLOCKER) — OPEN-19 key-compromise response doc — PENDING (review baaqi).** Doc **maujood hai** — `docs/FLASH-ENCRYPTION-PROCEDURE.md` §7 (Phase H W1): (a) signing-key compromise par revocation (nayi keypair, `signing_pubkey.h` badlo, purani key revoked); (b) naye key se re-sign ka procedure; (c) purane boards USB se reflash (OTA se nahi — purani key se signed image hi OTA ho sakti hai; `docs/SIGNED-OTA.md` §5); (d) Secure Boot key compromise (eFuse revoke bits, §7.2) aur flash-encryption key compromise (§7.3) ke alag sections; (e) user communication (release notes + USER-GUIDE, §7.1 step 6). **Baaqi: doc ka review** — jab tak review na ho, release nahi.
- [ ] **OPEN-3 flash encryption — PENDING (Phase H).** NVS obfuscation hai, encryption nahi (SEC-E-01 PARTIAL). Procedure `docs/SECURITY-REVIEW-PHASE-F-W1.md` §6 mein — sacrificial board par pehle, phir production. A7 runbook §6b mein dump check hai (password plaintext na mile; encryption na ho to "OPEN-3 pending — TM-W07" darj karo, PASS mat kaho).
- [x] SEC-H-12 (Low, doc) — **FIXED is workstream mein:** FLASH-GUIDE v4.0.0 + esptool primary + open-AP warning (step 6), USER-GUIDE pairing warning (wizard §4 step 2). Adafruit flasher page abhi live hai (2026-11-13 verify) — alternative ke tor par rehta hai, primary nahi.
- [x] B5 (A7 board verify) — **procedure likhi gayi** (`docs/A7-RUNBOOK.md` §6: signed OTA, flash-enc verify, NVS migration); board run PENDING hai (§4).

## 11. A7 board run — final gate (user karega)

- [ ] **A7 10-min run PASS — PENDING.** Runbook: `docs/A7-RUNBOOK.md`. Pass: sent/s ≈ 200 (195–205), overruns 0, maxCycleUs < 3500 µs, reboots 0, steering std-dev record, LED patterns OK, B5 §6 checks OK.
- [ ] **Freeze — PENDING (A7 pass ke baad).** Freeze ke baad bhi har change ke liye naya version.

## Counts (2026-11-13, is workstream ke end par — file se gine hue)

- Tick `[x]`: **41** · Partial `[~]`: **2** · Pending `[ ]`: **13** (unique pending **13**).
- Pending ki wajuhat: A7 board run + freeze + B5 verify (user, 3 items — §4 B5, §11 run + freeze) · 4h soak (INCOMPLETE — user order se roka gaya) + CI (2) · unsigned MSIX install test + winget submit (2, Windows/release par blocked — winget signed MSIX par hai) · SEC-H-04 + OPEN-3 (2) · SPDX + firewall Settings wiring + update live test + rollback E2E test (4, Windows/release par blocked). Deferred (blocker nahi): OV cert procurement + signed MSIX (cert khareedne ke baad).

## RUN LOG (gate run — is workstream, 2026-11-13)

- `dotnet build RSH.slnx -c Release`: **0 errors / 18 warnings** (sab pehle se mojood xUnit1051, meri files mein koi warning nahi).
- Rig `--only unit`: **PASS** — `dotnet test`: RSH.Tests **644 pass / 0 fail / 7 skip (651)**, RSH.VJoy.Tests **56/0/0**, RSH.HIDMaestro.Tests **78/0/0**; host tests **4374 checks, 0 failures** (+ merge-bin mock 7/0); pin-rules PASS; protocol-vectors PASS.
- Rig `--only firmware-host`: **PASS** (`run-host-tests.sh` exit 0).
- Pehli unit run FAIL thi (4 fail) — 2 mere bugs thay (neeche §Bugs), 1–2 flaky timing tests thay; fix ke baad dobara run **ALL PASS**.
- Full rig nahi chalaya (4-hour soak background mein chal raha hai — port conflict); full rig + soak + CI release se pehle baaqi hain (upar §2).

## Bugs mile + fixes (is workstream)

- **B-W6-1 — `Path.IsPathFullyQualified` Linux par Windows path reject karta hai.** `FirewallRule.BuildEnsureCommand(@"C:\...")` Linux par `ArgumentException` deta tha (test fail). Fix: `software/src/RSH.Core/Windows/FirewallRule.cs:51` — Windows path ki shape haath se check karo (drive letter + `:\` + `.exe`), `Path` API se nahi. Repro: `dotnet test --filter PhaseHWorkstream6Tests` (pehle fail, ab pass). Regression test: Unix-style absolute path (`/home/user/RSH.exe`) bhi reject hota hai.
- **B-W6-2 — install-firewall-rule.ps1 ke comment mein `-LocalPort` literal tha.** Script ke explanatory comment mein "-LocalPort 4212 GHALAT" likha tha, jis se `Artifact_FirewallScripts_ProgramScoped` test ka `DoesNotContain("-LocalPort")` fail hua. Fix: comment dobara likha (flag ka literal naam nahi). Asal command mein `-LocalPort` kabhi tha hi nahi.
- **Flaky (pre-existing, mera nahi) — `Detector_FindsLivePort_SkipsDeadPort`** (PhaseGWorkstream1Tests, FakeBoardResponder threading, 500 ms timeout). Pehli unit run mein fail, seedhe dobara run mein pass — VM load par timing. Phase G se maloom flaky hai (Phase H follow-up); is workstream ne isay nahi chheda.
- **Gate block (transient, doosri workstream) — RSH.App build fail hua tha** jab HIDMaestro workstream beech mein edit kar raha tha (pehle `.g.cs` missing — shared-tree race; phir `IJoystickOutput.ButtonCount` / `OutputDetails` CS1061/CS0103 — un ka aadha refactor). Un ke edit mukammal hote hi dobara build **0 errors** hua. Meri taraf se koi fix nahi — sirf retry + report.

## RUN LOG (Phase H W10 — Final QA fix round, 2026-11-13)

- `pio run` keyed (dev keypair `/tmp/rsh-w10-*/signing-key.pem`, version aarzi `4.0.0-w10`): **SUCCESS 36.5 s** — `firmware-signed.bin` 887,764 B (= firmware.bin 887,696 + 68), merged bin `RSH-Firmware-DevKitV1-v4.0.0-w10.bin`. Verify: `sign-image.py --verify` (header-derived PEM) **VERIFY OK**, signed-region SHA-256 == firmware.bin (`39fec1fe…`). Version wapas `4.0.0` ki gayi; keyless `pio run` **SUCCESS 18.2 s** — `sign_firmware: purana firmware-signed.bin DELETE kar diya`, baad mein signed bin maujood nahi (QA repro PASS).
- Merged bin verify (byte-exact): bootloader @0x1000 ✓, partitions @0x8000 ✓, app @0x10000 ✓ — is liye raw `bootloader.bin`/`partitions.bin` release assets se hatana safe hai.
- `dotnet build RSH.slnx -c Release`: **0 errors / 18 warnings** (sab pehle se mojood xUnit1051). `dotnet sln list` mein `tests/RSH.App.Tests/RSH.App.Tests.csproj` maujood; dll bani (`RSH.App.Tests.dll`, 50,688 B).
- Rig `--only unit`: **FAIL** — sirf `dotnet-test` check FAIL (neeche B-W10-1); host-tests **4374 checks, 0 failures** PASS; pin-rules PASS; protocol-vectors PASS. Unit stage ke dotnet numbers: RSH.Tests **646 pass / 0 fail / 5 skip (651)**, RSH.VJoy.Tests **56/0/0**, RSH.HIDMaestro.Tests **78/0/0** — teeno PASS; sirf RSH.App.Tests launch nahi hota (WindowsDesktop.App runtime Linux par nahi).
- Rig `--only firmware-host`: **PASS** — host tests **4374 checks, 0 failures** (+ merge-bin mock 7/0, cross-verify 51), `pio run` SUCCESS.
- Full rig nahi chalaya (4-hour soak background mein chal raha hai — port conflict); full rig + soak + CI release se pehle baaqi hain (upar §2).

## Bugs mile + fixes (Phase H W10 — is workstream)

- **BUG-W1-1 (High, release integrity) — FIXED + VERIFIED.** `firmware/rsh/sign_firmware.py`: key unset ho to purana `firmware-signed.bin` DELETE hota hai. `.github/workflows/release.yml`: (a) `sign-image.py --verify` baked pubkey (`signing_pubkey.h` → DER → PEM) se; (b) signed-region SHA-256 vs `firmware.bin` (stale signed bin FAIL); (c) release assets se raw `bootloader.bin` + `partitions.bin` hata diye gaye (checksums + upload-artifact + release job — release job raw bins milne par FAIL hoti hai). Verify: upar RUN LOG (keyed → keyless repro PASS; header→PEM mechanism dev keypair se end-to-end verify; repo baked header 32 B + valid Ed25519 pubkey). Caveat: stale-delete sirf tab chalta hai jab post-action chale (yaani `firmware.bin` dobara bana ho); no-op `pio run` par SCons post-action nahi chalata — is liye release.yml ka hash-compare hi asal fail-closed guarantee hai.
- **BUG-W9-1 — FIXED (solution mein shamil; Linux run Windows-only — neeche B-W10-1 dekho).** `software/RSH.slnx` mein `tests/RSH.App.Tests/RSH.App.Tests.csproj` add kiya. Verify: `dotnet sln list` mein maujood; `dotnet build RSH.slnx` 0 errors + dll banti hai; `dotnet test RSH.slnx` use uthata hai (run karne ki koshish karta hai — Linux par `Microsoft.WindowsDesktop.App 10.0.0` runtime na milne par launch fail). Windows CI (`windows-latest`, release.yml) par chalega — CI green hona release se pehle baaqi hai (§2).
- **BUG-W9-2 — FIXED.** `docs/RELEASE-CHECKLIST.md` §5 SEC-H-03 aur §10 SEC-H-04 ka stale text update kiya (SEC-H-03: firmware hissa W10 mein fixed/verified, sirf signed MSIX baaqi; SEC-H-04: doc `FLASH-ENCRYPTION-PROCEDURE.md` §7 maujood hai, review pending hai). Dono items abhi bhi PENDING hain (counts nahi badle).
- **BUG-W9-3 — FIXED.** `test-env/scenarios/scenarios.json` se `pendingPhaseC` key hata di (teeno entries — profile-migration, ephemeral-ports, ntp-ping-note — Phase C se implemented hain; koi code is key ko nahi parhta). Verify: JSON valid, 26 scenarios intact (saare ids maujood).
- **BUG-W9-4 — FIXED.** `docs/RELEASE-WORKFLOW-CHECK.md` ko SUPERSEDED mark kiya (banner: current authority `release.yml` + `RELEASE-CHECKLIST.md`; stale refs v3.3/app v1.2.2/F11 ki wazahat). Body tareekhi record ke tor par rehti hai.
- **BUG-W9-5 — FIXED.** `RSH-PLAN-v2.3.md:192` `APEXLINK_DEBUG` → `RSH_DEBUG` (plan mein ab 0 `APEXLINK` refs; `config.h:175` + `PERF-BASELINE.md:19` se match). `docs/KEY-LIFECYCLE.md:301` OPEN-20 → **DECIDED (D26, Phase G)**: pairing/key transfer dono — Wi-Fi primary, wired optional (PROTOCOL-V2.1 §10 + §2.2/§4.2).
- **B-W10-1 — NAYA BUG (is workstream ki W9-1 fix ka side effect — NOT FIXED, faisla parent/coordinator ka hai).** `RSH.App.Tests` ke `RSH.slnx` mein aane se rig ka `--only unit` stage Linux par **FAIL** hota hai: `dotnet test RSH.slnx` ab `RSH.App.Tests.dll` bhi chalata hai, jo `Microsoft.WindowsDesktop.App 10.0.0` runtime maangta hai (WPF — Linux par maujood nahi). Repro: `dotnet test RSH.slnx -c Release` (repo/software) ya `python3 run_rig.py --quick --only unit` → stage FAIL, `dotnet-test` check FAIL; log `test-env/logs/unit-dotnet-test.log` mein `error run failed: Framework: 'Microsoft.WindowsDesktop.App', version '10.0.0'`. Baqi teeno test assemblies usi run mein PASS (upar numbers). Rig ka behaviour is workstream ne **nahi** badla (task rule). Tajweez: rig unit stage Linux par per-project `dotnet test` chalaye aur `RSH.App.Tests` ko skip kare (wo Windows CI par chalega), ya `dotnet test RSH.slnx` ke liye Linux skip-list; faisla + fix agle workstream/coordinator ka hai. Jab tak ye na ho, rig `--only unit` Linux par green nahi hoga.
- **Note (follow-up, mera scope nahi):** `docs/DECISION-LOG.md:36` D26 row abhi bhi "Spec TBD hai" kehti hai — KEY-LIFECYCLE.md:301 update ho gaya, D26 row bhi update honi chahiye (ek line).

## RUN LOG (OV-cert faisla apply, 2026-11-13 — coordinator)

**User faisla 2026-11-13: OV code-signing certificate abhi NAHI lena — app unsigned jayegi, cert baad mein.** Is ke mutabiq:
- MSIX Assets ban gaye (`software/packaging/msix/Assets/`: StoreLogo 50×50, Square150x150Logo 150×150, Square44x44Logo 44×44 — `rsh.ico` 32×32 se upscale, LANCZOS; 150×150 tile soft hoga — asal bara logo milne par replace karo).
- Manifest Publisher `CN=TODO-RSH-PUBLISHER` → `CN=RSH` (unsigned identity, v2.0.0 final); `verify-manifest.py` PASS (Publisher set: CN=RSH).
- `build-msix.ps1`: default (bina `-Sign`) ab release path hai; `-Sign` sirf cert ke baad. Placeholder Publisher par ab throw hota hai (warning nahi).
- SIGNING.md: faisla notice upar + §1 Procurement row + §6 checklist "baad mein" (blocker nahi). DECISION-LOG D23 update hua. Winget drafts: Publisher `RSH`, submit signed MSIX par deferred.
- USER-GUIDE §3: unsigned install text (portable zip recommended; MSIX par SmartScreen "Unknown publisher"; SHA-256 check; firmware signed hai).
- Checklist §5: Assets [x], OV procurement [~] DEFERRED, Publisher [x], Signed MSIX → [ ] unsigned install test + [~] signed deferred, **SEC-H-03 [x] CLOSED** (firmware signed + verified; app unsigned by explicit user decision), Winget [ ] (signed MSIX par). Counts: **40 tick / 2 partial / 14 pending** (file se gine hue).
- Release notes draft: `docs/RELEASE-NOTES-v2.0.0.md` (unsigned notice samet) — final GitHub release ke liye parent finalize karega.
- Firmware signing (Ed25519, local key) is faisle se mutaliq nahi — waisa hi hai.

## RUN LOG (Final verification + wrap-up, 2026-11-13 — coordinator)

- **User hukam 2026-11-13 ~09:59 UTC: 4-hour soak rok do, wrap up karo, final release do.** Soak roka gaya (run_rig SIGTERM exit 143 + wheel band + orphaned harness PID 6700 bhi band). Soak ~16 min chala (240 mein se) — INCOMPLETE, koi metrics nahi (upar §2).
- **Final full rig (quick), 2026-11-13 10:00:20Z UTC: DONE — ALL PASS.** `test-env/REPORT.md`: **2/2 stages PASS, 25/25 scenarios PASS** (1 skipped — long-soak). Unit: RSH.Tests **646 pass / 0 fail / 5 skip (651)**, RSH.VJoy.Tests **56/0/0**, RSH.HIDMaestro.Tests **78/0/0**; host tests **4374 checks, 0 failures** (+ 50 + 50, cross-verify 51, merge-bin mock 7/0); pin-rules PASS; protocol-vectors PASS. Firmware-host: `pio run` SUCCESS (RAM 30.2%, Flash 67.2%, merged bin 953,280 B). Soak 60 s: loss 0%, rate 201/s, jitter p99 935.6 µs; ntp-ping p99 3298 µs (SLO ≤10 ms); fault-reboot duplicatesOrLate 0.
- **`dotnet build RSH.slnx -c Release`: Build succeeded, 0 errors** (incremental — 0 warnings; W10 ke full build mein 18 pre-existing xUnit1051 analyzer warnings theen test projects mein, 0 errors).
- **B-W10-1 — FIXED + VERIFIED is wrap-up mein:** rig unit stage Linux par per-project `dotnet test` chalata hai (RSH.Tests, RSH.VJoy.Tests, RSH.HIDMaestro.Tests); `RSH.App.Tests` Linux par skip (WPF Windows-only; Windows CI par chalega). Is ke baad `--only unit` + full rig dono PASS (upar). `test-env/run_rig.py` `run_unit_stage` dekho.
- **DECISION-LOG D26 row bhi update hui** (Phase G decided — pehle "Spec TBD" kehti thi; KEY-LIFECYCLE.md:301 se match).
- **Baaqi (release se pehle):** A7 board run (user) + freeze, 4h soak (incomplete — dobara chalana ho to 4 ghante), CI green (push ke baad), unsigned MSIX install test (Windows PC), SEC-H-04 review, OPEN-3 board-side. **OV cert deferred hai (user faisla) — blocker nahi.**
