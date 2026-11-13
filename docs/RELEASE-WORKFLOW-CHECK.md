# Release Workflow Check — WP10 (Plan v2.3)

- **File under review:** `.github/workflows/release.yml` (100 lines, `name: Build and Release`)
- **Reviewer:** Testers/QA team (tdd-guide role) — WP10 static validation
- **Date:** 2026-11-12
- **Scope: STATIC VALIDATION ONLY.** Workflow GitHub Actions par run **nahi** hua (push mana hai — local-only rule). Virtual Test Rig (`~/workspace/projects/rsh/test-env/`) abhi tayyar nahi hai, is liye ye test run nahi, sirf YAML + static checks hain. Koi fix implement nahi ki — sirf findings + fix wording neeche hai.
- **Verdict: YAML valid hai, lekin workflow abhi release-ready NAHI hai — 2 Blockers (F1, F2) + 2 Major (F3, F4) pehle fix hone chahiye.**

## 1. YAML syntax validation

- **Result: VALID.** `python3 -c "yaml.safe_load(...)"` (PyYAML 6.0.1) ne `.github/workflows/release.yml` baghair error ke parse kar liya.
- **Top-level keys:** `name`, `on`, `permissions`, `jobs`. Jobs: `firmware`, `software`, `release`.
- **`on:` boolean-quirk (note, bug nahi):** YAML 1.1 mein `on` ko boolean `True` samjha jata hai — `safe_load` ke result mein key `True` nazar aati hai (`{True: {push: {tags: ['v*']}, ...}}`). Ye PyYAML ka maroof quirk hai; GitHub Actions YAML 1.2 parser use karta hai aur `on:` ko sahi samajhta hai. Koi action nahi chahiye. (Isi liye validation mein `d.get(True, d.get('on'))` se parha gaya.)
- **actionlint / yamllint:** Local machine par maujood nahi thay (`which actionlint yamllint` — kuch nahi mila), is liye linting nahi ho saki. Pehli green run se pehle `actionlint` se check karna checklist mein hai (§5).

## 2. Har job ka static review

### 2.1 Trigger

| Cheez | Workflow mein | Faisla |
|---|---|---|
| Tag push | `push: tags: ["v*"]` (line 8) | Sahi, **lekin D27 se mismatch** — dekho F2 |
| Manual | `workflow_dispatch` + `tag` input, required, default `"v1.2.2"` (lines 10–15) | Kaam karta hai, lekin default stale hai — dekho F11 |
| Permissions | `contents: write` (lines 17–18) | Sahi — release banane ke liye zaroori hai |

### 2.2 Job: `firmware` (lines 20–47, `ubuntu-latest`)

- Runner sahi hai — ESP32/PlatformIO build ko Windows ki zaroorat nahi.
- Steps: checkout → setup-python 3.12 → cache (`~/.cache/pip`, `~/.platformio`, key `pio-<hash of platformio.ini>`) → `pip install -U platformio` → `pio run` (working-dir `firmware/rsh`) → `sha256sum *.bin > SHA256SUMS.txt` → upload-artifact `firmware` (`*.bin` + `SHA256SUMS.txt`).
- **PASS:** `platformio.ini` mein `platform = espressif32@6.9.0` pinned hai (task background se match), `extra_scripts = post:merge_bin.py` maujood hai, `merge_bin.py` repo mein hai.
- **PASS:** Merged bin ka naam `RSH-Firmware-DevKitV1-v3.3.bin` bane ga — `config.h` line 65 mein `#define FIRMWARE_VERSION "3.3"` hai, aur `merge_bin.py` usi se naam banata hai. Task background se match.
- **FAIL:** Merged-bin existence check **nahi hai** — dekho **F1 (Blocker)**.
- **Note:** `sha256sum *.bin` step khud fail ho jaye ga agar koi `.bin` hi na ho (bash glob expand nahi hota), lekin merged bin ke baghair baqi 3 bins (`firmware.bin`, `bootloader.bin`, `partitions.bin`) maujood hon to step green rehta hai — ye F1 ko nahi rokta.

### 2.3 Job: `software` (lines 49–79, `windows-latest`)

