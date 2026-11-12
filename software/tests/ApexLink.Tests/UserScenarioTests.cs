using ApexLink.Core.Output;
using ApexLink.Core.Pipeline;
using ApexLink.Core.Profiles;
using ApexLink.Core.Protocol;
using ApexLink.Core.Transport;

namespace ApexLink.Tests.Scenarios;

/// <summary>
/// Virtual USER testing: poori journey jese ek asal user chalata hai —
/// simulator-style wheel → engine → (nakli) vJoy, ghari (clock) bhi
/// virtual hai is liye har scenario har dafa bilkul ek jesa chalta hai.
/// Koi asal socket nahi (is mahol mein UDP loopback band hai); asal
/// socket wala rasta UdpSource test + Windows par covered hai.
/// </summary>
internal sealed class SimWheel : ISampleSource
{
    private WheelPacket _latest;
    private bool _has;
    private long _lastEmitMs;
    public long NowMs;
    private ushort _seq;

    public LinkStatistics Statistics { get; } = new();
    public long? LastValidAgeMs => _has ? NowMs - _lastEmitMs : null;

    /// <summary>Wheel ek packet bhejta hai — raw values bilkul firmware jese.</summary>
    public void Emit(ushort steering, ushort accel, ushort brake, byte buttons = 0, bool inMillivolts = false)
    {
        _latest = new WheelPacket(_seq++, (uint)(NowMs * 1000), steering, accel, brake, buttons, inMillivolts);
        _has = true;
        _lastEmitMs = NowMs;
        Statistics.Record(PacketParseResult.Ok, _latest.Sequence);
    }

    /// <summary>Packet gum hua (WiFi loss) — sequence aage barhta hai, packet nahi ata.</summary>
    public void DropNext(int count = 1) => _seq += (ushort)count;

    public bool TryGetLatest(out WheelPacket packet, out long ageMs, long maxAgeMs = long.MaxValue)
    {
        packet = _latest;
        if (!_has) { ageMs = long.MaxValue; return false; }
        ageMs = NowMs - _lastEmitMs;
        return ageMs <= maxAgeMs;
    }
}

internal sealed class RecordingOutput : IJoystickOutput
{
    public string Name => "RecordingVJoy";
    public bool IsConnected { get; private set; }
    public bool FailConnect { get; set; }
    public List<ProcessedState> Updates { get; } = [];
    public int NeutralSends { get; private set; }
    public ProcessedState Last => Updates.Count > 0 ? Updates[^1] : ProcessedState.Neutral;

    public bool TryConnect(out string error)
    {
        if (FailConnect) { error = "vJoy device busy (nakli nakami)"; return false; }
        IsConnected = true;
        error = "";
        return true;
    }

    public void Disconnect() => IsConnected = false;
    public void Update(in ProcessedState state)
    {
        if (!IsConnected) throw new InvalidOperationException("connected nahi");
        Updates.Add(state);
    }
    public void SendNeutral() => NeutralSends++;
    public void Dispose() { }
}

/// <summary>Engine + wheel + output, virtual ghari ke sath — har Step = ek 5 ms tick.</summary>
internal sealed class Harness : IDisposable
{
    public SimWheel Wheel { get; } = new();
    public RecordingOutput Output { get; } = new();
    public WheelEngine Engine { get; }
    public long NowMs;

    // Calibrated profile: steering raw 1000..3100 (center 2050), pedals 200..3900
    public static WheelProfile CalibratedProfile() => WheelProfile.CreateDefault() with
    {
        SteeringCalibration = new ApexLink.Core.Processing.SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true },
        AccelCalibration = new ApexLink.Core.Processing.PedalCalibration { RawMin = 200, RawMax = 3900, IsSet = true },
        BrakeCalibration = new ApexLink.Core.Processing.PedalCalibration { RawMin = 200, RawMax = 3900, IsSet = true },
        Smoothing = 0,
    };

    public Harness(WheelProfile? profile = null)
    {
        Engine = new WheelEngine(Wheel, Output, profile ?? CalibratedProfile());
    }

    /// <summary>Ek tick aage: ghari 5 ms, wheel ki ghari bhi, engine ka tick.</summary>
    public void Step(int ticks = 1)
    {
        for (int i = 0; i < ticks; i++)
        {
            NowMs += 5;
            Wheel.NowMs = NowMs;
            Engine.TickOnce(0.005, NowMs);
        }
    }

    /// <summary>Wheel chal raha hai: har tick naya packet (jese asal 200 Hz stream).</summary>
    public void Drive(int ticks, ushort steering, ushort accel = 200, ushort brake = 200, byte buttons = 0)
    {
        for (int i = 0; i < ticks; i++)
        {
            Wheel.Emit(steering, accel, brake, buttons);
            Step();
        }
    }

    public void Dispose() => Engine.Dispose();
}

public class UserScenarioTests
{
    [Fact]
    public void PehliDrive_CenterFullLockDonoTaraf()
    {
        using var h = new Harness();
        h.Drive(10, steering: 2050);                       // center
        Assert.Equal(16384, h.Output.Last.SteeringAxis);
        h.Drive(10, steering: 3100);                       // full right
        Assert.Equal(32768, h.Output.Last.SteeringAxis);
        h.Drive(10, steering: 1000);                       // full left
        Assert.Equal(1, h.Output.Last.SteeringAxis);
        Assert.True(h.Engine.WheelConnected);
        Assert.False(h.Engine.WatchdogActive);
    }

