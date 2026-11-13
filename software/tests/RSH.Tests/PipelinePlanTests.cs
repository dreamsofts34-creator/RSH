using RSH.Core.Pipeline;
using RSH.Core.Profiles;

namespace RSH.Tests;

public class PipelinePlanTests
{
    [Fact]
    public void DefaultPlan_HasAllTenModules_AllEnabled_DefaultTuning()
    {
        var plan = PipelinePlan.DefaultPlan();
        Assert.Equal(10, plan.Entries.Count);
        Assert.Equal(10, plan.EnabledModules.Count);
        Assert.True(plan.IsValid);
        Assert.Equal(1, plan.Version);
        foreach (var module in PipelinePlan.AllModules)
        {
            Assert.True(plan.IsEnabled(module));
            Assert.True(plan[module].Tuning.IsDefault);
        }
    }

    [Fact]
    public void Builder_PerModuleTuning_KeptSeparate()
    {
        var plan = new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning { Deadzone = 0.1, CurveStrength = 0.5, Smoothing = 0.3 })
            .WithTuning(ModuleKind.Brake, new ModuleTuning { Deadzone = 0.05 })
            .Build();

        Assert.Equal(0.1, plan[ModuleKind.Steering].Tuning.Deadzone);
        Assert.Equal(0.5, plan[ModuleKind.Steering].Tuning.CurveStrength);
        Assert.Equal(0.05, plan[ModuleKind.Brake].Tuning.Deadzone);
        Assert.Equal(0.0, plan[ModuleKind.Accelerator].Tuning.Deadzone);
        Assert.True(plan.IsValid);
    }

    [Theory]
    [InlineData(0.6)]   // deadzone > 0.5
    [InlineData(-0.1)]  // deadzone < 0
    public void Validation_DeadzoneOutOfRange_Rejected(double deadzone)
    {
        var builder = new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning { Deadzone = deadzone });
        Assert.False(builder.TryBuild(out var plan, out var errors));
        Assert.Null(plan);
        Assert.Contains(errors, e => e.Contains("Steering") && e.Contains("Deadzone"));
        var ex = Assert.Throws<PipelinePlanValidationException>(() => builder.Build());
        Assert.NotEmpty(ex.Errors);
    }

    [Theory]
    [InlineData(1.5)]
    [InlineData(-1.5)]
    public void Validation_CurveOutOfRange_Rejected(double curve)
    {
        Assert.False(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Accelerator, new ModuleTuning { CurveStrength = curve })
            .TryBuild(out _, out var errors));
        Assert.Contains(errors, e => e.Contains("Accelerator") && e.Contains("CurveStrength"));
    }

    [Theory]
    [InlineData(1.2)]
    [InlineData(-0.2)]
    public void Validation_SmoothingOutOfRange_Rejected(double smoothing)
    {
        Assert.False(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Brake, new ModuleTuning { Smoothing = smoothing })
            .TryBuild(out _, out _));
    }

    [Theory]
    [InlineData(0.1)]  // scale < 0.2
    [InlineData(1.5)]  // scale > 1.0
    public void Validation_ScaleOutOfRange_Rejected(double scale)
    {
        Assert.False(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning { Scale = scale })
            .TryBuild(out _, out var errors));
        Assert.Contains(errors, e => e.Contains("Scale"));
    }

    [Fact]
    public void Validation_NaN_Rejected()
    {
        Assert.False(new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning { Deadzone = double.NaN })
            .TryBuild(out _, out _));
    }

    [Fact]
    public void Validation_BoundaryValues_Accepted()
    {
        var plan = new PipelinePlanBuilder()
            .WithTuning(ModuleKind.Steering, new ModuleTuning
            {
                Deadzone = 0.5, CurveStrength = -1, Smoothing = 1, Scale = 0.2,
            })
            .Build();
        Assert.True(plan.IsValid);
    }

    [Fact]
    public void DisabledModule_ExcludedFromEnabledLists_AndTuningIgnored()
    {
        // Band module ki tuning hadd se bahar bhi ho to plan valid hai —
        // wo tuning kabhi istemal nahi hogi (v2.3 §4 rule).
        var plan = new PipelinePlanBuilder()
            .WithModule(ModuleKind.Accelerator, enabled: false,
                tuning: new ModuleTuning { Deadzone = 9.9, CurveStrength = 99, Smoothing = -5, Scale = 0 })
            .WithModule(ModuleKind.LED, enabled: false)
            .Build();

        Assert.True(plan.IsValid);
        Assert.False(plan.IsEnabled(ModuleKind.Accelerator));
        Assert.False(plan.IsEnabled(ModuleKind.LED));
        Assert.DoesNotContain(ModuleKind.Accelerator, plan.EnabledModules);
        Assert.DoesNotContain(ModuleKind.Accelerator, plan.EnabledEntries.Select(e => e.Module));
        Assert.True(plan.IsEnabled(ModuleKind.Steering));
        Assert.Equal(8, plan.EnabledModules.Count);
    }

    [Fact]
    public void DisabledModule_EnabledAgain_TuningValidatedAgain()
    {
        var builder = new PipelinePlanBuilder()
            .WithModule(ModuleKind.Brake, enabled: false,
                tuning: new ModuleTuning { Deadzone = 0.9 });
        Assert.True(builder.TryBuild(out _, out _)); // band → ignore
        builder.WithEnabled(ModuleKind.Brake, true);
        Assert.False(builder.TryBuild(out _, out var errors)); // on → reject
        Assert.Contains(errors, e => e.Contains("Brake"));
    }

    [Fact]
    public void FromProfile_MapsSteeringTuning_AndInvertFlags()
    {
        var profile = new WheelProfile
        {
            Name = "Race",
            Deadzone = 0.08,
            CurveStrength = -0.4,
            Smoothing = 0.6,
            InvertSteering = true,
            InvertBrake = true,
            SteeringScale = 0.4,
        };
        var plan = PipelinePlanBuilder.FromProfile(profile);

        Assert.Equal("Race", plan.Name);
        Assert.Equal(0.08, plan[ModuleKind.Steering].Tuning.Deadzone);
        Assert.Equal(-0.4, plan[ModuleKind.Steering].Tuning.CurveStrength);
        Assert.Equal(0.6, plan[ModuleKind.Steering].Tuning.Smoothing);
        Assert.True(plan[ModuleKind.Steering].Tuning.Invert);
        Assert.Equal(0.4, plan[ModuleKind.Steering].Tuning.Scale);
        Assert.True(plan[ModuleKind.Brake].Tuning.Invert);
        Assert.False(plan[ModuleKind.Accelerator].Tuning.Invert);
        Assert.Equal(0.6, plan[ModuleKind.Accelerator].Tuning.Smoothing);
        Assert.True(plan.IsValid);
    }

    [Fact]
    public void FromProfile_BadProfileValues_Sanitized_NotRejected()
    {
        // WheelProfile.Sanitized() clamp karta hai — FromProfile kabhi
        // ghalat plan nahi banata.
        var plan = PipelinePlanBuilder.FromProfile(new WheelProfile
        {
            Deadzone = 9, CurveStrength = 9, Smoothing = 9, SteeringScale = 9,
        });
        Assert.True(plan.IsValid);
        Assert.Equal(0.5, plan[ModuleKind.Steering].Tuning.Deadzone);
    }

    [Fact]
    public void Builder_MissingModuleEntry_Rejected()
    {
        var entries = PipelinePlan.AllModules
            .Where(m => m != ModuleKind.Horn)
            .ToDictionary(m => m, m => new ModulePlanEntry { Module = m });
        var ex = Assert.Throws<PipelinePlanValidationException>(() => new PipelinePlan(entries));
        Assert.Contains(ex.Errors, e => e.Contains("Horn"));
    }

    [Fact]
    public void UpdateTuning_OnlyChangesNamedModule()
    {
        var plan = new PipelinePlanBuilder()
            .UpdateTuning(ModuleKind.Clutch, t => t with { Smoothing = 0.7 })
            .Build();
        Assert.Equal(0.7, plan[ModuleKind.Clutch].Tuning.Smoothing);
        Assert.Equal(0.0, plan[ModuleKind.Handbrake].Tuning.Smoothing);
    }
}
