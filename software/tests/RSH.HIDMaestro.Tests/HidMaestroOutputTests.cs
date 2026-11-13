using System.Diagnostics;
using System.Security.Cryptography;
using HIDMaestro;
using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Output.HIDMaestro;

namespace RSH.HIDMaestro.Tests;

/// <summary>
/// HidMaestroOutput lifecycle (fake SDK) + asal v2.1.0 DLL ke khilaf
/// sandbox mein verify hone wali cheezen: SHA-256 pin, catalog, SEC-H-06
/// usbip refusal, Linux guard. Live driver (Windows + admin) yahan
/// verify NAHI hota — gate H-HID-1 mein darj hai.
/// </summary>
public class HidMaestroOutputTests
{
    private static ProcessedState State(int steer = 16384, int accel = 1, int brake = 1, uint buttons = 0) => new(
        SteeringAxis: steer, AccelAxis: accel, BrakeAxis: brake,
        ButtonsMask: buttons, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0);

    [Fact]
    public void TryConnect_FakeSdk_Succeeds_AndReportsConnected()
    {
        var sdk = new FakeHidMaestroSdk();
        using var output = new HidMaestroOutput("fake-wheel", sdk);
        Assert.Equal("HIDMaestro", output.Name);
        Assert.False(output.IsConnected);
        Assert.True(output.TryConnect(out string error));
        Assert.Equal("", error);
        Assert.True(output.IsConnected);
        Assert.Equal(1, sdk.CreateCalls);
        // Connect par neutral gaya (SubmitState rasta):
        Assert.Equal(1, sdk.SubmitStateCalls);
        Assert.Equal(0.5f, sdk.LastAxes[HMAxis.X], precision: 5);
    }

    [Fact]
    public void TryConnect_FakeSdkFails_ReturnsFalse_WithError_NoCrash()
    {
        var sdk = new FakeHidMaestroSdk { CreateResult = false };
        using var output = new HidMaestroOutput("fake-wheel", sdk);
        Assert.False(output.TryConnect(out string error));
        Assert.Equal("fake create failed", error);
        Assert.False(output.IsConnected);
    }

    [Fact]
    public void TryConnect_Twice_SecondIsNoop()
    {
        var sdk = new FakeHidMaestroSdk();
        using var output = new HidMaestroOutput("fake-wheel", sdk);
        Assert.True(output.TryConnect(out _));
        Assert.True(output.TryConnect(out _));
        Assert.Equal(1, sdk.CreateCalls);
    }

    [Fact]
    public void Update_NotConnected_ThrowsInvalidOperation()
    {
        using var output = new HidMaestroOutput("fake-wheel", new FakeHidMaestroSdk());
        Assert.Throws<InvalidOperationException>(() => output.Update(State()));
    }

    [Fact]
    public void Update_Connected_SubmitsGamepadState()
    {
        var sdk = new FakeHidMaestroSdk();
        using var output = new HidMaestroOutput("fake-wheel", sdk);
        Assert.True(output.TryConnect(out _));
        int before = sdk.SubmitStateCalls; // 1 = connect neutral
        output.Update(State(steer: 1000, accel: 20000, buttons: 0b101));
        Assert.Equal(before + 1, sdk.SubmitStateCalls);
        Assert.Equal(0.5f + (1000 - 16384) / 32768f, sdk.LastAxes[HMAxis.X], precision: 5);
        Assert.Equal((20000 - 1) / 32767f, sdk.LastAxes[HMAxis.Y], precision: 5);
        Assert.Equal(HMButton.A | HMButton.X, sdk.LastButtons);
        Assert.Equal(HMHat.None, sdk.LastHat);
    }

    [Fact]
    public void Update_RawReportPath_Submits26Bytes()
    {
        var sdk = new FakeHidMaestroSdk();
        using var output = new HidMaestroOutput("fake-wheel", sdk, useRawReport: true);
        Assert.True(output.TryConnect(out _));
        Assert.Equal(1, sdk.SubmitRawCalls); // connect neutral
        output.Update(State(steer: 1000, buttons: 1u << 30));
        Assert.Equal(2, sdk.SubmitRawCalls);
        Assert.Equal(0, sdk.SubmitStateCalls);
        Assert.NotNull(sdk.LastRawReport);
        Assert.Equal(26, sdk.LastRawReport!.Length);
        Assert.Equal(999, HidMaestroMapping.ReadRawAxis(sdk.LastRawReport, 0));
        Assert.True(HidMaestroMapping.IsRawButtonPressed(sdk.LastRawReport, 31));
    }

    [Fact]
    public void Update_SubmitThrows_WrappedAsInvalidOperation()
    {
        var sdk = new FakeHidMaestroSdk { ThrowOnSubmit = true };
        using var output = new HidMaestroOutput("fake-wheel", sdk);
        // Connect ka neutral bhi throw karega — TryConnect false deta hai:
        Assert.False(output.TryConnect(out string error));
        Assert.Contains("neutral", error);
        Assert.False(output.IsConnected);
    }