- **Runner sahi hai — `windows-latest` lazmi hai.** `software/src/RSH.App/RSH.App.csproj` mein `<TargetFramework>net10.0-windows</TargetFramework>` + `<UseWPF>true</UseWPF>` hai. WP6/CI wali wajah yahan bhi lagti hai: `dotnet build` Linux par bhi compile ho jata (`EnableWindowsTargeting`), lekin `dotnet test` Windows-targeted test assembly (`RSH.VJoy.Tests`, `net10.0-windows`) ko ubuntu-latest par execute nahi kar sakta. (CI workflow `ci.yml` mein yehi wajah comment mein likhi hai.)
- **Paths — sab maujood hain (verify kiye gaye):**
  - `software/RSH.slnx` — maujood; `src/RSH.App`, `src/RSH.Core`, `src/RSH.Output.VJoy`, `tests/RSH.Tests`, `tests/RSH.VJoy.Tests`, `tools/RSH.Benchmarks`, `tools/RSH.Simulator` refer karta hai.
  - `software/src/RSH.App/RSH.App.csproj` — maujood; `<Version>1.2.2</Version>`, `<AssemblyName>RSH</AssemblyName>`.
  - `software/global.json` — maujood; `{"sdk": {"rollForward": "latestPatch", "version": "10.0.100"}}`. **Note:** ye `software/` mein hai, repo root par nahi.
  - `software/tests/RSH.Tests/RSH.Tests.csproj` (`net10.0`) aur `software/tests/RSH.VJoy.Tests/RSH.VJoy.Tests.csproj` (`net10.0-windows`) — dono maujood.
- **`dotnet test` fail ho to job rukti hai? — HAAN (PASS).** Step mein `continue-on-error` nahi hai, aur `dotnet test` test fail hone par non-zero exit code deta hai, jis se GitHub step fail ho jata hai aur job ruk jati hai. Koi finding nahi — lekin per-project breakdown nahi hai (dekho F5, Minor).
- **VJoy driver ki zaroorat nahi:** `RSH.VJoy.Tests` ke tests driver ke baghair chalte hain (layout test + DLL-absent test; file header: "Ye tests kisi driver ke baghair chalte hain"). Windows runner par vJoy driver install kiye baghair bhi `dotnet test` green rehna chahiye.
- **Publish → zip paths consistent hain (PASS):** `dotnet publish src/RSH.App/RSH.App.csproj -o publish/RSH` (working-dir `software`, is liye output `software/publish/RSH/`), phir zip step repo root se `Compress-Archive -Path software/publish/RSH/* -DestinationPath RSH-windows-x64.zip` — path match karta hai. Zip ka naam `RSH-windows-x64.zip` task background se match.
- **FAIL:** SDK pin floating hai — dekho **F3 (Major)**.
- **Note:** `software/publish/` local working copy mein maujood nahi hai — ye build artifact directory hai, publish step se banti hai. Is ka na hona koi masla nahi.

### 2.4 Job: `release` (lines 81–100, `ubuntu-latest`, `needs: [firmware, software]`)

- **PASS:** `needs` sahi hai — firmware ya software fail ho to release job skip hoti hai (default `if: success()`).
- **PASS:** `download-artifact` dono artifacts `artifacts/` mein lata hai; `gh release create` mein paths `artifacts/firmware/*.bin`, `artifacts/firmware/SHA256SUMS.txt`, `artifacts/software/RSH-windows-x64.zip`, `artifacts/software/SHA256SUMS-app.txt` — upload-artifact names (`firmware`, `software`) se match karte hain.
- **PASS:** Release mein SHA256SUMS shamil hai — firmware wali (`SHA256SUMS.txt`, bins ke hashes) aur app wali (`SHA256SUMS-app.txt`, zip ka hash) dono upload hoti hain. Lekin do alag naam/format hain — dekho F7 (Minor).
- **PASS:** `gh` CLI ubuntu-latest runner par pehle se maujood hota hai; `GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}` + `contents: write` permission release banane ke liye kafi hai.
- **FAIL:** `TAG` expression aur dispatch path mein masail hain — dekho **F4 (Major)** aur **F2 (Blocker)**.
- **Note:** Is job mein `actions/checkout` step nahi hai — artifacts ke liye zaroorat bhi nahi, lekin F4 ki wajah se ye relevant hai (neeche dekho).

### 2.5 CI workflow se farq (WP6 `ci.yml` — sirf note, chheda nahi)

