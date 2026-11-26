# Stage the HIDMaestro sidecar for Tauri `bundle.externalBin` (RSH deep-scan Round 2, M6).
#
# Shared by three Windows jobs/steps that used to carry three copies of this
# block (and drifted): CI `test-windows`, CI `tauri-build`, and the release
# workflow. Change the pins or the staging here, once — never inline again.
#
# What it does, in order (every step fail-closed):
#   1. Fetch the pinned HIDMaestro release ZIP (v2.1.0) and verify its SHA-256.
#   2. Extract it, find `HIDMaestro.Core.dll`, verify its SHA-256.
#   3. `dotnet publish` the sidecar (NativeAOT, win-x64) with the DLL path.
#   4. Copy the published exe to
#      `src-tauri/binaries/RshCoreHiDcr-x86_64-pc-windows-msvc.exe`
#      (the exact file `tauri-build`'s `copy_binaries` looks for: the
#      `bundle.externalBin` entry `binaries/RshCoreHiDcr` plus the
#      Windows target triple plus `.exe`).
#   5. Copy the verified `HIDMaestro.Core.dll` beside the staged exe
#      (v2.0.8 retest Bug 1, H1): the sidecar READS this DLL beside
#      itself at startup and SHA-256-hashes it (Program.cs
#      CheckDllHash); the installer ships it from here, and the
#      release smoke test fails closed when it is missing after
#      install. The DLL staged here is the SAME file hash-verified
#      in step 2 against the pin — never a different copy.
#
# Why the sidecar must be staged BEFORE any cargo build of `rsh-app`:
# the rsh-app build script (`tauri_build::build()`) copies the externalBin
# at build-script time and fails with `ResourcePathNotFound` when the file
# is missing (release dry runs #1/#2 + first CI run, 2026-11-14).
#
# Outputs (audit, v2.0.13, A P3-3): `zip_sha256` / `dll_sha256` — the
# MEASURED hashes, written to `$env:GITHUB_OUTPUT` when running on a
# GitHub runner. NOTE: no step currently consumes them (the M4
# SBOM-verification step they were written for does not exist —
# SBOMs are not published); they are recorded only. If an M4 step
# is added later it must compare against these measured values, so
# a re-typed constant can never drift from what was actually staged.
#
# Run from the repo root: `pwsh -NoProfile -ExecutionPolicy Bypass -File tools/ci/stage-sidecar.ps1`
# Requires: .NET 10 SDK on PATH (`dotnet`), PowerShell 7+.

[CmdletBinding()]
param(
  [string]$HidMaestroVersion = "2.1.0",
  [string]$ZipSha256 = "7fe349f163864b8dfc36117af260d51bac94a2263b6cdfa719cb364a808397b4",
  [string]$DllSha256 = "f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5"
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path "tools/rsh-hm-sidecar/rsh-hm-sidecar.csproj")) {
  Write-Error "Run this script from the repo root: tools/rsh-hm-sidecar/rsh-hm-sidecar.csproj not found."
  exit 1
}

$zipUrl = "https://github.com/hifihedgehog/HIDMaestro/releases/download/v$HidMaestroVersion/HIDMaestro-v$HidMaestroVersion.zip"
$zip = Join-Path $env:RUNNER_TEMP "HIDMaestro-v$HidMaestroVersion.zip"
# P3 (retry): a transient network failure used to fail the release
# job fail-closed with no retry. Retry the download up to 3 times
# with backoff; a persistent failure still fails closed below — a
# partial/corrupt download can never pass the SHA-256 check.
$downloaded = $false
for ($attempt = 1; $attempt -le 3; $attempt++) {
  try {
    Invoke-WebRequest -Uri $zipUrl -OutFile $zip
    $downloaded = $true
    break
  } catch {
    Write-Host "HIDMaestro download attempt $attempt/3 failed: $($_.Exception.Message)"
    if ($attempt -lt 3) { Start-Sleep -Seconds 10 }
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
  }
}
if (-not $downloaded) { Write-Error "HIDMaestro download failed after 3 attempts ($zipUrl) — transient network failure, retry the run."; exit 1 }
$zipHash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
if ($zipHash -ne $ZipSha256) { Write-Error "HIDMaestro ZIP SHA-256 mismatch: $zipHash"; exit 1 }
$dllDir = Join-Path $env:RUNNER_TEMP "hidmaestro"
Expand-Archive $zip -DestinationPath $dllDir -Force
$dll = Get-ChildItem -Recurse $dllDir -Filter "HIDMaestro.Core.dll" | Select-Object -First 1
if (-not $dll) { Write-Error "HIDMaestro.Core.dll not found in the release ZIP."; exit 1 }
$dllHash = (Get-FileHash $dll.FullName -Algorithm SHA256).Hash.ToLower()
if ($dllHash -ne $DllSha256) { Write-Error "HIDMaestro.Core.dll SHA-256 mismatch: $dllHash"; exit 1 }
dotnet publish tools/rsh-hm-sidecar/rsh-hm-sidecar.csproj -c Release -r win-x64 -p:PublishAot=true -p:HidMaestroDllPath=$($dll.FullName)
$exe = "tools/rsh-hm-sidecar/bin/Release/net10.0-windows/win-x64/publish/RshCoreHiDcr.exe"
if (-not (Test-Path $exe)) { Write-Error "Sidecar AOT publish produced no exe at $exe."; exit 1 }
New-Item -ItemType Directory -Force -Path "src-tauri/binaries" | Out-Null
Copy-Item $exe "src-tauri/binaries/RshCoreHiDcr-x86_64-pc-windows-msvc.exe"
# v2.0.8 retest Bug 1 (H1): the sidecar READS HIDMaestro.Core.dll
# beside itself at startup and hashes it (Program.cs CheckDllHash);
# the installer never shipped the DLL, so the hello verdict was
# always "no hash" and the HIDMaestro switch always failed. Stage
# the SAME verified DLL (hash-checked above against the pin) beside
# the staged exe so the NSIS installer can ship it.
Copy-Item $dll.FullName "src-tauri/binaries/HIDMaestro.Core.dll"
Write-Host "Staged sidecar + HIDMaestro.Core.dll (SHA-256 $dllHash) to src-tauri/binaries/"
Write-Host "Sidecar staged for Tauri externalBin."

if ($env:GITHUB_OUTPUT) {
  "zip_sha256=$zipHash" >> $env:GITHUB_OUTPUT
  "dll_sha256=$dllHash" >> $env:GITHUB_OUTPUT
}
