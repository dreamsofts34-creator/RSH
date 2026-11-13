<#
 RSH / RSH performance baseline — run on the Windows PC where the app is tested.
 Usage (PowerShell, from this folder):
   .\Measure-RSH.ps1 -ExePath "C:\path\to\RSH.exe" -IdleSeconds 120
 It starts the app, records time-to-window, then samples CPU % and memory every second
 while the window is open and idle, and writes perf-baseline-<timestamp>.txt.
 (Not yet tested by the author - no Windows machine available. If it errors, send me the message.)
#>
param(
  [Parameter(Mandatory=$true)][string]$ExePath,
  [int]$IdleSeconds = 120
)
$ErrorActionPreference = "Stop"
$cores = [Environment]::ProcessorCount
$t0 = Get-Date
$p = Start-Process -FilePath $ExePath -PassThru
# time until the main window exists
$sw = [Diagnostics.Stopwatch]::StartNew()
while ($sw.Elapsed.TotalSeconds -lt 60) {
  $p.Refresh()
  if ($p.MainWindowHandle -ne 0) { break }
  Start-Sleep -Milliseconds 50
}
$startupMs = [int]$sw.Elapsed.TotalMilliseconds
Write-Host "Window appeared after $startupMs ms. Leave the app idle (wheel connected if possible) - sampling $IdleSeconds s..."
Start-Sleep -Seconds 5   # settle
$cpu = New-Object System.Collections.Generic.List[double]
$ws  = New-Object System.Collections.Generic.List[double]
$prev = $p.TotalProcessorTime; $prevT = Get-Date
for ($i = 0; $i -lt $IdleSeconds; $i++) {
  Start-Sleep -Seconds 1
  $p.Refresh()
  if ($p.HasExited) { Write-Host "App exited during sampling."; break }
  $now = $p.TotalProcessorTime; $nowT = Get-Date
  $c = ($now - $prev).TotalSeconds / (($nowT - $prevT).TotalSeconds * $cores) * 100
  $cpu.Add([math]::Round($c,2)); $ws.Add([math]::Round($p.WorkingSet64/1MB,1))
  $prev = $now; $prevT = $nowT
}
$stats = {
  param($a) if ($a.Count -eq 0) { return "n/a" }
  $s = $a | Sort-Object; "avg {0:N2}  p95 {1:N2}  max {2:N2}" -f ($a | Measure-Object -Average).Average, $s[[int]([math]::Floor($s.Count*0.95))-1], $s[-1]
}
$threads = $p.Threads.Count
$report = @"
RSH perf baseline  $(Get-Date -Format s)
Machine: $env:COMPUTERNAME  cores=$cores  OS=$([Environment]::OSVersion.VersionString)
Exe: $ExePath
Startup to window (ms): $startupMs
Idle CPU % of total ($IdleSeconds s): $(& $stats $cpu)
Working set MB: $(& $stats $ws)
Threads: $threads
"@
$out = "perf-baseline-$(Get-Date -Format yyyyMMdd-HHmmss).txt"
$report | Tee-Object -FilePath $out
Write-Host "`nSaved $out - send this file back."
Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