| | `ci.yml` (WP6) | `release.yml` (WP10) |
|---|---|---|
| Trigger | Har push / PR (`branches: ["**"]`) + dispatch | Sirf tag `v*` push + dispatch |
| Maqsad | Build + test, kuch release nahi karta | Firmware + app build kar ke GitHub Release banata hai |
| Permissions | `contents: read` | `contents: write` (release ke liye zaroori) |
| Concurrency | `ci-<ref>`, cancel-in-progress | **Koi concurrency group nahi** (F8) |
| SDK pin | `"10.0.100"` exact + global.json se verify step | `"10.0.x"` floating, koi verify nahi (F3) |
| Merged-bin check | Maujood — explicit existence check + fail (CI mein "silent-merge bug guard") | **Ghair-maujood** (F1) |
| Platform pin check | `grep -Fq 'platform = espressif32@6.9.0'` step | Nahi hai (F9) |
| Tests | Har test project alag step mein (T5 breakdown) | Ek combined `dotnet test RSH.slnx` (F5) |

## 3. Findings

### F1 — BLOCKER — Merged factory bin ka existence check nahi hai

- **Jagah:** `.github/workflows/release.yml`, firmware job, `pio run` step (line 38) ke baad — Checksums step (lines 39–41) se pehle koi check nahi.
- **Masla:** `firmware/rsh/merge_bin.py` lines 36–39: koi input (`firmware.bin`, `bootloader.bin`, `partitions.bin`, `boot_app0.bin`) missing ho to stderr par `"merge_bin: missing … — skipping merge"` likh kar **khamoshi se `return`** karta hai — `pio run` phir bhi exit 0 deta hai aur green rehta hai. Release workflow mein is ke baad `sha256sum *.bin` aur `upload-artifact *.bin` baqi 3 bins ke saath kamyab ho jate hain, aur release **merged factory bin (`RSH-Firmware-DevKitV1-v3.3.bin`) ke baghair** ban jati hai. Yehi woh bin hai jo FLASH-GUIDE ke mutabiq factory flash image hai — us ke baghair release tooti hui hai aur kisi ko pata bhi nahi chale ga.
- **Saboot:** CI workflow (`ci.yml`, firmware job) mein yehi guard pehle se maujood hai — "Check merged factory bin EXISTS (silent-merge bug guard)" — release workflow mein us ka koi counterpart nahi.
- **Fix:** `release.yml` mein `pio run` step (line 38) ke baad, Checksums step se pehle, CI wala step copy karo:
  ```yaml
  - name: Check merged factory bin EXISTS (silent-merge bug guard)
    run: |
      BUILD_DIR="firmware/rsh/.pio/build/esp32dev"
      ls -la "$BUILD_DIR"
      test -f "$BUILD_DIR/firmware.bin" || { echo "MISSING: firmware.bin (app-only OTA image)"; exit 1; }
      MERGED=$(ls "$BUILD_DIR"/RSH-Firmware-DevKitV1-v*.bin 2>/dev/null | head -n 1)
      if [ -z "$MERGED" ]; then
        echo "MISSING: merged factory bin RSH-Firmware-DevKitV1-v*.bin — merge_bin.py khamoshi se fail hua?"
        exit 1
      fi
      echo "Merged factory bin OK: $MERGED"
  ```

### F2 — BLOCKER — Tag pattern `v*` D27 version scheme se match nahi karta

- **Jagah:** `.github/workflows/release.yml`, line 8 (`tags: ["v*"]`).
- **Masla:** Plan v2.3 Decision D27 kehta hai: app release tag `v2.0.0`, firmware tag `fw-v4.0.0`. Firmware tag `fw-v4.0.0` `"v*"` glob se **match hi nahi karta** (ye `fw-` se shuru hota hai) — D27 ke mutabiq firmware tag push karne par release workflow chalega hi nahi, khamoshi se kuch nahi hoga. Dosra pehlu: workflow app aur firmware tag mein farq bhi nahi kar sakta — koi bhi `v*` tag dono builds (firmware + app) chala deta hai, jabke D27 mein dono ke alag tags hain.
- **Fix (pehle D27 ko confirm karo, phir in mein se ek):**
  - Option A (tajweez): trigger badlo — `tags: ["v*", "fw-v*"]` (line 8), aur release job mein tag ke mutabiq sirf mutaliqa artifacts upload karo (ya do alag workflows: app release vs firmware release).
  - Option B: D27 badlo — firmware ke liye bhi `v*` tag use karo (jaise `v4.0.0-fw`), lekin phir app tag se takrao ka hal bhi chahiye.
  - Jab tak ye faisla nahi hota, pehli release ke liye sirf app tag (`v2.0.0`) hi workflow chalayega; firmware tag par kuch nahi hoga.