    [Fact]
    public void Pedals_PuraSafarAurDonoEkSath()
    {
        using var h = new Harness();
        h.Drive(5, 2050, accel: 200, brake: 200);
        Assert.Equal(1, h.Output.Last.AccelAxis);
        Assert.Equal(1, h.Output.Last.BrakeAxis);
        h.Drive(5, 2050, accel: 3900, brake: 3900);
        Assert.Equal(32768, h.Output.Last.AccelAxis);
        Assert.Equal(32768, h.Output.Last.BrakeAxis);
        h.Drive(5, 2050, accel: 2050, brake: 200);         // aadha accel
        Assert.InRange(h.Output.Last.AccelAxis, 15000, 18000);
    }

    [Fact]
    public void GearButtons_SahiVJoyBits()
    {
        using var h = new Harness();
        h.Drive(5, 2050, buttons: WheelPacket.ButtonGearUp);
        Assert.Equal(0b01u, h.Output.Last.ButtonsMask);        // vJoy button 1
        h.Drive(5, 2050, buttons: WheelPacket.ButtonGearDown);
        Assert.Equal(0b10u, h.Output.Last.ButtonsMask);        // vJoy button 2
        h.Drive(5, 2050, buttons: 0);
        Assert.Equal(0u, h.Output.Last.ButtonsMask);
    }

    [Fact]
    public void WheelGhayab_WatchdogNeutral_WapasParRecovery()
    {
        using var h = new Harness();
        h.Drive(10, steering: 3100, accel: 3900);              // zor ki drive
        Assert.Equal(32768, h.Output.Last.SteeringAxis);

        h.Step(60);                                            // 300 ms khamoshi — koi packet nahi
        Assert.True(h.Engine.WatchdogActive);
        Assert.Equal(ProcessedState.Neutral, h.Output.Last);   // game ko neutral mila

        h.Drive(10, steering: 2050);                           // wheel wapas aya
        Assert.False(h.Engine.WatchdogActive);
        Assert.Equal(16384, h.Output.Last.SteeringAxis);
    }

    [Fact]
    public void OutputOff_PuranaInputAtaktaNahi_NeutralJataHai()
    {
        using var h = new Harness();
        h.Drive(10, steering: 3100);
        Assert.Equal(32768, h.Output.Last.SteeringAxis);
        int updatesBefore = h.Output.Updates.Count;

        h.Engine.OutputEnabled = false;                        // user ne output band kiya
        h.Step(10);
        Assert.True(h.Output.NeutralSends >= 1);               // neutral bheja gaya
        Assert.Equal(updatesBefore, h.Output.Updates.Count);   // koi stale update nahi gaya

        h.Engine.OutputEnabled = true;                         // dobara on
        h.Drive(5, steering: 2050);
        Assert.Equal(16384, h.Output.Last.SteeringAxis);
    }

    [Fact]
    public void VJoyBusy_EngineZindaRehtaHai_BaadMeinKhudJurta()
    {
        using var h = new Harness();
        h.Output.FailConnect = true;
        h.Drive(10, steering: 3100);
        Assert.False(h.Output.IsConnected);
        Assert.NotNull(h.Engine.LastOutputError);
        Assert.Equal(32768, h.Engine.CurrentState.SteeringAxis);  // pipeline phir bhi zinda

        h.Output.FailConnect = false;                          // doosri app band hui
        h.Step(450);                                           // reconnect har 2 s — waqt do
        h.Drive(5, steering: 2050);
        Assert.True(h.Output.IsConnected);
        Assert.Equal(16384, h.Output.Last.SteeringAxis);
    }

    [Fact]
    public void PacketLoss_StatsMeinGinaJataHai_EngineSambhalta()
    {
        using var h = new Harness();
        h.Drive(10, 2050);
        h.Wheel.DropNext(5);                                   // 5 packets hawa mein gum
        h.Drive(10, 2050);
        Assert.Equal(5, h.Wheel.Statistics.LostPackets);
        Assert.Equal(16384, h.Output.Last.SteeringAxis);
    }

    [Fact]
    public void ProfileSwitch_BeechDriveMein_ForanLaagu()
    {
        using var h = new Harness();
        h.Drive(5, steering: 2575);                            // halka right
        int normal = h.Output.Last.SteeringAxis;
        Assert.True(normal > 16384);

        var inverted = Harness.CalibratedProfile() with { InvertSteering = true };
        h.Engine.SetProfile(inverted);
        h.Drive(5, steering: 2575);
        int flipped = h.Output.Last.SteeringAxis;
        Assert.True(flipped < 16384);
        Assert.Equal(32768 - normal, flipped);                 // center 16384 ke gird aaina
    }

    [Fact]
    public void DigitalPedalButton_RampSeCharhtaHai()
    {
        using var h = new Harness();
        // Button dabaye rakho: 150 ms press ramp → takreeban full
        for (int i = 0; i < 40; i++)
        {
            h.Wheel.Emit(2050, 200, 200, WheelPacket.ButtonAccel);
            h.Step();
        }
        Assert.True(h.Output.Last.AccelNorm > 0.95, $"accel norm {h.Output.Last.AccelNorm:F2}");
        // Choro: release ramp se utarta hai
        h.Drive(40, 2050);
        Assert.True(h.Output.Last.AccelNorm < 0.05);
    }

    [Fact]
    public void Soak_AdhaMinute200Hz_Stable()
    {
        using var h = new Harness();
        // 30 second ki drive: steering sine wave, pedals kabhi kabhi
        for (int t = 0; t < 6000; t++)
        {
            double phase = t / 6000.0 * Math.PI * 8;
            ushort steer = (ushort)(2050 + Math.Sin(phase) * 1000);
            ushort accel = (ushort)(t % 400 < 200 ? 3000 : 200);
            h.Wheel.Emit(steer, accel, 200);
            h.Step();
        }
        Assert.True(h.Output.Updates.Count > 5000);
        Assert.InRange(h.Output.Last.SteeringAxis, 1, 32768);
        Assert.Equal(0, h.Wheel.Statistics.LostPackets);
    }
}
