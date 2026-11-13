using RSH.Core.Profiles;

namespace RSH.Core.Pipeline;

/// <summary>
/// PipelinePlan builder. Shuruwaat: sab modules on + default tuning
/// (DefaultPlan ke barabar). Har module ko alag se on/off ya tune karo;
/// <see cref="Build"/> validation karta hai aur ghalat plan par
/// <see cref="PipelinePlanValidationException"/> phenkta hai.
/// </summary>
public sealed class PipelinePlanBuilder
{
    private readonly Dictionary<ModuleKind, ModulePlanEntry> _entries = new();
    private string _name = "Default";

    public PipelinePlanBuilder()
    {
        foreach (var module in PipelinePlan.AllModules)
            _entries[module] = new ModulePlanEntry { Module = module };
    }

    public PipelinePlanBuilder WithName(string name)
    {
        _name = name;
        return this;
    }

    public PipelinePlanBuilder WithModule(ModuleKind module, bool enabled, ModuleTuning? tuning = null)
    {
        _entries[module] = new ModulePlanEntry
        {
            Module = module,
            Enabled = enabled,
            Tuning = tuning ?? _entries[module].Tuning,
        };
        return this;
    }

    public PipelinePlanBuilder WithEnabled(ModuleKind module, bool enabled) =>
        WithModule(module, enabled);

    public PipelinePlanBuilder WithTuning(ModuleKind module, ModuleTuning tuning) =>
        WithModule(module, _entries[module].Enabled, tuning);

    /// <summary>
    /// Kisi module ki tuning mein tabdeeli (baqi slots waisi ki waisi).
    /// </summary>
    public PipelinePlanBuilder UpdateTuning(ModuleKind module, Func<ModuleTuning, ModuleTuning> update)
    {
        var entry = _entries[module];
        return WithModule(module, entry.Enabled, update(entry.Tuning));
    }

    /// <summary>
    /// Mojooda WheelProfile se plan banao. Legacy (v1) profile mein sirf
    /// steering ki shared tuning hai (Deadzone/Curve/Smoothing) — wo
    /// Steering entry mein jati hai; Accelerator/Brake ko sirf un ke
    /// Invert flags + shared smoothing milta hai; baqi modules default.
    /// Profile v2 mein <see cref="WheelProfile.ModuleTunings"/> ho to us
    /// ki entries legacy base par override hoti hain (sirf mojood
    /// modules — aadhi dict bhi theek hai, baqi legacy rehte hain).
    /// </summary>
    public static PipelinePlan FromProfile(WheelProfile profile)
    {
        var p = profile.Sanitized();
        var builder = new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning
            {
                Deadzone = p.Deadzone,
                CurveStrength = p.CurveStrength,
                Smoothing = p.Smoothing,
                Invert = p.InvertSteering,
                Scale = p.SteeringScale,
            })
            .WithTuning(ModuleKind.Accelerator, new ModuleTuning
            {
                Smoothing = p.Smoothing,
                Invert = p.InvertAccel,
            })
            .WithTuning(ModuleKind.Brake, new ModuleTuning
            {
                Smoothing = p.Smoothing,
                Invert = p.InvertBrake,
            })
            .WithName(p.Name);
        foreach (var (key, tuning) in p.ModuleTunings)
        {
            if (!Enum.TryParse<ModuleKind>(key, ignoreCase: true, out var module)) continue;
            builder.WithModule(module, tuning.Enabled, new ModuleTuning
            {
                Deadzone = tuning.Deadzone,
                CurveStrength = tuning.CurveStrength,
                Smoothing = tuning.Smoothing,
                Invert = tuning.Invert,
                Scale = tuning.Scale,
                GateMode = tuning.GateMode,
            });
        }
        return builder.Build();
    }

    /// <summary>
    /// Plan ko profile mein save karne ke liye dict mein badlo — har
    /// module ki entry (tuning + on/off). Legacy readers ke liye
    /// WheelEngine steering ki qeematein v1 fields mein bhi likhta hai.
    /// </summary>
    public static Dictionary<string, PerModuleTuning> ToModuleTunings(PipelinePlan plan)
    {
        ArgumentNullException.ThrowIfNull(plan);
        var dict = new Dictionary<string, PerModuleTuning>(StringComparer.Ordinal);
        foreach (var module in PipelinePlan.AllModules)
        {
            var entry = plan[module];
            var t = entry.Tuning;
            dict[module.ToString()] = new PerModuleTuning
            {
                Deadzone = t.Deadzone,
                CurveStrength = t.CurveStrength,
                Smoothing = t.Smoothing,
                Invert = t.Invert,
                Scale = t.Scale,
                Enabled = entry.Enabled,
                GateMode = t.GateMode,
            };
        }
        return dict;
    }

    public bool TryBuild(out PipelinePlan? plan, out IReadOnlyList<string> errors)
    {
        var candidate = new PipelinePlan(_entries, _name);
        errors = candidate.Validate();
        plan = errors.Count == 0 ? candidate : null;
        return plan is not null;
    }

    public PipelinePlan Build()
    {
        var plan = new PipelinePlan(_entries, _name);
        var errors = plan.Validate();
        if (errors.Count > 0)
            throw new PipelinePlanValidationException(errors);
        return plan;
    }
}