### F3 — MAJOR — .NET SDK pin floating hai (`10.0.x`), global.json se match nahi karta

- **Jagah:** `.github/workflows/release.yml`, line 57 (`dotnet-version: "10.0.x"`).
- **Masla:** `software/global.json` SDK `10.0.100` pin karta hai (`rollForward: latestPatch`), aur CI (`ci.yml`) exact `"10.0.100"` + verify step use karta hai. Release workflow floating `10.0.x` install karta hai — kal ko naya patch aate hi release build CI/tested build se mukhtalif SDK par banega; reproducibility toot jati hai. Mazeed: Setup .NET step repo root par chalta hai (koi `working-directory` nahi), jahan `global.json` maujood hi nahi (wo `software/` mein hai) — is liye setup waqt global.json ka pin lagta hi nahi; baad wale `dotnet` commands `software/` se chalte hain aur `rollForward: latestPatch` ki wajah se naya patch qubool kar lete hain.
- **Fix:** Line 57 badlo — `dotnet-version: "10.0.100"` — aur CI jaisa verify step software job mein shamil karo:
  ```yaml
  - name: Verify SDK pin matches global.json
    shell: pwsh
    run: |
      $sdk = (dotnet --version).Trim()
      $pinned = (Get-Content software/global.json | ConvertFrom-Json).sdk.version
      Write-Host "SDK: $sdk, global.json pin: $pinned"
      if ($sdk -ne $pinned) { throw "SDK $sdk != global.json pin $pinned" }
  ```

### F4 — MAJOR — `workflow_dispatch` path par release galat commit par ban sakta hai; re-run fail hogi

- **Jagah:** `.github/workflows/release.yml`, lines 89–100 (release job), khaas tor par line 93 (`TAG` expression) aur line 96 (`gh release create "$TAG"`).
- **Masla 1 — tag ka target ghair-wazeh:** Release job mein `actions/checkout` step nahi hai. Tag push par `gh release create` maujooda tag par release banata hai (theek hai), lekin `workflow_dispatch` par diya gaya tag (masalan `v1.2.2`) aam tor par git tag ke tor par maujood nahi hota — `gh release create` phir API se tag banata hai, aur baghair checkout/`--target` ke wo default branch ke HEAD par lagta hai, jo us commit se mukhtalif ho sakta hai jis par dispatch chalaya gaya tha. Natija: release galat commit par tag ho jati hai.
- **Masla 2 — re-run idempotent nahi:** Agar us tag ki release pehle se maujood ho to `gh release create` fail ho jata hai ("already exists") — dobara chalane ka koi rasta nahi (na `--clobber`/delete-then-create, na skip logic).
- **Masla 3 — expression nazuk hai:** Line 93 (`github.ref_name != '' && startsWith(github.ref, 'refs/tags/') && github.ref_name || inputs.tag`) kaam to karta hai (tag push → `ref_name`, dispatch → `inputs.tag`), lekin `github.ref_name != ''` hamesha true hota hai aur precedence par inhisaar karta hai — parhne wale ko gumrah karta hai.
- **Fix:**
  - Release job mein `actions/checkout` step shamil karo aur `gh release create` mein `--target "$GITHUB_SHA"` lagao (dispatch par release usi commit par bane jis par workflow chala).
  - Ya dispatch input ko sirf maujooda tag ke liye allow karo aur step ke shuru mein `git rev-parse "$TAG"` se tag ki maujoodgi check karo.
  - Re-run ke liye: release step se pehle `gh release view "$TAG" >/dev/null 2>&1 && gh release delete "$TAG" -y` (ya `--clobber` jaisa flow) ka faisla karo aur document karo — warna dobara run karne par job fail hogi.
  - Line 93 ko saaf karo, masalan: `${{ startsWith(github.ref, 'refs/tags/') && github.ref_name || inputs.tag }}`.

