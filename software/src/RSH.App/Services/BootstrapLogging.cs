using System.IO;
using RSH.Core.Windows;
using Serilog;
using Serilog.Core;
using Serilog.Debugging;
using Serilog.Events;

namespace RSH.App.Services;

/// <summary>
/// v2.0.4 — bootstrap logger factory: dono logger-config jagah
/// (startup bootstrap + settings-level dobara config) ek hi logic,
/// aur logger EK dafa banta hai — dobara sirf level badalta hai
/// (<see cref="LoggingLevelSwitch"/>).
/// <list type="bullet">
/// <item>Primary <c>%AppData%\RSH\logs</c> likhne-layak na ho to fallback
/// <c>%TEMP%\RSH\logs</c> (<see cref="LogPathSelector"/> probe).</item>
/// <item>Serilog SelfLog ON — sink ki khamosh failure ab
/// <c>%TEMP%\RSH\serilog-selflog.txt</c> mein nazar aati hai (pehle
/// SelfLog off tha, is liye file-sink error kahin nahi aata tha).</item>
/// <item>Koi bhi failure startup nahi rokti: aakhri rasta sink-less
/// in-memory logger (events kahin nahi likhte, lekin app chalti hai).</item>
/// </list>
/// <para>
/// REPRO NOTE (2026-11-13, Serilog 4.4.0 + Sinks.File 7.0.0, App ka
/// exact pattern): bootstrap logger ko dispose kiye baghair replace
/// karne se KOI line nahi khoti — <c>shared: true</c> sink har event
/// foran disk par likhta hai (5 bootstrap lines replace se pehle hi
/// disk par thin; replace + 4 mazeed lines ke baad 9/9 maujood;
/// CloseAndFlush ke baad bhi same). Is liye single-config yahan
/// HYGIENE hai (aik khula handle, aik config jagah) — isay no-logs
/// ka root cause NA likho. Asal no-logs wajah abhi bhi khuli hai
/// (ghalat folder / ghalat filename / sink failure — SelfLog +
/// fallback + startup-status file isay agle run par pakrenge).
/// </para>
/// </summary>
public static class BootstrapLogging
{
    /// <summary>Primary log directory — <c>%AppData%\RSH\logs</c> (Roaming, AppServices jaisa hi).</summary>
    public static string PrimaryLogDirectory => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "RSH", "logs");

    /// <summary>Fallback log directory — <c>%TEMP%\RSH\logs</c>.</summary>
    public static string FallbackLogDirectory => StartupStatus.FallbackLogDirectory();

    /// <summary>Is waqt istemal ho rahi log directory (pehli ConfigureLogger call se).</summary>
    public static string CurrentLogDirectory => _currentLogDirectory ?? PrimaryLogDirectory;

    /// <summary>
    /// Aaj ki log file ki FULL PATH — Serilog rolling pattern
    /// <c>rsh-.log</c> se file <c>rsh-yyyyMMdd.log</c> banti hai
    /// (dashes NAHI — <c>rsh-20261111.log</c>, <c>rsh-2026-11-11.log</c>
    /// nahi). Startup par log karo taake user exact path copy-paste
    /// kar sake.
    /// </summary>
    public static string CurrentLogFilePath => LogFileNaming.FilePathForToday(CurrentLogDirectory);

    private static StreamWriter? _selfLogWriter;
    private static bool _selfLogEnabled;

    // Single-config state: logger EK dafa banta hai; dobara sirf level.
    private static readonly LoggingLevelSwitch _levelSwitch = new(LogEventLevel.Information);
    private static bool _configured;
    private static string? _currentLogDirectory;

    /// <summary>
    /// Serilog SelfLog on karo (file <c>%TEMP%\RSH\serilog-selflog.txt</c>,
    /// append). Aik dafa se zyada call safe hai. Kabhi throw nahi karta.
    /// </summary>
    public static void EnableSelfLog()
    {
        try
        {
            if (_selfLogEnabled)
                return;
            string path = StartupStatus.SelfLogFilePath();
            string? dir = Path.GetDirectoryName(path);
            if (!string.IsNullOrEmpty(dir))
                Directory.CreateDirectory(dir);
            _selfLogWriter = new StreamWriter(path, append: true) { AutoFlush = true };
            SelfLog.Enable(_selfLogWriter);
            _selfLogEnabled = true;
        }
        catch
        {
            // SelfLog best-effort hai — na khula to startup phir bhi chale.
        }
    }

    /// <summary>
    /// Minimum level badlo — logger replace NAHI hota (pehli
    /// <see cref="ConfigureLogger"/> call par bana logger hi rehta hai).
    /// Configure se pehle bhi call safe hai (level pehli config par lagta hai).
    /// </summary>
    public static void SetMinimumLevel(LogEventLevel level)
    {
        try
        {
            _levelSwitch.MinimumLevel = level;
        }
        catch
        {
            // Level switch kabhi startup na roke.
        }
    }

    /// <summary>
    /// <see cref="Log.Logger"/> banao aur set karo — sirf PEHLI call par.
    /// Dobara call ho to sirf level badalta hai, logger replace NAHI hota
    /// (is liye doosri call ke <paramref name="primaryDir"/> /
    /// <paramref name="fallbackDir"/> nazar-andaaz hote hain aur
    /// <paramref name="fallbackUsed"/> false rehta hai).
    /// Primary writable ho to wahi, warna fallback. <paramref name="fallbackUsed"/>
    /// batata hai ke fallback istemal hua (caller warning log kar sake).
    /// Wapas: asal mein istemal hui directory.
    /// </summary>
    public static string ConfigureLogger(LogEventLevel level, out bool fallbackUsed,
        string? primaryDir = null, string? fallbackDir = null)
    {
        EnableSelfLog();
        _levelSwitch.MinimumLevel = level;

        // Pehle se configured — sirf level badla (upar), logger wahi.
        if (_configured)
        {
            fallbackUsed = false;
            return _currentLogDirectory ?? PrimaryLogDirectory;
        }

        primaryDir ??= PrimaryLogDirectory;
        fallbackDir ??= FallbackLogDirectory;
        string chosen = LogPathSelector.ChooseLogDirectory(primaryDir, fallbackDir);
        fallbackUsed = !string.Equals(chosen, primaryDir, StringComparison.OrdinalIgnoreCase);

        try
        {
            Log.Logger = BuildLogger(chosen);
            _currentLogDirectory = chosen;
            _configured = true;
            return chosen;
        }
        catch
        {
            // Chuna hua dir phir bhi fail (race / lock) — fallback par dobara.
            if (!fallbackUsed)
            {
                try
                {
                    Log.Logger = BuildLogger(fallbackDir);
                    _currentLogDirectory = fallbackDir;
                    _configured = true;
                    fallbackUsed = true;
                    return fallbackDir;
                }
                catch
                {
                    // Neeche sink-less logger.
                }
            }
            try
            {
                Log.Logger = new LoggerConfiguration().MinimumLevel.ControlledBy(_levelSwitch).CreateLogger();
            }
            catch
            {
                // Aakhri rasta bhi fail — Log.Logger pehle wala rehta hai; startup na ruke.
            }
            _currentLogDirectory = chosen;
            _configured = true;
            return chosen;
        }
    }

    private static Serilog.Core.Logger BuildLogger(string directory) =>
        new LoggerConfiguration()
            .MinimumLevel.ControlledBy(_levelSwitch)
            // shared: true — Diagnostics zip export aaj ki log file ko
            // likhte hue bhi parh sake (v2.0.1 se yahi behaviour).
            .WriteTo.File(Path.Combine(directory, "rsh-.log"),
                rollingInterval: RollingInterval.Day, retainedFileCountLimit: 7, shared: true,
                outputTemplate: "{Timestamp:yyyy-MM-dd HH:mm:ss.fff} [{Level:u3}] {Message:lj}{NewLine}{Exception}")
            .CreateLogger();
}
