using HIDMaestro;
using RSH.Output.HIDMaestro;

namespace RSH.HIDMaestro.Tests;

/// <summary>
/// Fake SDK — asal driver ke baghair HidMaestroOutput ka poora lifecycle
/// test karta hai. Har submit ki snapshot rakhta hai (axes/buttons/raw
/// bytes) taake mapping + counts assert ho sakein.
/// </summary>
internal sealed class FakeHidMaestroSdk : IHidMaestroSdk
{
    public FakeHidMaestroSdk(string profileId = "fake-wheel")
    {
        ProfileId = profileId;
    }

    public string ProfileId { get; }
    public string? SdkVersion => "2.1.0-fake";
    public string? DriverVersionText => "2.1.0-fake";
    public HidMaestroCapabilities Capabilities { get; set; } = HidMaestroCapabilities.Unknown;

    public bool CreateResult { get; set; } = true;
    public string CreateError { get; set; } = "fake create failed";
    public bool ThrowOnSubmit { get; set; }

    public int CreateCalls { get; private set; }
    public int DestroyCalls { get; private set; }
    public int SubmitStateCalls { get; private set; }
    public int SubmitRawCalls { get; private set; }

    public Dictionary<HMAxis, float> LastAxes { get; private set; } = new();
    public HMButton LastButtons { get; private set; }
    public HMHat LastHat { get; private set; }
    public byte[]? LastRawReport { get; private set; }

    public bool TryCreateDevice(out string error)
    {
        CreateCalls++;
        if (!CreateResult)
        {
            error = CreateError;
            return false;
        }
        error = "";
        return true;
    }

    public void SubmitState(in HMGamepadState state)
    {
        SubmitStateCalls++;
        if (ThrowOnSubmit) throw new InvalidOperationException("fake submit failed");
        LastAxes = state.Axes is null ? new Dictionary<HMAxis, float>() : new Dictionary<HMAxis, float>(state.Axes);
        LastButtons = state.Buttons;
        LastHat = state.Hat;
    }

    public void SubmitRawReport(ReadOnlySpan<byte> report)
    {
        SubmitRawCalls++;
        if (ThrowOnSubmit) throw new InvalidOperationException("fake submit failed");
        LastRawReport = report.ToArray();
    }

    public void DestroyDevice() => DestroyCalls++;

    public void Dispose() => DestroyDevice();
}

/// <summary>OutputBackendSelector tests ke liye chhota sa IJoystickOutput fake.</summary>
internal sealed class FakeJoystickOutput : RSH.Core.Output.IJoystickOutput
{
    public FakeJoystickOutput(string name) => Name = name;
    public string Name { get; }
    public bool IsConnected { get; private set; }
    public bool TryConnect(out string error) { error = ""; IsConnected = true; return true; }
    public void Disconnect() => IsConnected = false;
    public void Update(in RSH.Core.Output.ProcessedState state) { }
    public void SendNeutral() { }
    public void Dispose() => Disconnect();
}
