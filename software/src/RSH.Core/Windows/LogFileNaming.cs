namespace RSH.Core.Windows;

/// <summary>
/// v2.0.4 — Serilog rolling log file ka naam: pattern <c>rsh-.log</c> +
/// <c>RollingInterval.Day</c> se file <c>rsh-yyyyMMdd.log</c> banti hai —
/// dashes NAHI hote (<c>rsh-20261111.log</c>, <c>rsh-2026-11-11.log</c>
/// nahi). User/parent dashed naam dhoondh rahe thay — is liye startup
/// par hamesha FULL PATH log karo (<see cref="BootstrapLogging"/>),
/// taake exact path copy-paste ho sake. Pure logic — Linux par testable.
/// </summary>
public static class LogFileNaming
{
    /// <summary>Aik din ki log file ka naam, masalan <c>rsh-20261111.log</c>.</summary>
    public static string FileName(DateTime date) => $"rsh-{date:yyyyMMdd}.log";

    /// <summary>Aik din ki log file ki full path.</summary>
    public static string FilePath(string directory, DateTime date) =>
        Path.Combine(directory, FileName(date));

    /// <summary>Aaj ki log file ki full path (local date).</summary>
    public static string FilePathForToday(string directory) => FilePath(directory, DateTime.Today);
}
