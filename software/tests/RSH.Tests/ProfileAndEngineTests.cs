using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests;

public class ProfileStoreTests
{
    private static string TempFile() =>
        Path.Combine(Path.GetTempPath(), "rsh-tests", Guid.NewGuid().ToString("N"), "profiles.json");

    [Fact]
    public void Roundtrip_SaveLoad_WaisaHi()
    {
        string file = TempFile();
        var store = new ProfileStore(file);
        var data = new ProfileFile
        {
            Profiles =
            [
                WheelProfile.CreateDefault() with { Deadzone = 0.07, CurveStrength = 0.4 },
                WheelProfile.CreateDefault("Race") with { Smoothing = 0.9 },
            ],
            ActiveProfileName = "Race",
        };
        store.Save(data);

        var loaded = store.Load();
        Assert.Equal(2, loaded.Profiles.Count);
        Assert.Equal("Race", loaded.ActiveProfileName);
        Assert.Equal(0.9, ProfileStore.GetActive(loaded).Smoothing, 6);   // active = "Race"
        var def = loaded.Profiles.Single(p => p.Name == "Default");
        Assert.Equal(0.07, def.Deadzone, 6);
        Assert.Equal(0.4, def.CurveStrength, 6);
        Assert.False(File.Exists(file + ".tmp"));   // atomic save ka kachra nahi
    }

    [Fact]
    public void CorruptFile_BackupSeRestore_HotaHai()
    {
        string file = TempFile();
        var store = new ProfileStore(file);
        var data = new ProfileFile
        {
            Profiles = [WheelProfile.CreateDefault() with { Deadzone = 0.11 }],
            ActiveProfileName = "Default",
        };
        store.Save(data);
        store.Save(data);   // dusri save par .bak banta hai

        File.WriteAllText(file, "{ ye JSON nahi hai !!!");
        var loaded = store.Load();
        Assert.Single(loaded.Profiles);
        Assert.Equal(0.11, loaded.Profiles[0].Deadzone, 6);   // backup se wapas
    }

    [Fact]
    public void CorruptFileAurBackup_DefaultsMilteHain_AppBandNahiHota()
    {
        string file = TempFile();
        Directory.CreateDirectory(Path.GetDirectoryName(file)!);
        File.WriteAllText(file, "garbage");
        var loaded = new ProfileStore(file).Load();
        Assert.Single(loaded.Profiles);
        Assert.Equal("Default", loaded.ActiveProfileName);
    }

    [Fact]
    public void Sanitized_HadSeBahirQeemateinTheekKartaHai()
    {
        // P2 (D9) ke baad: button numbers is test mein nahi — wo clamp
        // nahi hote, reject hote hain (neeche wale tests dekho).
        var p = WheelProfile.CreateDefault() with
        {
            Deadzone = 5, CurveStrength = -9, Smoothing = 3,
        };
        var s = p.Sanitized();
        Assert.Equal(0.5, s.Deadzone);
        Assert.Equal(-1.0, s.CurveStrength);
        Assert.Equal(1.0, s.Smoothing);
    }

    // P2 (D9): vJoy buttons sirf 1..32. 33, 128, 0 → error (reject),
    // clamp/alias kabhi nahi. 32 → theek hai.
    [Theory]
    [InlineData(33)]
    [InlineData(128)]
    [InlineData(0)]
    [InlineData(-5)]
    public void Sanitized_VJoyButtonHadSeBahir_RejectHotaHai(int button)
    {
        var up = WheelProfile.CreateDefault() with { GearUpVJoyButton = button };
        Assert.Throws<ArgumentOutOfRangeException>(() => { _ = up.Sanitized(); });
        var down = WheelProfile.CreateDefault() with { GearDownVJoyButton = button };
        Assert.Throws<ArgumentOutOfRangeException>(() => { _ = down.Sanitized(); });
    }

