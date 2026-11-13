using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using RSH.Core.Protocol;

// RSH Simulator — asal ESP ki jagah lene wala nakli wheel.
// Firmware ki tarah bartao: port 4210 par HELO sunta hai; HELO taaza ho
// (2 s ke andar) to usi PC ko port 4211 par 200 packets/sec bhejta hai.
//   --free-run     HELO ka intezar kiye baghair 127.0.0.1 par bhejte raho
//   --dropout-every N / --dropout-ms M   har N sec baad M ms khamoshi
//                  (link tootne ki mashq — watchdog test ke liye)

bool freeRun = args.Contains("--free-run");
int dropoutEverySec = GetIntArg("--dropout-every", 0);
int dropoutMs = GetIntArg("--dropout-ms", 0);

int GetIntArg(string name, int fallback)
{
    int i = Array.IndexOf(args, name);
    return i >= 0 && i + 1 < args.Length && int.TryParse(args[i + 1], out int v) ? v : fallback;
}

using var helloListener = new UdpClient(4210);
helloListener.Client.ReceiveTimeout = 50;
using var sender = new UdpClient();

IPAddress? pcIp = freeRun ? IPAddress.Loopback : null;
long lastHelloMs = 0;
ushort seq = 0;
var clock = Stopwatch.StartNew();
long startMs = clock.ElapsedMilliseconds;
long nextSendMs = startMs;

Console.WriteLine(freeRun
    ? "Simulator: FREE-RUN — 127.0.0.1:4211 par 200 pps ja raha hai. Band karne ke liye Ctrl+C."
    : "Simulator: HELO ka intezar hai (port 4210)… app start karo ya HELO bhejo.");
Console.CancelKeyPress += (_, e) => { e.Cancel = false; };

while (true)
{
    // HELO suno (jab mojood ho)
    try
    {
        if (helloListener.Available > 0)
        {
            var ep = new IPEndPoint(IPAddress.Any, 0);
            byte[] data = helloListener.Receive(ref ep);
            if (data.Length >= 4 && data[0] == (byte)'H' && data[1] == (byte)'E' && data[2] == (byte)'L' && data[3] == (byte)'O')
            {
                if (pcIp is null || !pcIp.Equals(ep.Address))
                    Console.WriteLine($"HELO mila: {ep.Address} — data shuru.");
                pcIp = ep.Address;
                lastHelloMs = clock.ElapsedMilliseconds;
            }
        }
    }
    catch (SocketException) { /* ek kharab lamha — aage barho */ }

    long now = clock.ElapsedMilliseconds;
    bool heloActive = freeRun || (pcIp is not null && now - lastHelloMs < 2000);

    // Dropout mashq: cycle ke aakhri dropoutMs khamoshi
    bool inDropout = false;
    if (dropoutEverySec > 0 && dropoutMs > 0)
    {
        long cycle = (now - startMs) % (dropoutEverySec * 1000L);
        inDropout = cycle >= dropoutEverySec * 1000L - dropoutMs;
    }

    if (heloActive && !inDropout && now >= nextSendMs && pcIp is not null)
    {
        double t = (now - startMs) / 1000.0;

        // Steering: 20 s ka cycle — center, sweep, kinare pakarna
        double phase = t % 20.0;
        double steerNorm;   // -1..1
        if (phase < 3) steerNorm = 0;
        else if (phase < 13) steerNorm = Math.Sin((phase - 3) / 10.0 * Math.PI * 2);
        else if (phase < 16) steerNorm = -1;
        else steerNorm = 1;
        ushort steeringMv = (ushort)Math.Round(2050 + steerNorm * 1150); // ~900..3200 mV

        // Pedals: bari bari ramp — pehle accel, phir brake
        double pedalPhase = t % 8.0;
        double accelNorm = pedalPhase < 4 ? Math.Abs(Math.Sin(pedalPhase / 4.0 * Math.PI)) : 0;
        double brakeNorm = pedalPhase >= 4 ? Math.Abs(Math.Sin((pedalPhase - 4) / 4.0 * Math.PI)) : 0;
        ushort accel = (ushort)Math.Round(300 + accelNorm * 2300);
        ushort brake = (ushort)Math.Round(300 + brakeNorm * 2300);

        // Buttons: sweep ke doran gear up/down ki jhalak
        byte buttons = 0;
        if (phase is >= 5 and < 5.3) buttons |= WheelPacket.ButtonGearUp;
        if (phase is >= 10 and < 10.3) buttons |= WheelPacket.ButtonGearDown;

        var packet = new WheelPacket(
            Sequence: seq++,
            EspTimestampUs: (uint)(clock.Elapsed.TotalMicroseconds),
            Steering: steeringMv,
            Accel: accel,
            Brake: brake,
            Buttons: buttons,
            SteeringInMillivolts: true);

        byte[] bytes = packet.ToBytes();
        await sender.SendAsync(bytes, bytes.Length, new IPEndPoint(pcIp, 4211));
        nextSendMs += 5;
        if (nextSendMs < now - 50) nextSendMs = now + 5;   // peeche reh jaye to resync
    }
    else
    {
        Thread.Sleep(1);
    }
}
