# tools/vigem — ViGEmBus vendor binaries (v2.0.19)

Two files the RSH installer ships beside the app. Both are official
Nefarius binaries; neither is built from source here.

## ViGEmClient.dll (x64, 130,048 B)

- SHA-256: `2bf0cb1d809039573c922737d298a1653d4dbc61408060ff45a9bcfde82e97d2`
- Source: the ViGEmClient SDK ships source only (no DLL asset in any
  release), so the DLL comes from the `vgamepad` 0.2.0 sdist on PyPI,
  which vendors an MSVC release build of the SDK
  (`vgamepad/win/vigem/client/x64/ViGEmClient.dll`). Its embedded PDB
  path (`...\ViGEmClient\bin\release\x64\ViGEmClient.pdb`) shows it
  was built from the official SDK source (by the vgamepad author, not
  by Nefarius) — it is not Nefarius's own signed binary.
- Verified 2026-11-28: PE32+ x64 DLL, 27 exports, all 8 symbols
  `rsh-output/src/vigem.rs` loads are present (`vigem_alloc`,
  `vigem_connect`, `vigem_disconnect`, `vigem_free`,
  `vigem_target_x360_alloc`, `vigem_target_add`,
  `vigem_target_x360_update`, `vigem_target_remove`,
  `vigem_target_free`).
- Installed by `installer/rsh.nsi` beside `rsh-daemon.exe` — the Xbox
  backend loads it from its own exe dir (`resolve_dll_path`).

## ViGEmBus_1.22.0_x64_x86_arm64.exe (6,278,576 B)

- SHA-256: `89220a7865076b342892f98865f3499fb7c4cfd673159e89d352c360fd014c6a`
- Source: official release `v1.22.0` ("It's dead, Jim", 2023-11-02),
  `nefarius/ViGEmBus` on GitHub (the `ViGEm/ViGEmBus` repo moved
  there). Final release of the retired project; signed by Nefarius.
- Installed by `installer/rsh.nsi` beside `rsh-daemon.exe` so the app
  can launch it interactively when the ViGEmBus driver is missing.
  There is NO documented silent-install switch for this setup — the
  app must launch it interactively and never claim a silent install.

## Re-fetching

- Setup EXE: `gh release download v1.22.0 --repo nefarius/ViGEmBus
  --pattern "ViGEmBus_1.22.0_x64_x86_arm64.exe"`
- Client DLL: `pip download vgamepad --no-deps`, extract the sdist,
  take `vgamepad/win/vigem/client/x64/ViGEmClient.dll`. Re-verify the
  SHA-256 above after any re-fetch.
