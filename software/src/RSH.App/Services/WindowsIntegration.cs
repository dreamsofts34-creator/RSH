using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace RSH.App.Services;

/// <summary>Windows ke sath chhoti chhoti integrations: autostart + timer resolution.</summary>
public static class WindowsIntegration
{
    private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string RunValueName = "RSH";

    public static bool GetAutostart()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: false);
            return key?.GetValue(RunValueName) is string;
        }
        catch { return false; }
    }

    public static void SetAutostart(bool enabled)
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: true)
                            ?? Registry.CurrentUser.CreateSubKey(RunKeyPath);
            if (enabled)
            {
                string exe = Environment.ProcessPath ?? "";
                if (exe.Length > 0) key.SetValue(RunValueName, $"\"{exe}\" --minimized");
            }
            else
            {
                key.DeleteValue(RunValueName, throwOnMissingValue: false);
            }
        }
        catch { /* registry na mile to app phir bhi chalta hai */ }
    }

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
            string tmp = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "rsh-wlan.xml");
            System.IO.File.WriteAllText(tmp, xml);
            var add = RunNetsh($"wlan add profile filename=\"{tmp}\" user=current");
            try { System.IO.File.Delete(tmp); } catch { /* temp safai best-effort */ }
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