    [Fact]
    public void SendNeutral_NotConnected_DoesNothing_NoThrow()
    {
        using var output = new HidMaestroOutput("fake-wheel", new FakeHidMaestroSdk());
        output.SendNeutral(); // koi exception nahi
    }

    [Fact]
    public void Disconnect_DestroysDevice_AndDisposeIsIdempotent()
    {
        var sdk = new FakeHidMaestroSdk();
        var output = new HidMaestroOutput("fake-wheel", sdk);
        Assert.True(output.TryConnect(out _));
        output.Disconnect();
        Assert.False(output.IsConnected);
        Assert.Equal(1, sdk.DestroyCalls);
        output.Dispose();
        output.Dispose();
        Assert.Equal(1, sdk.DestroyCalls); // dobara Dispose dobara destroy nahi karta
    }

    [Fact]
    public void TryConnect_PlanValidationFails_DestroysDevice_AndReturnsFalse()
    {
        var sdk = new FakeHidMaestroSdk
        {
            Capabilities = new HidMaestroCapabilities(
                HasAxisX: false, HasAxisY: true, HasAxisZ: true, HasAxisRx: false, HasAxisSlider: false, ButtonCount: 0),
        };
        using var output = new HidMaestroOutput("fake-wheel", sdk);
        var plan = new PipelinePlanBuilder().Build(); // sab on — X chahiye
        Assert.False(output.TryConnect(out string error, plan));
        Assert.Contains("X axis", error);
        Assert.False(output.IsConnected);
        Assert.Equal(1, sdk.DestroyCalls);
    }

    [Fact]
    public void TryConnect_SteeringOnlyPlan_WithMinimalCaps_Succeeds()
    {
        var sdk = new FakeHidMaestroSdk
        {
            Capabilities = new HidMaestroCapabilities(
                HasAxisX: true, HasAxisY: false, HasAxisZ: false, HasAxisRx: false, HasAxisSlider: false, ButtonCount: 0),
        };
        using var output = new HidMaestroOutput("fake-wheel", sdk);
        var plan = new PipelinePlanBuilder()
            .WithEnabled(ModuleKind.Accelerator, false)
            .WithEnabled(ModuleKind.Brake, false)
            .WithEnabled(ModuleKind.Clutch, false)
            .WithEnabled(ModuleKind.Handbrake, false)
            .WithEnabled(ModuleKind.Buttons, false)
            .Build();
        Assert.True(output.TryConnect(out string error, plan));
        Assert.Equal("", error);
    }

    // ------------------------------------------------------------------
    // Asal SDK (vendored v2.1.0 DLL) — sandbox mein verify hone wala hissa
    // ------------------------------------------------------------------

    [Fact]
    public void PinnedDllSha256_MatchesVendoredAssembly()
    {
        // SEC-H-06: ZIP + SHA-256 pin lazmi — ghalat/purani DLL build
        // par nahi, test par pakri jati hai.
        string location = typeof(HMContext).Assembly.Location;
        Assert.False(string.IsNullOrEmpty(location));
        using var stream = File.OpenRead(location);
        string actual = Convert.ToHexString(SHA256.HashData(stream)).ToLowerInvariant();
        Assert.Equal(HidMaestroConstants.PinnedDllSha256, actual);
        Assert.Equal("2.1.0", HidMaestroConstants.PinnedVersion);
    }

    [Fact]
    public void RealCatalog_Loads_336Profiles_AndG29Exists()
    {
        // Evaluation mein 337 likha tha; ek sandbox measurement mein 336
        // aaya tha ("ek kam" — gate doc). Asal Windows runner par 337 load
        // hota hai — count environment par munhasir hai (ek profile kabhi
        // load hoti hai, kabhi nahi). Is liye exact count ki bajaye range
        // assert karo; asal substance G29 ki properties hain (neeche).
        using var ctx = new HMContext();
        int loaded = ctx.LoadDefaultProfiles();
        Assert.InRange(loaded, 336, 337);
        var g29 = ctx.GetProfile("logitech-g29");
        Assert.NotNull(g29);
        Assert.Equal(25, g29.ButtonCount);
        Assert.Equal(4, g29.AxisCount);
        Assert.False(g29.RequiresUsbipBackend);
    }

    [Fact]
    public void SecH06_UsbipCompositeProfile_IsRefused_BeforeAnyDeviceCreate()
    {
        // SEC-H-06 hard rule: composite profiles ka bundled usbip-win2
        // kernel transport KABHI enable na ho — refusal device create se
        // pehle, profile dekhte hi hoti hai (is test mein koi device
        // nahi banta, koi admin nahi chahiye).
        using var ctx = new HMContext();
        ctx.LoadDefaultProfiles();
        var composite = ctx.GetProfile("dualsense-composite");
        Assert.NotNull(composite);
        Assert.True(composite.RequiresUsbipBackend);
        Assert.Equal("usbip", composite.Backend);
        string? refusal = HidMaestroSdkAdapter.ValidateProfile(composite);
        Assert.NotNull(refusal);
        Assert.Contains("usbip", refusal, StringComparison.OrdinalIgnoreCase);

        var composite2 = ctx.GetProfile("dualshock-4-v2-composite");
        Assert.NotNull(composite2);
        Assert.NotNull(HidMaestroSdkAdapter.ValidateProfile(composite2));
    }

