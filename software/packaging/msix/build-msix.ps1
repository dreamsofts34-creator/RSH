# RSH — MSIX packaging (D22, Phase H / SW-16)
# WINDOWS ONLY (MakeAppx + signtool Windows SDK se aate hain).
# Usage (Windows, repo root se):
#   powershell -ExecutionPolicy Bypass -File software/packaging/msix/build-msix.ps1
#   powershell -ExecutionPolicy Bypass -File software/packaging/msix/build-msix.ps1 -Sign -CertThumbprint <thumbprint>
# Bina -Sign ke package UNSIGNED banta hai (test install ke liye sideload
# karna hoga). USER DECISION 2026-11-13: OV certificate abhi NAHI khareeda
# jayega — v2.0.0 UNSIGNED ship hoga, is liye default (bina -Sign) hi
# release path hai. Cert khareedne ke baad -Sign se sign karo
# (docs/SIGNING.md §3.1: Publisher == cert Subject EXACTLY).
# Publisher check: manifest ka Publisher certificate Subject se EXACT match
# hona chahiye (SIGNING.md §3.1) — script sign karne se pehle check karta hai.
param(
    [switch]$Sign,
    [string]$CertThumbprint = "",
    [string]$Configuration = "Release"
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$PublishDir = Join-Path $RepoRoot "software\publish\RSH"
$LayoutDir  = Join-Path $RepoRoot "software\packaging\msix\layout"
$OutMsix    = Join-Path $RepoRoot "software\packaging\msix\RSH-2.0.3.msix"
$Manifest   = Join-Path $PSScriptRoot "Package.appxmanifest"

# 0. Manifest mein placeholder Publisher to nahi? (signing ke waqt lazmi check)
[xml]$xml = Get-Content $Manifest
$publisher = $xml.Package.Identity.Publisher
if ($publisher -like "*TODO*") {
    if ($Sign) { throw "Manifest Publisher abhi placeholder hai ($publisher) — pehle OV certificate Subject se badlo (docs/SIGNING.md §3.1)." }
    throw "Manifest Publisher abhi placeholder hai ($publisher) — pehle ise CN=RSH (unsigned identity) ya OV cert Subject se badlo."
}

# 1. Publish (framework-dependent loose win-x64) — release workflow
# wali command jaisi (v2.0.1 HOTFIX W2: self-contained hata diya — user ke PC
# par .NET pehle se hai; single-file NAHI — user faisla, halki RSH.exe +
# DLLs saath). MSIX install karne wale PC par .NET 10 DESKTOP
# Runtime chahiye hota hai (WPF usi mein hai, sirf base runtime nahi) —
# MSIX mein koi framework dependency declare nahi hoti, is liye ye
# requirement release notes / USER-GUIDE mein likhni hai (W3).
dotnet publish (Join-Path $RepoRoot "software\src\RSH.App\RSH.App.csproj") `
    -c $Configuration -r win-x64 --self-contained false -o $PublishDir
if (-not (Test-Path (Join-Path $PublishDir "RSH.exe"))) { throw "RSH.exe missing after publish." }

# 2. Layout: publish output + manifest + Assets (logos — rsh.ico se banao, abhi nahi hain to step fail hoga).
if (Test-Path $LayoutDir) { Remove-Item $LayoutDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $LayoutDir | Out-Null
Copy-Item "$PublishDir\*" $LayoutDir -Recurse
Copy-Item $Manifest (Join-Path $LayoutDir "AppxManifest.xml")
$assets = Join-Path $PSScriptRoot "Assets"
if (-not (Test-Path $assets)) { throw "Assets folder nahi hai (StoreLogo/Square150x150Logo/Square44x44Logo chahiye) — pehle rsh.ico se logos banao." }
Copy-Item $assets (Join-Path $LayoutDir "Assets") -Recurse

# 3. Pack (MakeAppx — Windows SDK).
makeappx pack /d $LayoutDir /p $OutMsix /o
Write-Host "MSIX packed: $OutMsix"

# 4. Sign (sirf -Sign par) + verify.
if ($Sign) {
    if ($CertThumbprint -eq "") { throw "-Sign ke liye -CertThumbprint lazmi hai (signtool /sha1)." }
    $cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Thumbprint -eq $CertThumbprint }
    if (-not $cert) { throw "Certificate thumbprint $CertThumbprint store mein nahi mila." }
    if ($cert.Subject -notlike "*$($publisher -replace '^CN=','')*") {
        throw "Publisher mismatch: manifest Publisher='$publisher', cert Subject='$($cert.Subject)'. SIGNING.md §3.1 dekho."
    }
    signtool sign /fd SHA256 /sha1 $CertThumbprint /tr http://timestamp.digicert.com /td SHA256 $OutMsix
    signtool verify /pa /v $OutMsix
    Write-Host "MSIX signed + verified."
} else {
    Write-Warning "UNSIGNED package — v2.0.0 unsigned ship hota hai (user faisla 2026-11-13, OV cert baad mein); SmartScreen warning expected hai (USER-GUIDE §3)."
}
