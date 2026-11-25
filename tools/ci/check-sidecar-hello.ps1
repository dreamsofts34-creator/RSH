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
# v2.0.11 (P3): the stdio check never exercised the production
# TCP connect-back path — the v2.0.10 failure mode (sidecar dies
# between tcp.Connect and hello) passed it. The script now ALSO
# spawns the sidecar with --connect-back + RSH_HM_SIDECAR_TOKEN,
# accepts the loopback connection, and asserts a hello arrives
# over the socket with the token echoed. It then asserts the
# sidecar file log (%PROGRAMDATA%\RSH\logs\rsh-sidecar-*.log)
# exists with the 'hello sent' milestone (P0).
#
# update4-fix (P1-2 / P2-5): the stdio run now asserts exit code 0
# (it never checked $p.ExitCode), the TCP run's exit path is
# checked too (exit 0 after the socket closes), and the file-log
# check asserts 'context ready=' as well as 'hello sent' — scoped
# to TODAY's log file, deleted before the runs, with a fresh mtime,
# so a stale log can never pass.
#
# The daemon-side negative test — a sidecar that connects and dies
# before hello must report SidecarDied, never hang — lives in the
# Rust suite (manager::tests::tcp_connect_then_die_before_hello_is_sidecar_died),
# which runs on the Windows CI job too.
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
  [string]$Exe = "src-tauri/binaries/RshCoreHiDcr-x86_64-pc-windows-msvc.exe",
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

# P2-5: delete today's sidecar log BEFORE the runs, so the file-log
# check below can only pass on a log this run wrote — a stale log
# from an earlier day (or an earlier run) must never satisfy it.
$todayLog = Join-Path "$env:ProgramData\RSH\logs" ("rsh-sidecar-{0}.log" -f (Get-Date -Format yyyyMMdd))
Remove-Item $todayLog -Force -ErrorAction SilentlyContinue

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
# P1-2: assert the stdio run's exit code too — the check used to
# read the hello and never look at how the sidecar exited (stdin
# EOF is a clean exit, so 0 is expected; a crash or a fast-exit 2
# after a good hello must fail the check).
if ($p.ExitCode -ne 0) {
  Write-Error "Sidecar hello check: sidecar exited $($p.ExitCode) on the stdio run (expected 0)."
  exit 1
}
Write-Host "Sidecar hello OK (stdio): sdk $($hello.sdk), dll_sha256_ok $($hello.dll_sha256_ok), admin $($hello.admin), transport $($hello.transport), exit $($p.ExitCode)."

