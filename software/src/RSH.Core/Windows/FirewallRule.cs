// RSH — Windows Firewall rule, program-scoped (A11, Phase E/H).
// Plan v2.3 §6 + SW-10: firewall rule New-NetFirewallRule se banti hai,
// PROGRAM-scoped (RSH.exe par), install par banti hai, uninstall par
// hat-ti hai. netsh kabhi nahi.
//
// Kyun program-scoped, port-scoped nahi: PC app wheel se UDP stream
// EPHEMERAL local port par sunti hai (T10 — fixed listen port nahi);
// board ka port 4212 REMOTE (destination) hai, local nahi. Is liye rule
// mein -LocalPort 4212 likhna GHALAT hota — rule sirf -Program + UDP
// inbound + Private profile par banti hai. (Yehi ghalti pehle se bachi
// hui thi: NtpPing.cs mein likha hai ke ping ke liye "naya firewall
// rule nahi" chahiye — wahi port reuse hota hai.)
//
// Design: Build* methods pure hain (string banate hain — Linux par bhi
// test ho sakte hain). Asal chalana sirf Windows par hota hai
// (FirewallRuleManager — powershell.exe se New-/Remove-NetFirewallRule;
// admin chahiye, warna PowerShell error deta hai aur result fail hota
// hai, app crash nahi karti). MSIX install par rule installer nahi
// banata (MSIX mein arbitrary install hook nahi hota) — app pehli run
// par EnsureRule() chalati hai, aur uninstall par rule is liye reh
// jati hai ke MSIX uninstall hook nahi deta; is liye Settings mein
// "Remove firewall rule" + uninstall-firewall-rule.ps1 script maujood
// hai (docs/RELEASE-CHECKLIST.md mein PENDING item ke tor par darj hai).
using System.Diagnostics;

namespace RSH.Core.Windows;

/// <summary>Firewall rule operation ka nateeja.</summary>
public sealed record FirewallRuleResult(bool Ok, string Message);

/// <summary>
/// Program-scoped RSH firewall rule ke PowerShell commands (pure — testable).
/// Rule naam: "RSH" (DisplayName). Doosri apps ki rules ko kabhi nahi chherta.
/// </summary>
public static class FirewallRule
{
    /// <summary>Rule ka DisplayName — ensure/remove dono mein yahi istemal hota hai.</summary>
    public const string DisplayName = "RSH";

    /// <summary>
    /// Ensure (create-or-update) command banao. exePath: RSH.exe ka FULL
    /// Windows path hona chahiye (masalan C:\Program Files\RSH\RSH.exe —
    /// drive letter + backslash, relative nahi, .exe hona chahiye) —
    /// warna ArgumentException. Ye check jaan-boojh kar <see cref="Path"/>
    /// se nahi hota: Linux par <c>Path.IsPathFullyQualified</c> Windows
    /// path ko qualified nahi manta, aur ye builder Linux par bhi test
    /// hona chahiye (pure function).
    /// -Direction Inbound, -Protocol UDP, -Profile Private, -Action Allow.
    /// -LocalPort JAAN-BOOJH kar nahi hai (upar wajah dekho).
    /// </summary>
    public static string BuildEnsureCommand(string exePath)
    {
        if (string.IsNullOrWhiteSpace(exePath))
            throw new ArgumentException("RSH.exe ka full path chahiye.", nameof(exePath));
        bool isFullWindowsExePath = exePath.Length >= 4
            && char.IsAsciiLetter(exePath[0])
            && exePath[1] == ':'
            && exePath[2] == '\\'
            && exePath.EndsWith(".exe", StringComparison.OrdinalIgnoreCase);
        if (!isFullWindowsExePath)
            throw new ArgumentException("exePath full Windows .exe path hona chahiye (masalan C:\\Program Files\\RSH\\RSH.exe).", nameof(exePath));
        string quoted = "'" + exePath.Replace("'", "''") + "'";
        return $"New-NetFirewallRule -DisplayName '{DisplayName}' -Direction Inbound " +
               $"-Program {quoted} -Protocol UDP -Action Allow -Profile Private " +
               "-Description 'RSH wheel (ESP32) se UDP data receive karne ke liye' -ErrorAction Stop";
    }

    /// <summary>Remove command banao — sirf "RSH" naam wali rule hat-ti hai.</summary>
    public static string BuildRemoveCommand() =>
        $"Remove-NetFirewallRule -DisplayName '{DisplayName}' -ErrorAction SilentlyContinue";
}

/// <summary>
/// Firewall rule ko asal mein chalana (sirf Windows; admin chahiye).
/// Non-Windows par PlatformNotSupportedException — caller pehle
/// OperatingSystem.IsWindows() check kare.
/// </summary>
public static class FirewallRuleManager
{
    /// <summary>Rule banao (agar pehle se hai to PowerShell dobara banane par error de sakta hai — wo bhi result mein aata hai).</summary>
    public static Task<FirewallRuleResult> EnsureRuleAsync(string exePath, CancellationToken ct = default)
    {
        if (!OperatingSystem.IsWindows())
            throw new PlatformNotSupportedException("Firewall rule sirf Windows par banti hai.");
        return RunPowerShellAsync(FirewallRule.BuildEnsureCommand(exePath), ct);
    }

    /// <summary>"RSH" rule hatao (na ho to bhi Ok — SilentlyContinue).</summary>
    public static Task<FirewallRuleResult> RemoveRuleAsync(CancellationToken ct = default)
    {
        if (!OperatingSystem.IsWindows())
            throw new PlatformNotSupportedException("Firewall rule sirf Windows par hat-ti hai.");
        return RunPowerShellAsync(FirewallRule.BuildRemoveCommand(), ct);
    }

    private static async Task<FirewallRuleResult> RunPowerShellAsync(string command, CancellationToken ct)
    {
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "powershell.exe",
                Arguments = "-NoProfile -NonInteractive -Command " + "\"" + command.Replace("\"", "`\"") + "\"",
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true,
            };
            using Process? proc = Process.Start(psi);
            if (proc is null) return new FirewallRuleResult(false, "PowerShell start nahi hua.");
            string stderr = await proc.StandardError.ReadToEndAsync(ct).ConfigureAwait(false);
            await proc.WaitForExitAsync(ct).ConfigureAwait(false);
            return proc.ExitCode == 0
                ? new FirewallRuleResult(true, "Firewall rule OK.")
                : new FirewallRuleResult(false, $"Firewall rule fail hui (exit {proc.ExitCode}): {stderr.Trim()} — admin ke tor par chalao.");
        }
        catch (Exception ex) when (ex is InvalidOperationException or System.ComponentModel.Win32Exception or OperationCanceledException)
        {
            return new FirewallRuleResult(false, $"Firewall rule nahi chali: {ex.Message}");
        }
    }
}