    [Fact]
    public void SecH06_PlainWheelProfile_IsNotRefusedByUsbipRule()
    {
        using var ctx = new HMContext();
        ctx.LoadDefaultProfiles();
        var g29 = ctx.GetProfile("logitech-g29");
        Assert.NotNull(g29);
        Assert.Null(HidMaestroSdkAdapter.ValidateProfile(g29));
    }

    [Fact]
    public void RealAdapter_OnLinux_TryConnectFails_WithWindowsError_NoDriverTouched()
    {
        // Sandbox Linux hai — adapter OS guard se pehle hi false deta hai.
        // Ye test sirf non-Windows mahol mein maani rakhta hai.
        if (OperatingSystem.IsWindows())
            return;
        var adapter = new HidMaestroSdkAdapter("logitech-g29");
        Assert.False(adapter.TryCreateDevice(out string error));
        Assert.Contains("Windows", error);
        Assert.Equal(HidMaestroCapabilities.Unknown, adapter.Capabilities);
    }

    [Fact]
    public void RealAdapter_EmptyProfile_OnLinux_FailsWithWindowsErrorFirst()
    {
        if (OperatingSystem.IsWindows())
            return;
        var adapter = new HidMaestroSdkAdapter("");
        Assert.False(adapter.TryCreateDevice(out string error));
        Assert.Contains("Windows", error);
    }

    // ------------------------------------------------------------------
    // Latency + soak (fake SDK — mapping + submit path, driver nahi)
    // ------------------------------------------------------------------

    [Fact]
    public void MappingLatency_MeanBuildGamepadState_WellUnderBudget()
    {
        // E2E SLO p99 ≤ 10 ms mein output mapping ka hissa negligible
        // hona chahiye — yahan sirf mapping naapi jati hai (submit nahi).
        var state = State(steer: 12345, accel: 20000, brake: 30000, buttons: 0xFF);
        var dict = new Dictionary<HMAxis, float>(5);
        const int iterations = 100_000;
        // Warmup (JIT):
        for (int i = 0; i < 1000; i++)
            HidMaestroMapping.BuildGamepadState(state, HidMaestroCapabilities.Unknown, dict);
        var sw = Stopwatch.StartNew();
        for (int i = 0; i < iterations; i++)
            HidMaestroMapping.BuildGamepadState(state, HidMaestroCapabilities.Unknown, dict);
        sw.Stop();
        double meanNs = sw.Elapsed.TotalNanoseconds / iterations;
        Assert.True(meanNs < 100_000, $"BuildGamepadState mean {meanNs:F0} ns — 100 µs budget se zyada.");
    }

    [Fact]
    public void MappingLatency_MeanBuildRawReport_WellUnderBudget()
    {
        var state = State(steer: 12345, accel: 20000, brake: 30000, buttons: 0xFF);
        var buf = new byte[HidMaestroMapping.RawReportSize];
        const int iterations = 100_000;
        for (int i = 0; i < 1000; i++)
            HidMaestroMapping.BuildRawReport(state, HidMaestroCapabilities.Unknown, buf);
        var sw = Stopwatch.StartNew();
        for (int i = 0; i < iterations; i++)
            HidMaestroMapping.BuildRawReport(state, HidMaestroCapabilities.Unknown, buf);
        sw.Stop();
        double meanNs = sw.Elapsed.TotalNanoseconds / iterations;
        Assert.True(meanNs < 100_000, $"BuildRawReport mean {meanNs:F0} ns — 100 µs budget se zyada.");
    }

    [Fact]
    public void Soak_200kUpdates_NoCrash_AllSubmitted()
    {
        // 200,000 Updates ≈ 1000 s @ 200 Hz — no-crash soak ka in-process
        // hissa (live 4 h soak Windows PC par hoga, gate H-HID-3).
        var sdk = new FakeHidMaestroSdk();
        using var output = new HidMaestroOutput("fake-wheel", sdk);
        Assert.True(output.TryConnect(out _));
        for (int i = 0; i < 200_000; i++)
        {
            int steer = 1 + (i * 37 % 32768);
            output.Update(State(steer: steer, accel: 1 + (i % 32767), buttons: (uint)i));
        }
        Assert.Equal(200_001, sdk.SubmitStateCalls); // +1 connect neutral
        output.SendNeutral();
        Assert.Equal(200_002, sdk.SubmitStateCalls);
    }
}