### F5 — MINOR — `dotnet test` ek combined step mein hai — fail hone par pata nahi chalta kaunsa project fail hua

- **Jagah:** `.github/workflows/release.yml`, lines 58–61 (`dotnet test RSH.slnx -c Release --nologo`).
- **Masla:** Test fail hone par job ruk to jati hai (dekho §2.3 — PASS), lekin CI (T5) ke bar-aks dono test projects (`RSH.Tests`, `RSH.VJoy.Tests`) ek hi step mein hain — kaunsa project fail hua, ye log khol kar dhoondna parta hai.
- **Fix:** CI jaisa do alag steps banao — pehle `dotnet build software/RSH.slnx -c Release --nologo`, phir `dotnet test software/tests/RSH.Tests/RSH.Tests.csproj -c Release --no-build --nologo` aur `dotnet test software/tests/RSH.VJoy.Tests/RSH.VJoy.Tests.csproj -c Release --no-build --nologo`.

### F6 — MINOR — Zip banne se pehle `RSH.exe` ki maujoodgi verify nahi hoti

- **Jagah:** `.github/workflows/release.yml`, lines 67–73 (Zip app step).
- **Masla:** `Compress-Archive -Path software/publish/RSH/*` khaali folder par fail ho jata hai (acha hai), lekin agar publish mein DLLs hon aur `RSH.exe` na ho (masalan publish ghalat project/output par hua ho) to bhi zip ban jata hai aur release mein chala jata hai. Zip mein koi version file / README bhi nahi hai — user ko zip khol kar bhi nahi pata chale ga ke ye kis version ki build hai.
- **Fix:** Zip step ke shuru mein shamil karo:
  ```powershell
  if (-not (Test-Path software/publish/RSH/RSH.exe)) { throw "RSH.exe missing after publish" }
  ```
  Aur ghaur karo ke zip mein version (masalan `VERSION.txt` jis mein tag + commit ho) shamil karni hai ya nahi — ye WP10 ka faisla hai, abhi sirf note hai.

### F7 — MINOR — Do alag checksum files, koi combined SHA256SUMS nahi

- **Jagah:** `.github/workflows/release.yml`, lines 41, 71–73, 97–100.
- **Masla:** Release mein checksums hain (PASS) — lekin do mukhtalif files hain: firmware ke liye `SHA256SUMS.txt` (sirf bins) aur app ke liye `SHA256SUMS-app.txt` (sirf zip). Release ki tamam files ka ek combined checksum nahi hai; user ko do files check karni parti hain aur naam bhi ghair-mutabiq hain.
- **Fix:** Release job mein artifacts download karne ke baad ek combined `SHA256SUMS` banao (tamaam release files ke hashes) aur use bhi upload karo; purani do files ko compatibility ke liye rehne do ya hatao — faisla WP10 ka hai.

### F8 — MINOR — Koi concurrency group nahi — do tag pushes race kar sakti hain

- **Jagah:** `.github/workflows/release.yml` — poori file mein `concurrency:` nahi hai (CI mein hai: `ci-<ref>`, cancel-in-progress).
- **Masla:** Aik hi tag par do dafa dispatch / tag push + dispatch aik saath chale to dono `gh release create "$TAG"` karein gi — aik kamyab, dosri "already exists" par fail.
- **Fix:** File ke top par shamil karo:
  ```yaml
  concurrency:
    group: release-${{ github.ref }}
    cancel-in-progress: false
  ```
  (`cancel-in-progress: false` — release ko beech mein cancel karna khatarnak hai.)

### F9 — MINOR — PlatformIO version unpinned; platform pin verify nahi hota

- **Jagah:** `.github/workflows/release.yml`, lines 35 (`pip install -U platformio`) aur firmware job (koi pin-check step nahi).
- **Masla:** `pip install -U platformio` hamesha latest PlatformIO lata hai — kal ko major version badla to firmware build badal sakta hai. Aur CI ke bar-aks yahan `platform = espressif32@6.9.0` ka grep-check nahi hai — agar kisi ne `platformio.ini` mein pin badal di to release build khamoshi se mukhtalif platform par banegi.
- **Fix:** `pip install -U "platformio==<pinned version>"` (version CI/team se confirm kar ke) aur CI wala step copy karo:
  ```yaml
  - name: Verify platform pin (espressif32@6.9.0)
    run: grep -Fq 'platform = espressif32@6.9.0' firmware/rsh/platformio.ini
  ```