    [Theory]
    [InlineData(1, 2)]
    [InlineData(32, 2)]
    [InlineData(1, 32)]
    public void Sanitized_VJoyButton1Se32_OK(int up, int down)
    {
        var s = (WheelProfile.CreateDefault() with { GearUpVJoyButton = up, GearDownVJoyButton = down }).Sanitized();
        Assert.Equal(up, s.GearUpVJoyButton);
        Assert.Equal(down, s.GearDownVJoyButton);
    }

    [Fact]
    public void Load_Button33WaliFile_DefaultsParGirtiHai_AppBandNahiHota()
    {
        // Purani/haath se badli file mein button 33 ho to Load throw
        // nahi karta — backup/defaults milte hain.
        string file = TempFile();
        var store = new ProfileStore(file);
        store.Save(new ProfileFile
        {
            Profiles = [WheelProfile.CreateDefault() with { GearUpVJoyButton = 33 }],
            ActiveProfileName = "Default",
        });
        var loaded = store.Load();
        Assert.Single(loaded.Profiles);
        Assert.Equal("Default", loaded.ActiveProfileName);
        Assert.Equal(1, loaded.Profiles[0].GearUpVJoyButton);
    }

    [Fact]
    public void Pipeline_GearButton33_RejectHotaHai_AliasNahiBanta()
    {
        // Pehle `1u << (33-1)` wrap hokar button 1 par alias banta tha.
        var profile = WheelProfile.CreateDefault() with { GearUpVJoyButton = 33, Smoothing = 0 };
        Assert.Throws<ArgumentOutOfRangeException>(() => { _ = new ProcessingPipeline(profile); });
    }

    [Fact]
    public void Pipeline_GearButton32_Bit31ParMapHotaHai()
    {
        var profile = WheelProfile.CreateDefault() with
        {
            GearUpVJoyButton = 32, Smoothing = 0, Deadzone = 0,
            SteeringCalibration = new SteeringCalibration
            { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true },
        };
        var pipeline = new ProcessingPipeline(profile);
        var state = pipeline.Process(new WheelPacket(1, 0, 2050, 300, 300, WheelPacket.ButtonGearUp, true), 0.005);
        Assert.Equal(1u << 31, state.ButtonsMask);   // vJoy button 32 = bit 31
    }
}

/// <summary>Nakli source: jo packet do, wahi umar ke sath wapas.</summary>
internal sealed class FakeSource : ISampleSource
{
    public WheelPacket Packet;
    public long AgeMs;
    public LinkStatistics Statistics { get; } = new();
    public long? LastValidAgeMs => AgeMs;
    public bool TryGetLatest(out WheelPacket packet, out long ageMs, long maxAgeMs = long.MaxValue)
    {
        packet = Packet;
        ageMs = AgeMs;
        return ageMs <= maxAgeMs;
    }
}

internal sealed class FakeOutput : IJoystickOutput
{
    public string Name => "Fake";
    public bool IsConnected { get; private set; }
    public ProcessedState LastState;
    public int UpdateCount;
    public bool FailConnect;
    public bool ThrowOnUpdate;

    public bool TryConnect(out string error)
    {
        if (FailConnect) { error = "nakli nakami"; return false; }
        IsConnected = true;
        error = "";
        return true;
    }
    public void Disconnect() => IsConnected = false;
    public void Update(in ProcessedState state)
    {
        if (ThrowOnUpdate) throw new InvalidOperationException("nakli output kharabi");
        LastState = state;
        UpdateCount++;
    }
    public void SendNeutral() => LastState = ProcessedState.Neutral;
    public void Dispose() { }
}

public class EngineTests
{
    private static WheelPacket Packet(ushort steer = 2050, byte buttons = 0) =>
        new(1, 0, steer, 300, 300, buttons, true);

