# rsh-hm-sidecar

The HIDMaestro backend helper for RSH. A C# NativeAOT sidecar that
owns the HIDMaestro lifecycle (context → driver install → custom
profile → controller → submit → dispose) and talks JSON-lines to
the Rust crate `crates/rsh-output-hidmaestro` over stdin/stdout
(dev) or a loopback TCP connect-back (production, when spawned
elevated via `runas`).

Rust never touches the HIDMaestro SDK. The sidecar is the only
process that loads `HIDMaestro.Core.dll`.

## Fetching the SDK (required to build)

The HIDMaestro DLL is **never committed** to the repo. Fetch
HIDMaestro **v2.1.0** (the latest release, pinned) and verify it:

```powershell
# release ZIP SHA-256 (pinned)
# 7fe349f163864b8dfc36117af260d51bac94a2263b6cdfa719cb364a808397b4
# DLL SHA-256 (pinned, checked again by the sidecar at startup)
# f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5
```

Place `HIDMaestro.Core.dll` at `tools/rsh-hm-sidecar/lib/` (the
default), or pass its path to the build:

```sh
dotnet build -p:HidMaestroDllPath=/path/to/HIDMaestro.Core.dll
```

The build fails fast with fetch instructions when the DLL is
missing.

## Build (sandbox / dev — plain .NET app, no AOT)

```sh
dotnet build -p:HidMaestroDllPath=/path/to/HIDMaestro.Core.dll
dotnet bin/Debug/net10.0-windows/RshCoreHiDcr.dll
```

`verify-linux.sh` does this end-to-end (build + scripted session
against the real DLL, asserting the derived 15-byte report size).

## Publish (Windows only — shipping form)

NativeAOT **cannot** build on Linux (ILCompiler: "Cross-OS native
compilation is not supported"). Publish on Windows (CI job / owner
PC):

```powershell
dotnet publish -c Release -r win-x64 -p:PublishAot=true `
  -p:HidMaestroDllPath=C:\path\to\HIDMaestro.Core.dll
```

That produces a single `RshCoreHiDcr.exe` (no .NET runtime
needed). A self-contained non-AOT publish is the fallback if AOT
ever breaks:

```powershell
dotnet publish -c Release -r win-x64 --self-contained true
```

## Run

```sh
# stdio (dev / when RSH itself runs elevated)
RshCoreHiDcr.exe
# TCP connect-back (production; RSH listens on loopback first).
# The token comes from the environment, never argv (C-TOK-1):
$env:RSH_HM_SIDECAR_TOKEN = "<128-bit hex>"
RshCoreHiDcr.exe --connect-back 127.0.0.1:54321
```

> Sandbox note: the TCP connect-back path is **sandbox-blocked**
> in the Linux dev sandbox (raw TCP from a .NET process is
> redirected to the egress proxy, which refuses it). It is
> exercised on Windows instead (gate H-HID-5). The stdio path is
> fully verified in the sandbox (`verify-linux.sh` + the Rust
> `real_sidecar_stdio_smoke` test).

The sidecar prints a `hello` line first (see `PROTOCOL.md`), then
answers one command per line. Log lines go to stderr only.

The sidecar's manifest requests `requireAdministrator`: launching
it directly prompts UAC. That is the headline UX cost of this
backend (R-1) — the elevated helper runs for the whole session, so
Windows asks on every RSH launch while HIDMaestro is the active
backend. vJoy needs no prompt. Do **not** work around it with a
scheduled task or a service that stores elevation.

## Profile / VID / PID

Custom profile `rsh-wheel-custom` (Type `wheel`, 5 axes, 32
buttons, hat). VID `0x1209` is the pid.codes open-source pool;
RSH is MIT-licensed, so it qualifies. PID `0x5253` = ASCII "RS",
verified absent from the pid.codes 1209 allocation list
(2026-11-14). It is **PROVISIONAL** until a pid.codes PR claiming
`0x5253` for "RSH — Real Sim Hub Wheel" is merged — that PR is
the release gate before v1.0.0. No brand is spoofed: no catalog
brand profile is ever created (the sidecar refuses any profile
id other than `rsh-wheel-custom`).

The input report size is **derived from the descriptor** (15
bytes), never the old 26-byte assumption (a 128-button profile
that was never built).

## Files

- `Program.cs` — the sidecar (lifecycle, commands, transports).
- `SidecarJson.cs` — reply DTOs + source-generated JSON context
  (AOT-safe; fixes the spike's IL3050/IL2026 warnings).
- `Constants.cs` — pinned SDK version / hashes / VID / PID.
- `HidReportSize.cs` — minimal HID descriptor parser deriving the
  input report size.
- `app.manifest` — `requireAdministrator`.
- `PROTOCOL.md` — the JSON-lines protocol.
- `verify-linux.sh` — sandbox build + scripted session checks.
