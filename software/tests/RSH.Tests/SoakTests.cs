using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests.Scenarios;

/// <summary>
/// T12 — Soak test mein drop + watchdog + alloc asserts.
/// Virtual ghari (Harness) se 30 s @ 200 Hz deterministic chalta hai —
/// koi wall-clock, koi socket nahi. Alloc assert engine-tick path ko
/// counting output se maapta hai (RecordingOutput ki List grow karti hai,
/// is liye wo alloc measurement ko kharab karti).
/// </summary>
public class SoakTests
{
    [Fact]
    public void Soak_30s200Hz_DropsCounted_EngineStaysStable()
    {
        using var h = new Harness();
        // 30 second ki drive: steering sine wave, pedals kabhi kabhi
        for (int t = 0; t < 6000; t++)
        {
            double phase = t / 6000.0 * Math.PI * 8;
            ushort steer = (ushort)(2050 + Math.Sin(phase) * 1000);
            ushort accel = (ushort)(t % 400 < 200 ? 3000 : 200);
            if (t == 3000) h.Wheel.DropNext(5);   // beech mein 5 packets gum (Wi-Fi loss)
            h.Wheel.Emit(steer, accel, 200);
            h.Step();
        }
        Assert.True(h.Output.Updates.Count > 5000);
        Assert.InRange(h.Output.Last.SteeringAxis, 1, 32768);
        // Drop assert: gum hue 5 packets gine gaye, engine phir bhi stable
        Assert.Equal(5, h.Wheel.Statistics.LostPackets);
        Assert.False(h.Engine.WatchdogActive);
        Assert.True(h.Engine.WheelConnected);
    }

    [Fact]
    public void Soak_WatchdogFiresOnSilence_AndRecovers()
    {
        using var h = new Harness();
        // Pehle drive, phir khamoshi — watchdog neutral bhejta hai
        h.Drive(2000, steering: 3100, accel: 3900);
        Assert.Equal(32768, h.Output.Last.SteeringAxis);
        Assert.False(h.Engine.WatchdogActive);

        h.Step(60);                                            // 300 ms khamoshi — koi packet nahi
        Assert.True(h.Engine.WatchdogActive);
        Assert.Equal(ProcessedState.Neutral, h.Output.Last);   // game ko neutral mila

        h.Drive(10, steering: 2050);                           // wheel wapas aya
        Assert.False(h.Engine.WatchdogActive);
        Assert.Equal(16384, h.Output.Last.SteeringAxis);
    }

    [Fact]
    public void Soak_EngineTicks_ZeroAlloc()
    {
        // Taaza packet har tick (smoothing ON — OneEuro path bhi zero-alloc
        // hona chahiye), counting output (koi List grow nahi).
        var source = new FreshSource();
        var output = new CountingOutput();
        var profile = Harness.CalibratedProfile() with { Smoothing = 0.3 };
        using var engine = new WheelEngine(source, output, profile);

        long nowMs = 0;
        for (int i = 0; i < 200; i++) { nowMs += 5; engine.TickOnce(0.005, nowMs); }   // warm-up (JIT)
        // THREAD-LOCAL counter: GetTotalAllocatedBytes process-wide hai —
        // parallel chalti doosri test class ka ek bhi allocation window mein
        // aa jaye to number kharab ho jata hai (80 B/tick tak dekha gaya).
        // TickOnce isi thread par chalta hai, is liye thread-local counter
        // sirf engine-tick path ko maapta hai — doosre threads ka shor nahi.
        long allocWarm = GC.GetAllocatedBytesForCurrentThread();
        for (int i = 0; i < 6000; i++) { nowMs += 5; engine.TickOnce(0.005, nowMs); }  // 30 s @ 200 Hz
        long allocEnd = GC.GetAllocatedBytesForCurrentThread();

        double perTick = (double)(allocEnd - allocWarm) / 6000;
        // Pehla tick sirf connect karta hai (Update nahi) — 200 + 6000 ticks mein 6199 updates.
        Assert.Equal(6199, output.Updates);
        Assert.False(engine.WatchdogActive);
        // SLO: 0 B/tick. 1 B/tick ki gunjaish sirf test-host background noise
        // ke liye hai — ek chhota object bhi har tick par 24+ B/tick banata.
        Assert.True(perTick <= 1.0, $"engine tick alloc {perTick:F2} B/tick — SLO 0 B/tick");
    }

    /// <summary>Har tick taaza packet — koi allocation nahi (struct hi hai).</summary>
    private sealed class FreshSource : ISampleSource
    {
        private ushort _seq;
        public LinkStatistics Statistics { get; } = new();
        public long? LastValidAgeMs => 0;
        public bool TryGetLatest(out WheelPacket packet, out long ageMs, long maxAgeMs = long.MaxValue)
        {
            ushort seq = _seq++;
            packet = new WheelPacket(seq, (uint)(seq * 5000), (ushort)(2050 + (seq % 500) - 250), 1200, 900, 0, true);
            ageMs = 0;
            return true;
        }
    }

    private sealed class CountingOutput : IJoystickOutput
    {
        public string Name => "Counting";
        public bool IsConnected { get; private set; }
        public long Updates;
        public int NeutralSends;
        public bool TryConnect(out string error) { IsConnected = true; error = ""; return true; }
        public void Disconnect() => IsConnected = false;
        public void Update(in ProcessedState state) => Updates++;
        public void SendNeutral() => NeutralSends++;
        public void Dispose() { }
    }
}
