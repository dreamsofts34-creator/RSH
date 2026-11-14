using System.Reflection;

namespace RSH.Core.Windows;

/// <summary>
/// v2.0.4 — window title mein version: <c>RSH — Real Sim Hub v2.0.4</c>.
/// Version hamesha assembly se (<see cref="AppVersionInfo.ForAssembly"/>),
/// kabhi hard-code nahi — taake user ki screenshot se hi pata chal jaye
/// ke kaunsi binary chali thi. Dono windows (Fluent + classic) yahi
/// istemal karte hain, taake title har jagah ek jaisa ho.
/// </summary>
public static class WindowTitle
{
    /// <summary>Display version se title banao; version na ho to bina version ke.</summary>
    public static string Format(string? displayVersion) =>
        string.IsNullOrWhiteSpace(displayVersion)
            ? "RSH — Real Sim Hub"
            : $"RSH — Real Sim Hub v{displayVersion}";

    /// <summary>Assembly se version le kar title banao.</summary>
    public static string ForAssembly(Assembly? assembly = null) =>
        Format(AppVersionInfo.ForAssembly(assembly).DisplayVersion);
}