### F10 — MINOR — Actions SHA-pinned nahi hain

- **Jagah:** `.github/workflows/release.yml`, lines 24, 25, 28, 42, 53, 54, 75, 87 (`actions/checkout@v4`, `actions/setup-python@v5`, `actions/cache@v4`, `actions/upload-artifact@v4`, `actions/setup-dotnet@v4`, `actions/download-artifact@v4`).
- **Masla:** Sab major-version tags hain, SHA-pinned nahi — supply-chain best practice ke mutabiq tag hijack hone par workflow mutasir ho sakta hai. CI workflow bhi yehi pattern use karta hai, is liye ye release-only masla nahi — poore repo ka faisla hai.
- **Fix:** Agar repo policy SHA-pinning apnati hai to dono workflows mein SHA pin karo; warna is finding ko "accepted risk" ke tor par band karo. Akela release workflow mat badlo.

### F11 — MINOR — Tag aur code version ka koi match check nahi; dispatch default stale hai

- **Jagah:** `.github/workflows/release.yml`, line 14 (default `"v1.2.2"`) aur poori file (koi version check nahi).
- **Masla:** Code mein app `<Version>1.2.2</Version>` (`RSH.App.csproj`) aur firmware `FIRMWARE_VERSION "3.3"` (`config.h`) hai. Workflow ye check nahi karta ke tag in se match karta hai ya nahi — masalan tag `v2.0.0` (D27 ke mutabiq agla app version) push karne par bhi build mein `1.2.2` wali assembly banegi aur release `v2.0.0` ke naam se ship hogi. Dispatch default `"v1.2.2"` D27 ki agli release (`v2.0.0`) ke lihaz se stale hai.
- **Fix:** Software job mein check shamil karo ke tag `v<csproj Version>` se match karta hai (tag push par; dispatch par input se), aur firmware job mein ke merged bin ka version (`v3.3` — ya D27 ke baad `v4.0.0`) tag se mutabiq hai. Dispatch default ko agli release ke saath update karo (`v2.0.0` jab version bump ho).

## 4. PASS ki gayi cheezein (dobara check karne ki zaroorat nahi)

- YAML syntax valid hai (PyYAML 6.0.1).
- Trigger `v*` tag push + dispatch — maujood aur sahi shakal mein hai (D27 mismatch F2 mein hai, trigger mechanism mein nahi).
- Software job `windows-latest` par hai — WPF (`net10.0-windows`, `UseWPF`) ki wajah se lazmi hai; ubuntu par test execute nahi hota.
- Firmware job `ubuntu-latest` par hai — theek hai.
- Sab referenced paths maujood hain: `software/RSH.slnx`, `software/src/RSH.App/RSH.App.csproj`, `software/global.json` (10.0.100), `firmware/rsh/platformio.ini` (espressif32@6.9.0), `firmware/rsh/merge_bin.py`, `firmware/rsh/config.h` (v3.3).
- `dotnet test` fail hone par job ruk jati hai (koi `continue-on-error` nahi).
- Zip paths consistent hain (`software/publish/RSH/*` → `RSH-windows-x64.zip`); naam background se match karta hai.
- Release mein SHA256SUMS shamil hain (firmware + app, dono).
- Release job `needs: [firmware, software]` — koi bhi build fail ho to release nahi banti.
- `permissions: contents: write` — release ke liye zaroori aur maujood hai.
- Workflow mein purana product naam kahin nahi hai — product naam RSH hai.
- VJoy tests driver ke baghair chalte hain — windows runner par driver install ki zaroorat nahi.

## 5. "Pehli green Actions run" ki checklist (GitHub par run se pehle)

Push allow hone ke baad, pehli run se pehle ye sab hona chahiye — tartib se:

