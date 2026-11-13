namespace RSH.Core.Pipeline;

/// <summary>
/// Plan ki validation fail hone par — <see cref="Errors"/> mein har
/// ghalti "Module: wajah" ki shakal mein hoti hai.
/// </summary>
public sealed class PipelinePlanValidationException : Exception
{
    public PipelinePlanValidationException(IReadOnlyList<string> errors)
        : base("PipelinePlan invalid: " + string.Join(" ", errors))
    {
        Errors = errors;
    }

    public IReadOnlyList<string> Errors { get; }
}

/// <summary>
/// Ek module ki plan entry: on/off + us ki tuning.
/// <b>Rule (v2.3 §4 / SW-6 P11): band module ka data pipeline se nahi
/// guzarta.</b> Is liye band entry ki tuning validation mein poori tarah
/// ignore hoti hai — band module ki ghalat tuning bhi plan ko reject
/// nahi karti (wo kabhi istemal hi nahi hogi); sirf enabled module ki
/// tuning validate hoti hai.
/// </summary>
public sealed record ModulePlanEntry
{
    public required ModuleKind Module { get; init; }
    public bool Enabled { get; init; } = true;
    public ModuleTuning Tuning { get; init; } = ModuleTuning.Default;

    /// <summary>Band module → koi error nahi (tuning ignore). Enabled → tuning ki errors.</summary>
    public IEnumerable<string> Validate()
    {
        if (!Enabled) yield break;
        foreach (string error in Tuning.Validate())
            yield return $"{Module}: {error}";
    }
}

/// <summary>
/// Per-module PipelinePlan (SW-6 P11 / Phase B skeleton, Phase C wiring).
/// WheelEngine / ProcessingPipeline is plan ko parhte hain: SetPlan plan
/// badalne par pipeline dobara wire karta hai (per-module tuning + on/off),
/// aur plan Profile v2 (ModuleTunings) se load/save hota hai.
/// Default plan ka matlab "sab modules on, default tuning" hai,
/// aur mojooda behaviour (jo pipeline aaj karta hai) is se badalta nahi.
/// </summary>
public sealed class PipelinePlan
{
    /// <summary>
    /// SEC-E-08: pipeline plan ka schema version — ye PROFILE file ka
    /// version NAHI hai (us ki authority sirf
    /// <see cref="Profiles.ProfileFile.CurrentSchemaVersion"/> hai).
    /// Doosri "SchemaVersion" definition thi — naam ka takrao khatam
    /// karne ke liye is ka naam PlanSchemaVersion hai; plan ka version
    /// <see cref="Version"/> se milta hai.
    /// </summary>
    public const int PlanSchemaVersion = 1;

    public static readonly ModuleKind[] AllModules =
        Enum.GetValues<ModuleKind>();

    private readonly IReadOnlyDictionary<ModuleKind, ModulePlanEntry> _entries;

    public PipelinePlan(
        IReadOnlyDictionary<ModuleKind, ModulePlanEntry> entries,
        string name = "Default")
    {
        // Har module ki entry lazmi — aadha plan kabhi valid nahi.
        var missing = AllModules.Where(m => !entries.ContainsKey(m)).ToArray();
        if (missing.Length > 0)
            throw new PipelinePlanValidationException(
                missing.Select(m => $"{m}: entry missing hai.").ToArray());
        _entries = new Dictionary<ModuleKind, ModulePlanEntry>(entries);
        Name = string.IsNullOrWhiteSpace(name) ? "Default" : name.Trim();
    }

    public string Name { get; }
    public int Version => PlanSchemaVersion;

    public IReadOnlyDictionary<ModuleKind, ModulePlanEntry> Entries => _entries;

    public ModulePlanEntry this[ModuleKind module] => _entries[module];

    public bool IsEnabled(ModuleKind module) =>
        _entries.TryGetValue(module, out var entry) && entry.Enabled;

    /// <summary>Sirf enabled modules — band module pipeline mein nahi aata.</summary>
    public IReadOnlyList<ModuleKind> EnabledModules =>
        AllModules.Where(IsEnabled).ToArray();

    /// <summary>Sirf enabled modules ki entries.</summary>
    public IReadOnlyList<ModulePlanEntry> EnabledEntries =>
        AllModules.Where(IsEnabled).Select(m => _entries[m]).ToArray();

    /// <summary>Tamaam enabled entries ki validation errors; khaali = valid.</summary>
    public IReadOnlyList<string> Validate() =>
        AllModules.SelectMany(m => _entries[m].Validate()).ToArray();

    public bool IsValid => Validate().Count == 0;

    /// <summary>Sab modules on, default tuning — mojooda behaviour ke barabar.</summary>
    public static PipelinePlan DefaultPlan() =>
        new PipelinePlanBuilder().Build();
}