# --- TCP connect-back path (v2.0.11, P3) ---------------------------------
# The v2.0.10 failure mode happened ONLY on this path: the sidecar
# died between tcp.Connect and hello, and the stdio check above
# never exercised the connect-back at all (CI hello check ran the
# sidecar directly with stdin closed). So: listen on loopback,
# spawn the sidecar with --connect-back + RSH_HM_SIDECAR_TOKEN,
# accept, and assert a hello line arrives OVER THE SOCKET with the
# token echoed and the DLL verdict OK. Fail closed on timeout, on
# a non-hello first line, or on a bad verdict / wrong token echo.
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
$listener.Start()
$tcpPort = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
$token = -join ((1..32) | ForEach-Object { '{0:x}' -f (Get-Random -Maximum 16) })
$env:RSH_HM_SIDECAR_TOKEN = $token
$tcpErrFile = Join-Path $tmp "tcp.stderr.txt"
Remove-Item $tcpErrFile -Force -ErrorAction SilentlyContinue
$tcpClient = $null
$p2 = $null
try {
  $p2 = Start-Process -FilePath (Resolve-Path $Exe).Path `
    -ArgumentList "--connect-back", "127.0.0.1:$tcpPort" `
    -RedirectStandardError $tcpErrFile -PassThru -NoNewWindow
  $acceptTask = $listener.AcceptTcpClientAsync()
  if (-not $acceptTask.Wait(15000)) {
    $p2.Kill()
    Write-Error "Sidecar hello check (TCP): sidecar did not connect back to 127.0.0.1:$tcpPort within 15 s."
    exit 1
  }
  $tcpClient = $acceptTask.Result
  $reader = [System.IO.StreamReader]::new($tcpClient.GetStream())
  $readTask = $reader.ReadLineAsync()
  if (-not $readTask.Wait(15000)) {
    $p2.Kill()
    Write-Error "Sidecar hello check (TCP): no hello line arrived over the socket within 15 s (the v2.0.10 failure mode — sidecar died between connect and hello)."
    exit 1
  }
  $tcpLine = $readTask.Result
  if (-not $tcpLine) {
    $tcpErr = if (Test-Path $tcpErrFile) { Get-Content $tcpErrFile -Raw } else { "" }
    Write-Error "Sidecar hello check (TCP): socket closed before hello (EOF). Sidecar stderr: $tcpErr"
    exit 1
  }
  try {
    $tcpHello = $tcpLine | ConvertFrom-Json
  } catch {
    Write-Error "Sidecar hello check (TCP): first socket line is not JSON: $tcpLine"
    exit 1
  }
  if ($tcpHello.hello -ne "rsh-hm-sidecar" -and $tcpHello.sidecar -ne "rsh-hm-sidecar") {
    Write-Error "Sidecar hello check (TCP): first line is not a hello: $tcpLine"
    exit 1
  }
  if ($tcpHello.sdk -ne $ExpectedSdk) {
    Write-Error "Sidecar hello check (TCP): sdk is '$($tcpHello.sdk)', expected '$ExpectedSdk'."
    exit 1
  }
  if ($tcpHello.dll_sha256_ok -ne $true) {
    Write-Error "Sidecar hello check (TCP): dll_sha256_ok is '$($tcpHello.dll_sha256_ok)'."
    exit 1
  }
  if ($tcpHello.token -ne $token) {
    Write-Error "Sidecar hello check (TCP): hello did not echo the loopback token."
    exit 1
  }
  if ($tcpHello.transport -ne "tcp") {
    Write-Error "Sidecar hello check (TCP): transport is '$($tcpHello.transport)', expected 'tcp'."
    exit 1
  }
  Write-Host "Sidecar hello OK (TCP connect-back): sdk $($tcpHello.sdk), dll_sha256_ok $($tcpHello.dll_sha256_ok), admin $($tcpHello.admin), transport $($tcpHello.transport), pid $($tcpHello.pid)."
} finally {
  if ($null -ne $tcpClient) { $tcpClient.Close() }
  $listener.Stop()
  Remove-Item Env:\RSH_HM_SIDECAR_TOKEN -ErrorAction SilentlyContinue
}
# P1-2: the TCP run's exit path is checked too — closing the socket
# is the sidecar's EOF, so it must exit 0 promptly, like the stdio
# run. (The wait used to live in the finally above with no
# exit-code assertion at all.)
if ($null -ne $p2) {
  if (-not $p2.WaitForExit(10000)) {
    $p2.Kill()
    Write-Error "Sidecar hello check (TCP): sidecar did not exit within 10 s after the socket closed."
    exit 1
  }
  if ($p2.ExitCode -ne 0) {
    Write-Error "Sidecar hello check (TCP): sidecar exited $($p2.ExitCode) (expected 0)."
    exit 1
  }
}

# --- Sidecar file log (v2.0.11, P0; P1-2/P2-5 update4-fix) --------
# Both runs above must have written TODAY'S file log
# (%PROGRAMDATA%\RSH\logs\rsh-sidecar-YYYYMMDD.log) with the hello
# milestone AND the context milestone ("context ready=") — the
# context line proves the sidecar got past hello into context
# creation, which the old "hello sent"-only check did not. P2-5:
# today's log was deleted before the runs (above), so this file can
# only be from this run; the mtime is asserted fresh (last
# 10 minutes) as well, so a stale file can never pass.
$sidecarLog = Join-Path "$env:ProgramData\RSH\logs" ("rsh-sidecar-{0}.log" -f (Get-Date -Format yyyyMMdd))
if (-not (Test-Path $sidecarLog)) {
  Write-Error "Sidecar hello check: no sidecar file log for today at $sidecarLog (the sidecar must log to file as well as stderr)."
  exit 1
}
$logAge = (Get-Date) - (Get-Item $sidecarLog).LastWriteTime
if ($logAge.TotalMinutes -gt 10) {
  Write-Error "Sidecar hello check: today's sidecar file log is stale (last write $([math]::Round($logAge.TotalMinutes, 1)) min ago, expected < 10 min) — it is not from this run."
  exit 1
}
$logText = Get-Content $sidecarLog -Raw
if ($logText -notmatch "hello sent") {
  Write-Error "Sidecar hello check: sidecar file log exists but contains no 'hello sent' milestone."
  exit 1
}
if ($logText -notmatch "context ready=") {
  Write-Error "Sidecar hello check: sidecar file log exists but contains no 'context ready=' milestone (the sidecar never reached context creation)."
  exit 1
}
Write-Host "Sidecar file log OK: $sidecarLog, 'hello sent' and 'context ready=' present, fresh ($([math]::Round($logAge.TotalMinutes, 1)) min old)."