1. **Blockers fix karo:** F1 (merged-bin check) aur F2 (tag pattern vs D27 faisla) — in ke baghair pehli run ka koi matlab nahi.
2. **Majors fix karo:** F3 (SDK pin `10.0.100` + verify step) aur F4 (dispatch `--target`/checkout + re-run policy).
3. **Minors ka faisla karo:** F5–F11 — fix karo ya "accepted" ke tor par document karo (F10 poore repo ka faisla hai — akela release workflow mat badlo).
4. **D27 version faisla confirm karo:** Agli release app `v2.0.0` hai ya `v1.2.2`? Firmware `fw-v4.0.0` hai ya `v3.3`? Jab tak csproj `<Version>`, `FIRMWARE_VERSION` aur tag aik jese nahi hote, F11 ka check likha nahi ja sakta.
5. **Workflow ko `actionlint` se lint karo** (local par maujood nahi tha) — `actionlint .github/workflows/release.yml` clean hona chahiye.
6. **Release workflow ko pehle Virtual Test Rig mein chalao** (project rule: HAR test rig mein hoga) — rig tayyar hone par: firmware job + software job (Windows runner/VM) + release job ka dry-run (asal `gh release create` ke baghair — masalan `--draft` ya sirf artifact check).
7. **Push ki ijazat lo:** Local-only rule ki wajah se `release.yml` abhi GitHub par nahi hai (purana push commit 274e7acc mein ye file nahi gayi — token mein `workflow` scope nahi tha). Push se pehle token scope (`workflow`) aur branch protection check karo.
8. **Pehla tag test-release se lagao:** Asal release se pehle koi test tag (masalan `v0.0.0-test`) nahi — tag push hi release banata hai, is liye pehla asal tag hi test hai. Is liye pehle `workflow_dispatch` se dry-run karo (F4 fix ke baad), phir asal tag `v2.0.0` (ya D27 ke mutabiq) push karo.
9. **Pehli run par check karo:** (a) merged bin `RSH-Firmware-DevKitV1-v*.bin` artifact mein hai; (b) `SHA256SUMS.txt` + `SHA256SUMS-app.txt` release mein hain aur hashes verify hote hain; (c) zip khulta hai aur `RSH.exe` chalti hai; (d) release ka tag sahi commit par hai; (e) SDK version log mein `10.0.100` hai.
10. **CI green hone ke baad hi release tag lagao:** `ci.yml` pehle green hona chahiye — release workflow CI ka replacement nahi hai (wo har push/PR par chalta hai, ye sirf tag par).

## 6. Follow-up fixes — F1–F4 (Testers/QA, 2026-11-12)

F1 (Blocker), F2 (Blocker), F3 (Major), F4 (Major) fix kar diye gaye hain
(`.github/workflows/release.yml` + `firmware/rsh/merge_bin.py` — sirf ye
do files chhedi gayi hain). `merge_bin.py` mein missing-input par ab khamoshi
se return ke bajaye `RuntimeError` raise hota hai (pio run fail hoga).

**Minors F5–F11 — koi fix NAHI hui.** F1–F4 ke fixes ka in minors se koi
overlap nahi hai (F3 ka verify step CI jaisa hai, F5 ka test-breakdown nahi;
F4 ka `--target`/checkout F8 ki concurrency ya F7 ke checksums ko nahi
chhedta). Baqi hain:

- **F5** — `dotnet test` combined step (per-project breakdown nahi)
- **F6** — Zip se pehle `RSH.exe` existence check nahi; zip mein version file nahi
- **F7** — Do alag checksum files, koi combined SHA256SUMS nahi
- **F8** — Koi `concurrency:` group nahi (race ka khatra)
- **F9** — PlatformIO unpinned; platform pin verify nahi hota
- **F10** — Actions SHA-pinned nahi (poore repo ka faisla — akela release workflow mat badlo)
- **F11** — Tag/code version match check nahi; dispatch default `"v1.2.2"` stale — **VERSION FAISLA PARENT/USER KA HAI** (app v1.2.2 vs v2.0.0, firmware v3.3 vs fw-v4.0.0); jab tak faisla nahi hota, F11 ka check likha nahi ja sakta. Is follow-up ne version numbers nahi badle.

**Verification status: PENDING** — Virtual Test Rig abhi tayyar nahi hai;
runtime verification (workflow run, `pio run`) rig ka intezar karegi.
Static checks hue hain: YAML `yaml.safe_load` valid + Python `py_compile`
valid (§5 checklist ke items 1–2 ab "fix hue" hain; baqi items waise hi).