    [Fact]
    public void Engine_FreshData_OutputTakJataHai()
    {
        var source = new FakeSource { Packet = Packet(3100), AgeMs = 10 };
        var output = new FakeOutput();
        var profile = WheelProfile.CreateDefault() with
        {
            Smoothing = 0,
            Deadzone = 0,
            SteeringCalibration = new RSH.Core.Processing.SteeringCalibration
            { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true, IsMillivolts = true },
        };
        using var engine = new WheelEngine(source, output, profile);

        engine.TickOnce(0.005, 1000);   // pehla tick: connect
        engine.TickOnce(0.005, 1005);   // dusra: update

        Assert.False(engine.WatchdogActive);
        Assert.True(output.UpdateCount >= 1);
        Assert.Equal(32768, output.LastState.SteeringAxis);   // full right
    }

    [Fact]
    public void Engine_PuranaData_WatchdogNeutralBhejtaHai()
    {
        var source = new FakeSource { Packet = Packet(1000), AgeMs = 900 };   // 900 ms purana
        var output = new FakeOutput();
        using var engine = new WheelEngine(source, output, WheelProfile.CreateDefault());

        engine.TickOnce(0.005, 1000);
        engine.TickOnce(0.005, 1005);

        Assert.True(engine.WatchdogActive);
        Assert.Equal(ProcessedState.Neutral, output.LastState);
    }

    [Fact]
    public void Engine_OutputKharabi_LoopKoNahiTorti_DobaraConnectHotaHai()
    {
        var source = new FakeSource { Packet = Packet(), AgeMs = 5 };
        var output = new FakeOutput();
        using var engine = new WheelEngine(source, output, WheelProfile.CreateDefault());

        engine.TickOnce(0.005, 1000);              // connect
        output.ThrowOnUpdate = true;
        engine.TickOnce(0.005, 1005);              // update throw — disconnect ho jana chahiye
        Assert.False(output.IsConnected);
        Assert.NotNull(engine.LastOutputError);

        output.ThrowOnUpdate = false;
        engine.TickOnce(0.005, 4000);              // 2 s baad reconnect ki koshish
        Assert.True(output.IsConnected);
        engine.TickOnce(0.005, 4005);
        Assert.True(output.UpdateCount >= 1);
    }

    [Fact]
    public void Engine_GearButton_ProfileKeButtonParMapHotaHai()
    {
        var source = new FakeSource { Packet = Packet(2050, WheelPacket.ButtonGearUp), AgeMs = 5 };
        var output = new FakeOutput();
        var profile = WheelProfile.CreateDefault() with { GearUpVJoyButton = 5, Smoothing = 0 };
        using var engine = new WheelEngine(source, output, profile);

        engine.TickOnce(0.005, 1000);
        engine.TickOnce(0.005, 1005);

        Assert.Equal(1u << 4, output.LastState.ButtonsMask);   // vJoy button 5 = bit 4
        Assert.Equal(16384, output.LastState.SteeringAxis);    // center steering
    }

    [Fact]
    public void Engine_GearButton32_Bit31ParMapHotaHai()
    {
        // P2 (D9): button 32 aakhri jaiz button hai — bit 31, alias nahi.
        var source = new FakeSource { Packet = Packet(2050, WheelPacket.ButtonGearUp), AgeMs = 5 };
        var output = new FakeOutput();
        var profile = WheelProfile.CreateDefault() with { GearUpVJoyButton = 32, Smoothing = 0 };
        using var engine = new WheelEngine(source, output, profile);

        engine.TickOnce(0.005, 1000);
        engine.TickOnce(0.005, 1005);

        Assert.Equal(1u << 31, output.LastState.ButtonsMask);   // vJoy button 32 = bit 31
    }

    [Fact]
    public void Engine_GearButton33_ProfileRejectKartaHai()
    {
        // P2 (D9): button 33 wala profile engine tak nahi pahunchta.
        var source = new FakeSource { Packet = Packet(2050, WheelPacket.ButtonGearUp), AgeMs = 5 };
        var output = new FakeOutput();
        var profile = WheelProfile.CreateDefault() with { GearUpVJoyButton = 33, Smoothing = 0 };
        Assert.Throws<ArgumentOutOfRangeException>(() => { _ = new WheelEngine(source, output, profile); });
    }
}
