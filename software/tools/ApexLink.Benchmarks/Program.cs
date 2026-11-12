using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using ApexLink.Core.Output;
using ApexLink.Core.Pipeline;
using ApexLink.Core.Profiles;
using ApexLink.Core.Protocol;
using ApexLink.Core.Transport;

// ApexLink Benchmarks — "halka hai" ka dawa nahi, number hai.
// Chalao: dotnet run -c Release --project tools/ApexLink.Benchmarks -- --soak-sec 60
// Budgets (SOFTWARE-PLAN.md): hot path ~0 alloc, engine tick avg bohat kam,
// e2e loopback loss 0, soak mein memory growth bounded.

int soakSec = 60;
int iArg = Array.IndexOf(args, "--soak-sec");
if (iArg >= 0 && iArg + 1 < args.Length) soakSec = int.Parse(args[iArg + 1]);

Console.WriteLine("=== ApexLink Benchmarks (Release build mein chalao) ===");
Console.WriteLine($"Machine: {Environment.MachineName}, CPUs: {Environment.ProcessorCount}, OS: {Environment.OSVersion}");

// ---------- 1) Parse + CRC ----------
var golden = new WheelPacket(0x1234, 0x01020304, 2300, 1000, 2000, 0x05, true).ToBytes();
{
    const int N = 200_000;
    long allocBefore = GC.GetTotalAllocatedBytes(true);
    var sw = Stopwatch.StartNew();
    int ok = 0;
    for (int n = 0; n < N; n++)
        if (WheelPacket.TryParse(golden, out _) == PacketParseResult.Ok) ok++;
    sw.Stop();
    long alloc = GC.GetTotalAllocatedBytes(true) - allocBefore;
    Console.WriteLine($"[Parse]  {N:N0} packets: {sw.Elapsed.TotalMilliseconds:F1} ms  ({sw.Elapsed.TotalNanoseconds / N:F0} ns/packet), ok={ok}, allocated={alloc} B ({(double)alloc / N:F2} B/packet)");
}

// ---------- 2) Pipeline.Process ----------
{
    var profile = WheelProfile.CreateDefault() with
    {
        Smoothing = 0.3,
        SteeringCalibration = new ApexLink.Core.Processing.SteeringCalibration { RawMin = 1000, RawCenter = 2050, RawMax = 3100, IsSet = true },
    };
    var pipeline = new ApexLink.Core.Processing.ProcessingPipeline(profile);
    const int N = 200_000;
    long allocBefore = GC.GetTotalAllocatedBytes(true);
    var sw = Stopwatch.StartNew();
    double sink = 0;
    for (int n = 0; n < N; n++)
    {
        var pkt = new WheelPacket((ushort)n, (uint)(n * 5000), (ushort)(2050 + (n % 500) - 250), 1200, 900, (byte)(n % 3 == 0 ? 1 : 0), true);
        sink += pipeline.Process(pkt, 0.005).SteeringAxis;
    }
    sw.Stop();
    long alloc = GC.GetTotalAllocatedBytes(true) - allocBefore;
    Console.WriteLine($"[Pipe]   {N:N0} ticks: {sw.Elapsed.TotalMilliseconds:F1} ms  ({sw.Elapsed.TotalNanoseconds / N:F0} ns/tick), allocated={alloc} B ({(double)alloc / N:F2} B/tick), sink={sink:F0}");
}

