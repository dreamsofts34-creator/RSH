using System.Runtime.InteropServices;
using Microsoft.Win32;
using RSH.Core.Security;
using RSH.Core.Windows;

namespace RSH.App.Services;

/// <summary>Windows ke sath chhoti chhoti integrations: autostart + timer resolution.</summary>
public static class WindowsIntegration
{
    private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string RunValueName = "RSH";

    /// <summary>Registry par mabni autostart store (A5 — testable AutostartService ke peeche).</summary>
    private sealed class RegistryAutostartStore : IAutostartStore
    {
        public string? ReadValue()
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: false);
            return key?.GetValue(RunValueName) as string;
        }

        public void WriteValue(string value)
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: true)
                            ?? Registry.CurrentUser.CreateSubKey(RunKeyPath)
                            ?? throw new InvalidOperationException("Run registry key khol/bana nahi sakte.");
            key.SetValue(RunValueName, value);
        }

        public void DeleteValue()
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: true);
            key?.DeleteValue(RunValueName, throwOnMissingValue: false);
        }
    }

    private static AutostartService Autostart => new(new RegistryAutostartStore(),
        () => Environment.ProcessPath ?? "");

    public static bool GetAutostart() => Autostart.GetAutostart();

    /// <summary>
    /// Autostart lagao/hatao — A5: kamyabi sirf tab jab registry write ke
    /// baad read-back match kare. <c>false</c> = UI ON nahi dikhaye, error
    /// dikhaye. Khaali <c>catch {}</c> nahi — failure wajah ke saath wapas.
    /// </summary>
    /// <param name="error">Failure ki wajah; kamyabi par <c>null</c>.</param>
    public static bool SetAutostart(bool enabled, out string? error) =>
        Autostart.TrySetAutostart(enabled, out error);

    /// <summary>Compatibility overload — error chahiye ho to <see cref="SetAutostart(bool, out string?)"/> istemal karo.</summary>
    public static bool SetAutostart(bool enabled) => SetAutostart(enabled, out _);

    // Engine ka 5 ms tick Windows ke default 15.6 ms timer par sust par
    // sakta hai — process ke liye 1 ms resolution mango (games bhi yahi
    // karte hain). Sirf app ke process par asar, system-wide nahi.
    [DllImport("winmm.dll", EntryPoint = "timeBeginPeriod")]
    private static extern uint TimeBeginPeriod(uint periodMs);

    [DllImport("winmm.dll", EntryPoint = "timeEndPeriod")]
    private static extern uint TimeEndPeriod(uint periodMs);

    public static void BeginHighResolutionTimer() { try { TimeBeginPeriod(1); } catch { } }
    public static void EndHighResolutionTimer() { try { TimeEndPeriod(1); } catch { } }

    // --- Per-game auto-switch: sab se samne wali window ka process naam ---
    [DllImport("user32.dll")]
    private static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    /// <summary>Foreground app ka process naam (bina .exe ke); na mile to null.</summary>
    public static string? ForegroundProcessName()
    {
        try
        {
            var hwnd = GetForegroundWindow();
            if (hwnd == IntPtr.Zero) return null;
            GetWindowThreadProcessId(hwnd, out uint pid);
            if (pid == 0) return null;
            using var proc = System.Diagnostics.Process.GetProcessById((int)pid);
            return proc.ProcessName;
        }
        catch { return null; }
    }

    // --- Auto-setup: wheel ke Wi-Fi se khud jorna (netsh profile) ---

    /// <summary>
    /// RSH network ka WLAN profile bana kar connect karta hai —
    /// user ko Windows ki Wi-Fi list se haath se nahi jorna parta.
    /// Background thread se chalana (process calls hain).
    /// C9: password wala XML ab <see cref="WlanProfileTempFile"/> mein
    /// jata hai — unique naam, owner-only permissions, secure delete
    /// (overwrite + delete, `using` se har raste par), aur crash par reh
    /// jane wali files agle connect par sweep hoti hain. Fixed naam
    /// `%TEMP%\rsh-wlan.xml` + best-effort delete khatam.
    /// </summary>
    public static (bool Ok, string Message) ConnectToWheelWifi(string password)
    {
        try
        {
            string ssid = "RSH";
            string xml = $"""
                <?xml version="1.0"?>
                <WLANProfile xmlns="http://www.microsoft.com/networking/WLAN/profile/v1">
                  <name>{ssid}</name>
                  <SSIDConfig><SSID><name>{ssid}</name></SSID></SSIDConfig>
                  <connectionType>ESS</connectionType>
                  <connectionMode>manual</connectionMode>
                  <MSM><security>
                    <authEncryption><authentication>WPA2PSK</authentication><encryption>AES</encryption><useOneX>false</useOneX></authEncryption>
                    <sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{System.Security.SecurityElement.Escape(password)}</keyMaterial></sharedKey>
                  </security></MSM>
                </WLANProfile>
                """;
            // Crash-safe: pichli crash ki reh jane wali profile files saaf karo.
            WlanProfileTempFile.DeleteStaleFiles();
            using var tmp = WlanProfileTempFile.Create(xml);
            var add = RunNetsh($"wlan add profile filename=\"{tmp.Path}\" user=current");
            // tmp yahin scope ke end par secure-delete hota hai (using) —
            // add ke baad bhi, aur return/exception ke har raste par.
            if (add.ExitCode != 0) return (false, "Wi-Fi profile could not be created.");
            var con = RunNetsh($"wlan connect name={ssid} ssid={ssid}");
            if (con.ExitCode != 0) return (false, "Connect command failed — is the wheel powered and in range?");
            return (true, "Connecting to RSH…");
        }
        catch (Exception ex) { return (false, ex.Message); }
    }

    private static (int ExitCode, string Output) RunNetsh(string args)
    {
        using var p = System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo
        {
            FileName = "netsh",
            Arguments = args,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
        })!;
        string output = p.StandardOutput.ReadToEnd();
        p.WaitForExit(15000);
        return (p.ExitCode, output);
    }

    /// <summary>vJoyConf installed hai to khol do (device enable karne ke liye).</summary>
    public static bool OpenVJoyConfigure()
    {
        string exe = @"C:\Program Files\vJoy\x64\vJoyConf.exe";
        try
        {
            if (!System.IO.File.Exists(exe)) return false;
            System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(exe) { UseShellExecute = true });
            return true;
        }
        catch { return false; }
    }

    /// <summary>vJoy ki official download jagah browser mein khol do.</summary>
    public static void OpenVJoyDownloadPage()
    {
        try
        {
            System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(
                "https://github.com/BrunnerInnovation/vJoy/releases") { UseShellExecute = true });
        }
        catch { /* browser na khule to wizard ki hidayat kaafi hai */ }
    }
}
