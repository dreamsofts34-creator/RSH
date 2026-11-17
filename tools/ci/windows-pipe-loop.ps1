# F18 (v2.0.3): Windows named-pipe loop regression test.
#
# The v2.0.2 restart loop was a WINDOWS-ONLY bug: the daemon's IPC
# loop created a new named-pipe instance on every iteration, so after
# the UI connected, every further create failed with "Access is
# denied (os error 5)" — 1199 times in the user's log — until 5
# consecutive failures killed the daemon and the UI respawned it.
# The Linux rig cannot reproduce a Windows named-pipe failure, so
# this is the execution proof: the REAL daemon runs on Windows, a
# named-pipe client connects and HOLDS the pipe for > 10 s (the
# failing sequence — the second create used to fail ~1 s after the
# first client connected), and the daemon must still be alive with a
# second client able to connect afterwards.
#
# Usage (from the repo root, on Windows):
#   pwsh -NoProfile -ExecutionPolicy Bypass -File tools/ci/windows-pipe-loop.ps1 `
#     -DaemonExe target/debug/rsh-daemon.exe -DataDir $env:TEMP\rsh-pipe-loop
#
# Exit code is 0 only when every assertion below holds; any failure
# throws (fail closed — a missing daemon, a dead daemon, a failed
# ping, a pipe error in the log, or a --check failure all fail the
# job instead of passing quietly).
param(
    [Parameter(Mandatory = $true)][string]$DaemonExe,
    [Parameter(Mandatory = $true)][string]$DataDir,
    [Parameter(Mandatory = $false)][int]$HoldSeconds = 12
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $DaemonExe)) {
    throw "daemon binary not found: $DaemonExe (run cargo build -p rsh-daemon first)"
}
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
$errLog = Join-Path $DataDir 'daemon-stderr.log'
$outLog = Join-Path $DataDir 'daemon-stdout.log'
Remove-Item $errLog, $outLog -ErrorAction SilentlyContinue

function Connect-PipeClient {
    # One named-pipe client to \\.\pipe\rsh-daemon (IPC_PIPE_NAME).
    # DS-05 follow-up (deep-scan FC-03/CI-03): do NOT set ReadTimeout /
    # WriteTimeout on this stream at all. The first DS-05 fix opened it
    # with PipeOptions::Asynchronous, on the theory that .NET accepts
    # timeouts on an async pipe — but CI run 44129826880 (head cfbd135,
    # which HAS the async flag) still threw "Exception setting
    # ReadTimeout: Timeouts are not supported on this stream" 0.2 s
    # after "daemon listening", before the first ping. In pwsh on the
    # runner the async flag does not make the timeouts settable, so the
    # assignments are deleted instead of re-attempted. The connect is
    # still bounded by Connect(5000); the reads are bounded by the job
    # timeout, so a hung daemon fails the job instead of hanging it.
    $stream = New-Object System.IO.Pipes.NamedPipeClientStream(
        '.', 'rsh-daemon',
        [System.IO.Pipes.PipeDirection]::InOut,
        [System.IO.Pipes.PipeOptions]::Asynchronous)
    $stream.Connect(5000)
    $writer = New-Object System.IO.StreamWriter(
        $stream, (New-Object System.Text.UTF8Encoding($false)))
    $writer.AutoFlush = $true
    $reader = New-Object System.IO.StreamReader(
        $stream, [System.Text.Encoding]::UTF8)
    return @{ Stream = $stream; Writer = $writer; Reader = $reader }
}

function Invoke-PipePing {
    param($Client, [int]$Id)
    $Client.Writer.WriteLine(
        "{`"v`":1,`"id`":$Id,`"method`":`"ping`",`"params`":{}}")
    $line = $Client.Reader.ReadLine()
    if ($null -eq $line) { return $false }
    return $line.Contains('"pong":true')
}

function Close-PipeClient {
    param($Client)
    try { $Client.Writer.Close() } catch { }
    try { $Client.Reader.Close() } catch { }
    try { $Client.Stream.Close() } catch { }
}

$daemon = Start-Process -FilePath $DaemonExe `
    -ArgumentList @('--data-dir', $DataDir, '--no-connect', '--no-tray', '--verbose') `
    -RedirectStandardOutput $outLog -RedirectStandardError $errLog `
    -PassThru -WindowStyle Hidden
try {
    # 1. The daemon must reach "IPC listening" (pipe bound) within 30 s.
    $deadline = (Get-Date).AddSeconds(30)
    $listening = $false
    while ((Get-Date) -lt $deadline) {
        if ($daemon.HasExited) {
            throw "daemon exited early with code $($daemon.ExitCode) before listening"
        }
        if ((Test-Path $errLog) -and
            (Select-String -Path $errLog -Pattern 'IPC listening' -Quiet)) {
            $listening = $true
            break
        }
        Start-Sleep -Milliseconds 500
    }
    if (-not $listening) {
        throw "daemon did not log 'IPC listening' within 30 s"
    }
    Write-Host "daemon listening (pid $($daemon.Id))"

    # 2. First client: connect + ping, then HOLD the pipe.
    $client1 = Connect-PipeClient
    try {
        if (-not (Invoke-PipePing -Client $client1 -Id 1)) {
            throw "first client ping failed"
        }
        Write-Host "first client ping OK — holding the pipe for $HoldSeconds s"
        Start-Sleep -Seconds $HoldSeconds
        if ($daemon.HasExited) {
            throw "daemon died while the first client held the pipe (exit $($daemon.ExitCode))"
        }

        # 3. Second client: connect + ping WHILE the first still holds
        # the pipe. Pre-fix this is where the loop died: the daemon's
        # next pipe create failed "Access is denied" and, after 5
        # failures, the daemon exited.
        $client2 = Connect-PipeClient
        try {
            if (-not (Invoke-PipePing -Client $client2 -Id 2)) {
                throw "second client ping failed while the first held the pipe"
            }
            Write-Host "second client ping OK while the first held the pipe"
        }
        finally { Close-PipeClient -Client $client2 }
    }
    finally { Close-PipeClient -Client $client1 }

    # 4. Independent second-client verification: --check connects as a
    # separate process and verifies IPC answers, exit 0.
    & $DaemonExe --check --data-dir $DataDir
    if ($LASTEXITCODE -ne 0) {
        throw "rsh-daemon --check failed with exit code $LASTEXITCODE"
    }
    Write-Host "--check OK"

    # 5. The daemon must still be alive, with no pipe errors in its log.
    if ($daemon.HasExited) {
        throw "daemon exited during the test (exit $($daemon.ExitCode))"
    }
    if (Select-String -Path $errLog -Pattern 'IPC pipe create failed' -Quiet) {
        throw "daemon logged 'IPC pipe create failed' — the pipe loop is back"
    }
    if (Select-String -Path $errLog -Pattern 'FATAL' -Quiet) {
        throw "daemon logged a FATAL during the test"
    }
    Write-Host "WINDOWS-PIPE-LOOP PASS: daemon (pid $($daemon.Id)) survived a ${HoldSeconds}s held pipe + second client, no pipe errors"
}
finally {
    if (-not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force -ErrorAction SilentlyContinue
    }
}
