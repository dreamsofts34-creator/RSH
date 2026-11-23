# Check the staged AOT sidecar's hello (v2.0.8 retest Bug 1, H5).
#
# No CI test ever ran the sidecar, so a sidecar that could not say
# hello — the installer never shipped HIDMaestro.Core.dll, and the
# Rust side refuses a hello with no DLL-hash measurement — shipped
# in v2.0.8. This check runs the STAGED exe (DLL beside it, staged
# by stage-sidecar.ps1) with stdin closed, reads the first stdout
# line (hello JSON), and asserts the DLL-hash verdict is OK with
# the pinned SDK. Fail-closed on timeout, on a non-hello first
# line, or on a bad verdict.
#
# The sidecar is a WinExe (no console): stdout/stderr go to files
# via Start-Process redirection (no pipe-buffer deadlock), and
# stdin comes from an empty file so the sidecar sees EOF and exits
# right after hello. The sidecar carries a requireAdministrator
# manifest; on a GitHub runner the agent runs elevated, so no UAC
# prompt appears. If this check ever fails on elevation, the
# message says so — fix the runner, not the check.
#
# Run from the repo root AFTER stage-sidecar.ps1:
#   pwsh -NoProfile -ExecutionPolicy Bypass -File tools/ci/check-sidecar-hello.ps1
# Requires: PowerShell 7+ (Start-Process redirection), Windows.
[CmdletBinding()]
param(
  [string]$Exe = "src-tauri/binaries/rsh-hm-sidecar-x86_64-pc-windows-msvc.exe",
  [string]$ExpectedDllSha256 = "f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5",
  [string]$ExpectedSdk = "2.1.0",
  [int]$TimeoutSec = 60
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Exe)) { Write-Error "Sidecar hello check: staged exe not found at $Exe (run stage-sidecar.ps1 first)."; exit 1 }
$dll = Join-Path (Split-Path $Exe -Parent) "HIDMaestro.Core.dll"
if (-not (Test-Path $dll)) { Write-Error "Sidecar hello check: HIDMaestro.Core.dll not found beside the staged exe at $dll (stage-sidecar.ps1 must stage it)."; exit 1 }

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "rsh-sidecar-hello-check"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$emptyIn = Join-Path $tmp "empty.stdin"
$outFile = Join-Path $tmp "stdout.txt"
$errFile = Join-Path $tmp "stderr.txt"
New-Item -ItemType File -Force -Path $emptyIn | Out-Null
Remove-Item $outFile, $errFile -Force -ErrorAction SilentlyContinue

$p = Start-Process -FilePath (Resolve-Path $Exe).Path `
  -RedirectStandardInput $emptyIn -RedirectStandardOutput $outFile -RedirectStandardError $errFile `
  -PassThru -NoNewWindow
if (-not $p.WaitForExit($TimeoutSec * 1000)) {
  $p.Kill()
  Write-Error "Sidecar hello check TIMEOUT: no exit within $TimeoutSec s (hello never arrived — the sidecar may be waiting on elevation/UAC or a hung HMContext; on a runner the agent must run elevated)."
  exit 1
}
$stdout = if (Test-Path $outFile) { Get-Content $outFile -Raw } else { "" }
$stderr = if (Test-Path $errFile) { Get-Content $errFile -Raw } else { "" }
$first = ($stdout -split "`r?`n" | Where-Object { $_.Trim() -ne "" } | Select-Object -First 1)
if (-not $first) {
  Write-Error "Sidecar hello check: sidecar exited $($p.ExitCode) with no stdout. stderr: $stderr"
  exit 1
}
try {
  $hello = $first | ConvertFrom-Json
} catch {
  Write-Error "Sidecar hello check: first stdout line is not JSON: $first"
  exit 1
}
if ($hello.hello -ne "rsh-hm-sidecar" -and $hello.sidecar -ne "rsh-hm-sidecar") {
  Write-Error "Sidecar hello check: first line is not a hello: $first"
  exit 1
}
if ($hello.sdk -ne $ExpectedSdk) {
  Write-Error "Sidecar hello check: sdk is '$($hello.sdk)', expected '$ExpectedSdk'."
  exit 1
}
if ($hello.dll_sha256_ok -ne $true) {
  Write-Error "Sidecar hello check: dll_sha256_ok is '$($hello.dll_sha256_ok)' (dll_sha256='$($hello.dll_sha256)'). The DLL beside the staged exe did not verify — the HIDMaestro switch would fail."
  exit 1
}
if ("$($hello.dll_sha256)".ToLower() -ne $ExpectedDllSha256) {
  Write-Error "Sidecar hello check: dll_sha256 is '$($hello.dll_sha256)', expected '$ExpectedDllSha256'."
  exit 1
}
Write-Host "Sidecar hello OK: sdk $($hello.sdk), dll_sha256_ok $($hello.dll_sha256_ok), admin $($hello.admin), transport $($hello.transport), exit $($p.ExitCode)."