// ---------- 3) End-to-end UDP (loopback) ----------
{
    const int port = 44221;
    using var source = new UdpWheelSource(port);
    source.Start();
    using var sender = new UdpClient();
    var ep = new IPEndPoint(IPAddress.Loopback, port);
    int sent = 0;
    bool udpBlocked = false;
    var sendTask = Task.Run(async () =>
    {
        ushort seq = 0;
        var sw = Stopwatch.StartNew();
        long next = 0;
        while (sw.Elapsed.TotalSeconds < 10)
        {
            var pkt = new WheelPacket(seq++, (uint)(seq * 5000), 2050, 1000, 1000, 0, true);
            try { await sender.SendAsync(pkt.ToBytes(), WheelPacket.PacketSize, ep); sent++; }
            catch (SocketException) { udpBlocked = true; return; }
            next += 5;
            long wait = next - sw.ElapsedMilliseconds;
            if (wait > 0) await Task.Delay((int)wait);
        }
    });

    long maxAge = 0; double ageSum = 0; int ageSamples = 0;
    var sampleSw = Stopwatch.StartNew();
    while (!sendTask.IsCompleted)
    {
        if (source.TryGetLatest(out _, out long age, long.MaxValue) && age != long.MaxValue)
        {
            maxAge = Math.Max(maxAge, age); ageSum += age; ageSamples++;
        }
        await Task.Delay(50);
    }
    await sendTask;
    if (udpBlocked)
    {
        Console.WriteLine("[E2E]    SKIP — is mahol mein UDP loopback band hai (VM policy). Windows par chalega.");
    }
    else
    {
        var st = source.Statistics;
        Console.WriteLine($"[E2E]    10s @200pps: sent={sent}, valid={st.ValidPackets}, lost={st.LostPackets}, badCrc={st.BadCrc}, latest-age avg={ageSum / Math.Max(1, ageSamples):F1} ms max={maxAge} ms");
    }
}

// ---------- 4) Engine soak ----------
{
    var src = new BenchSource();
    var output = new BenchOutput();
    using var engine = new WheelEngine(src, output, WheelProfile.CreateDefault());
    engine.Start();
    var proc = Process.GetCurrentProcess();

    Thread.Sleep(5000);   // warm-up — is ke baad ke numbers gin'ne hain
    long alloc0 = GC.GetTotalAllocatedBytes(true);
    long heap0 = GC.GetTotalMemory(false);
    long ws0 = proc.WorkingSet64;
    int gen2_0 = GC.CollectionCount(2);
    long ticks0 = engine.TickCount;

    var soakSw = Stopwatch.StartNew();
    long wsPeak = 0;
    while (soakSw.Elapsed.TotalSeconds < soakSec)
    {
        Thread.Sleep(2000);
        proc.Refresh();
        wsPeak = Math.Max(wsPeak, proc.WorkingSet64);
    }
    engine.Stop();

    long ticks = engine.TickCount - ticks0;
    long alloc = GC.GetTotalAllocatedBytes(true) - alloc0;
    proc.Refresh();
    Console.WriteLine($"[Soak]   {soakSec}s: ticks={ticks} (~{ticks / soakSec}/s), avg tick={engine.TotalTickDurationUs / Math.Max(1, engine.TickCount)} us, max tick={engine.MaxTickDurationUs} us, late={engine.LateTickCount}");
    Console.WriteLine($"         allocated/tick={(double)alloc / Math.Max(1, ticks):F1} B, Gen2 collections={GC.CollectionCount(2) - gen2_0}, heap {heap0 / 1048576.0:F1} MB -> {GC.GetTotalMemory(false) / 1048576.0:F1} MB, working set {ws0 / 1048576.0:F1} MB -> peak {wsPeak / 1048576.0:F1} MB");
    Console.WriteLine("         (Budgets: idle WS <=120 MB, race WS <=150 MB — ye numbers is VM ke hain; asal faisla tumhare PC par inhi benchmarks se hoga.)");
}

Console.WriteLine("=== Benchmarks khatam ===");

internal sealed class BenchSource : ISampleSource
{
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private ushort _seq;
    public LinkStatistics Statistics { get; } = new();
    public long? LastValidAgeMs => 5;
    public bool TryGetLatest(out WheelPacket packet, out long ageMs, long maxAgeMs = long.MaxValue)
    {
        long t = _clock.ElapsedMilliseconds;
        packet = new WheelPacket(_seq++, (uint)(t * 1000), (ushort)(2050 + Math.Sin(t / 500.0) * 800), 1200, 900, 0, true);
        ageMs = 5;
        return ageMs <= maxAgeMs;
    }
}

internal sealed class BenchOutput : IJoystickOutput
{
    public string Name => "Bench";
    public bool IsConnected { get; private set; }
    public long Updates;
    public bool TryConnect(out string error) { IsConnected = true; error = ""; return true; }
    public void Disconnect() => IsConnected = false;
    public void Update(in ProcessedState state) => Updates++;
    public void SendNeutral() { }
    public void Dispose() { }
}
