# RSH — install firewall rule (A11, Phase E/H)
# Windows only, ADMIN ke tor par chalao.
# Rule: program-scoped (RSH.exe par), UDP inbound, Private profile.
# Port-scoped nahi — PC app ephemeral local port par sunti hai (T10);
# board ka port 4212 REMOTE hai, is liye usay local-port flag mein
# likhna GHALAT hota.
# Usage: .\install-firewall-rule.ps1 [-ExePath "C:\Program Files\RSH\RSH.exe"]
param([string]$ExePath = "")
$ErrorActionPreference = "Stop"
if ($ExePath -eq "") {
    $ExePath = Join-Path $PSScriptRoot "..\..\publish\RSH\RSH.exe"
    if (-not (Test-Path $ExePath)) {
        $ExePath = (Get-Process RSH -ErrorAction SilentlyContinue | Select-Object -First 1).Path
    }
}
if (-not $ExePath -or -not (Test-Path $ExePath)) { throw "RSH.exe nahi mila. -ExePath se full path do." }
New-NetFirewallRule -DisplayName 'RSH' -Direction Inbound -Program $ExePath `
    -Protocol UDP -Action Allow -Profile Private `
    -Description 'RSH wheel (ESP32) se UDP data receive karne ke liye' -ErrorAction Stop
Write-Host "Firewall rule 'RSH' ban gayi: $ExePath"
