using System.Reflection;

namespace RSH.Core.Windows;

/// <summary>
/// App version — hamesha assembly se, kabhi hard-code nahi (v2.0.2 HOTFIX).
/// <para>
/// v2.0.1 ka blank-window incident dobara diagnose na ho sakne ki aik wajah
/// ye thi ke startup log mein version tha hi nahi — kaunsi binary chali thi,
/// ye tak pata nahi chal sakta tha. Aur single-instance mutex/pipe ka naam
/// version se azaad tha, is liye purani (blank) v2.0.0 instance nayi v2.0.1
/// instance ko rok leti thi. Dono ka hal: version assembly se lo.
/// </para>
/// <para>
/// <see cref="ForAssembly"/> ko RSH.App ki assembly do
/// (<c>typeof(App).Assembly</c>) — RSH.Core ki apni assembly ka version
/// (<c>Directory.Build.props</c> se) app version nahi hota.
/// </para>
/// </summary>
public sealed record AppVersionInfo(
    Version AssemblyVersion,
    string? InformationalVersion,
    string DisplayVersion,
    string MutexSuffix)
{
    /// <summary>
    /// Assembly se version nikalo. <paramref name="assembly"/> null ho to
    /// entry assembly, wo bhi null ho to calling assembly.
    /// </summary>
    public static AppVersionInfo ForAssembly(Assembly? assembly = null)
    {
        assembly ??= Assembly.GetEntryAssembly() ?? Assembly.GetCallingAssembly();
        Version assemblyVersion = assembly.GetName().Version ?? new Version(0, 0, 0);
        string? informational = assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion;
        // Informational version mein commit hash hota hai ("2.0.1+abc123") —
        // display ke liye sirf version hissa, hash alag se log hota hai.
        string display = informational is not null
            ? informational.Split('+', 2)[0]
            : VersionSuffix(assemblyVersion);
        return new AppVersionInfo(assemblyVersion, informational, display, VersionSuffix(assemblyVersion));
    }

    /// <summary>
    /// Mutex/pipe suffix: sirf Major.Minor.Build ("2.0.2") — revision
    /// (4th hissa) hamesha 0 hota hai, is liye naam har rebuild par
    /// nahi badalta, sirf version bump par.
    /// </summary>
    public static string VersionSuffix(Version version) =>
        version.Build >= 0
            ? $"{version.Major}.{version.Minor}.{version.Build}"
            : $"{version.Major}.{version.Minor}";

    /// <summary>Versioned mutex naam, masalan <c>Global\RSH.SingleInstance.2.0.2</c>.</summary>
    public static string MutexNameForVersion(Version version) =>
        $@"Global\RSH.SingleInstance.{VersionSuffix(version)}";

    /// <summary>Versioned mutex naam — suffix pehle se string ho to.</summary>
    public static string MutexNameForVersion(string versionSuffix) =>
        $@"Global\RSH.SingleInstance.{versionSuffix}";

    /// <summary>Versioned pipe naam, masalan <c>RSH.SingleInstance.2.0.2</c>.</summary>
    public static string PipeNameForVersion(Version version) =>
        $"RSH.SingleInstance.{VersionSuffix(version)}";

    /// <summary>Versioned pipe naam — suffix pehle se string ho to.</summary>
    public static string PipeNameForVersion(string versionSuffix) =>
        $"RSH.SingleInstance.{versionSuffix}";

    /// <summary>Is assembly ke version ke mutex/pipe naam.</summary>
    public string MutexName => MutexNameForVersion(MutexSuffix);

    /// <summary>Is assembly ke version ke mutex/pipe naam.</summary>
    public string PipeName => PipeNameForVersion(MutexSuffix);
}
