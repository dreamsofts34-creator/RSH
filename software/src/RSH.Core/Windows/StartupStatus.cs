namespace RSH.Core.Windows;

/// <summary>
/// v2.0.4 — startup milestones ka SECOND channel: <c>%TEMP%\RSH\startup-status.txt</c>.
///
/// Background: v2.0.3 tak user ke PC par koi log file hi nahi mili
/// (<c>%AppData%\RSH\logs\</c> khaali) — is liye pata hi nahi chal saka ke
/// startup kahan tak pohancha tha. Ye helper Serilog se bilkul azaad hai
/// (koi sink, koi config nahi — seedha <c>File</c> likhta hai), taake
/// logger fail bhi ho to milestones phir bhi milen. Har run par file
/// fresh banti hai (<see cref="Reset"/>), har milestone timestamp ke
/// saath append hota hai (<see cref="Write"/>).
///
/// Best-effort: koi bhi method kabhi throw nahi karta — status file na
/// likh saki to startup phir bhi chalti hai (silent swallow, by design).
/// </summary>
public static class StartupStatus
{
    public const string StatusFileName = "startup-status.txt";
    public const string SelfLogFileName = "serilog-selflog.txt";

    private static readonly Lock _writeLock = new();

    /// <summary>Startup-status file ka path — default <c>%TEMP%\RSH\startup-status.txt</c>.</summary>
    public static string StatusFilePath(string? tempBase = null) =>
        Path.Combine(tempBase ?? Path.GetTempPath(), "RSH", StatusFileName);

    /// <summary>Serilog SelfLog file ka path — default <c>%TEMP%\RSH\serilog-selflog.txt</c>.</summary>
    public static string SelfLogFilePath(string? tempBase = null) =>
        Path.Combine(tempBase ?? Path.GetTempPath(), "RSH", SelfLogFileName);

    /// <summary>Log fallback directory — default <c>%TEMP%\RSH\logs</c>.</summary>
    public static string FallbackLogDirectory(string? tempBase = null) =>
        Path.Combine(tempBase ?? Path.GetTempPath(), "RSH", "logs");

    /// <summary>Milestone line ka format — <c>[yyyy-MM-dd HH:mm:ss.fff] milestone</c>.</summary>
    public static string FormatLine(DateTimeOffset timestamp, string milestone) =>
        $"[{timestamp:yyyy-MM-dd HH:mm:ss.fff}] {milestone}";

    /// <summary>
    /// Har run ke shuru mein file fresh karo aur pehli line likho:
    /// <c>started v{displayVersion}</c>. Purani run ki lines nahi rehtin —
    /// file hamesha isi run ki hoti hai.
    /// </summary>
    public static void Reset(string displayVersion, string? tempBase = null)
    {
        TryWrite(StatusFilePath(tempBase), FormatLine(DateTimeOffset.Now, $"started v{displayVersion}"), append: false);
    }

    /// <summary>Ek milestone append karo (timestamp ke saath). Kabhi throw nahi karta.</summary>
    public static void Write(string milestone, string? tempBase = null)
    {
        TryWrite(StatusFilePath(tempBase), FormatLine(DateTimeOffset.Now, milestone), append: true);
    }

    /// <summary>
    /// Swallowed exception ko status file mein AIK line mein likho:
    /// context + exception type + message + stack trace ki pehli line.
    /// <see cref="App"/> ka DispatcherUnhandledException handler exception
    /// ko nigal kar app chalata rehta hai (v2.0.1 resilience faisla) —
    /// bina is line ke woh exception diagnostics mein kabhi nazar nahi
    /// aati. Kabhi throw nahi karta.
    /// </summary>
    public static void WriteException(string context, Exception? ex, string? tempBase = null)
    {
        TryWrite(StatusFilePath(tempBase), FormatLine(DateTimeOffset.Now, FormatException(context, ex)), append: true);
    }

    /// <summary>Exception ko aik line mein format karo (newlines hata kar, lambi lines kaat kar).</summary>
    public static string FormatException(string context, Exception? ex)
    {
        if (ex is null)
            return $"{context}: (no exception)";
        string message = SingleLine(ex.Message, 300);
        string stackFirst = ex.StackTrace?
            .Split(['\r', '\n'], StringSplitOptions.RemoveEmptyEntries)
            .FirstOrDefault()?.Trim() ?? "(no stack trace)";
        return $"{context}: {ex.GetType().FullName}: {message} | {SingleLine(stackFirst, 300)}";
    }

    private static string SingleLine(string? text, int maxLength)
    {
        string line = (text ?? string.Empty).Replace('\r', ' ').Replace('\n', ' ');
        return line.Length > maxLength ? line[..maxLength] + "…" : line;
    }

    private static void TryWrite(string path, string line, bool append)
    {
        try
        {
            lock (_writeLock)
            {
                string? dir = Path.GetDirectoryName(path);
                if (!string.IsNullOrEmpty(dir))
                    Directory.CreateDirectory(dir);
                if (append)
                    File.AppendAllText(path, line + Environment.NewLine);
                else
                    File.WriteAllText(path, line + Environment.NewLine);
            }
        }
        catch
        {
            // Best-effort channel — likhna fail ho to startup kabhi na ruke.
        }
    }
}
