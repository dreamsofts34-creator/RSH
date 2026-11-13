# RSH — uninstall firewall rule (A11, Phase E/H)
# Windows only, ADMIN ke tor par chalao.
# MSIX uninstall par koi hook nahi hota, is liye rule khud hatani parti hai
# (ya app ki Settings se "Remove firewall rule").
# Sirf 'RSH' naam wali rule hat-ti hai — doosri apps ki rules ko nahi chherta.
$ErrorActionPreference = "Stop"
Remove-NetFirewallRule -DisplayName 'RSH' -ErrorAction SilentlyContinue
Write-Host "Firewall rule 'RSH' hata di gayi (agar thi to)."
