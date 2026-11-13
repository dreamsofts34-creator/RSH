# RSH — MSIX packaging (D22, Phase H / SW-16)

**Version 2.0.2 — app v2.0.2 · firmware v4.0.0**

## Files

| File | Kaam |
|---|---|
| `Package.appxmanifest` | MSIX manifest (Identity, capabilities, entry point) |
| `build-msix.ps1` | Publish → layout → MakeAppx pack → signtool sign + verify (Windows only) |
| `verify-manifest.py` | Manifest draft check — kisi bhi OS par (`python3 verify-manifest.py`) |
| `Assets/` | `StoreLogo.png` (50×50) + `Square150x150Logo.png` (150×150) + `Square44x44Logo.png` (44×44) — `rsh.ico` (32×32) se banaye gaye. Caveat: 150×150 tile 32×32 source se upscale hai (soft lagega) — asal bara logo milne par replace karo. |

## Status (honest)

- [x] Manifest draft tayyar (Name RSH, Version 2.0.2.0, MinVersion Win 11
      10.0.22000.0, capabilities `internetClient` +
      `privateNetworkClientServer`, Executable `RSH.exe`).
- [ ] **v2.0.1 HOTFIX (W2, 2026-11-13): publish framework-dependent +
      LOOSE ho gaya hai** (single-file NAHI — user faisla, halki RSH.exe +
      DLLs saath) — MSIX ab Desktop Runtime par depend karta hai: install
      karne wale PC par **.NET 10 Desktop Runtime** chahiye
      (WPF usi mein hai, sirf base runtime nahi). MSIX mein koi framework
      dependency declare nahi hoti, is liye ye requirement release notes /
      USER-GUIDE mein likhni hai (W3). **Ho gaya (v2.0.2):** manifest `Version`
      `2.0.2.0` hai aur script output `RSH-2.0.2.msix` hai. `build-msix.ps1` ki publish command
      framework-dependent loose par update kar di gayi hai.
- [x] `verify-manifest.py` pass (Publisher `CN=RSH` — unsigned identity,
      dekho neeche).
- [x] **Publisher set hai** (`CN=RSH`, PublisherDisplayName `RSH`) —
      **USER DECISION 2026-11-13:** OV certificate abhi NAHI khareeda
      jayega, v2.0.0 **UNSIGNED** ship hoga; `CN=RSH` unsigned identity
      hai. Cert milne par Publisher ko exact cert Subject se badlo
      (docs/SIGNING.md §3.1) + `-Sign` build karo.
- [x] **Assets (logos) ban gaye** — `rsh.ico` (32×32) se 3 PNG (upscale
      caveat upar dekho).
- [ ] **Windows par build + install test nahi hua** — MakeAppx/signtool
      Windows SDK ke hain; sandbox (Linux) mein sirf manifest verify ho
      saka. Pehla asal (unsigned) MSIX Windows PC par banna + clean PC par
      install test hona baaqi hai (release checklist item). Unsigned MSIX
      install par SmartScreen warning aayega — USER-GUIDE §3 dekho.
- [ ] **v2.0.1 note (framework-dependent):** v2.0.1 zip ab .NET bundle
      nahi karta — app ko **.NET 10 Desktop Runtime 10.x** alag se
      chahiye (docs/RELEASE-NOTES-v2.0.1.md). Jab MSIX banaya jaye to
      manifest mein Desktop Runtime dependency declare karni hogi
      (PackageDependency `Microsoft.WindowsDesktop.App` 10.x), warna
      MSIX install hone ke baad bhi app runtime ke baghair nahi
      chalegi. Ye packaging (W2) ka kaam hai — is docs workstream ne
      sirf note likha hai, dependency abhi declare nahi hui.
- [ ] **Signing step release workflow mein nahi hai** (SEC-H-03, W1 ka kaam)
      — workflow abhi unsigned zip + **signed firmware** ship karta hai.
      App signing **DEFERRED hai (cert khareedne ke baad)** — user faisla
      2026-11-13, blocker nahi. Firmware signing (Ed25519) is se mutaliq
      nahi — wo waisa hi hai.

## Build (Windows)

```powershell
# Unsigned v2.0.0 package (default — user faisla 2026-11-13, OV cert baad mein):
powershell -ExecutionPolicy Bypass -File software/packaging/msix/build-msix.ps1
# Signed package (sirf OV cert khareedne ke BAAD — Publisher pehle cert Subject se badlo):
powershell -ExecutionPolicy Bypass -File software/packaging/msix/build-msix.ps1 -Sign -CertThumbprint <thumbprint>
```

## Firewall rule (A11)

MSIX install par koi install hook nahi hota — is liye firewall rule
installer nahi banata. App pehli run par rule banati hai
(`RSH.Core.Windows.FirewallRuleManager`, program-scoped
`New-NetFirewallRule`, UDP inbound, Private profile). Uninstall par rule
reh jati hai (MSIX uninstall hook nahi deta) — hatane ke liye
`software/packaging/firewall/uninstall-firewall-rule.ps1` hai, aur
Settings mein "Remove firewall rule" ka rasta release checklist mein
PENDING item hai. Detail: `docs/RELEASE-CHECKLIST.md`.
