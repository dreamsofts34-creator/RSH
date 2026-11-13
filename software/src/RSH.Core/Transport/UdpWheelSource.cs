using System.Net;
using System.Net.Sockets;
using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>
/// ESP se data lene wala hissa: UDP :4211 par sunta hai, har datagram
/// parse karta hai, aur sirf SAB SE NAYA valid packet rakhta hai
/// (latest-sample slot — purana kaam ka nahi, queue kabhi barhti nahi).
/// Pipeline is se TryGetLatest se padhta hai jab chahe.
/// </summary>
public sealed class UdpWheelSource : ISampleSource, IDisposable
{
    public const int DefaultListenPort = 4211;

    private readonly UdpClient _udp;
    private readonly LinkStatistics _stats = new();
    private readonly object _slotLock = new();
    private WheelPacket _latest;
    private bool _hasLatest;
    private long _lastValidMs;
    private CancellationTokenSource? _cts;
    private Task? _receiveTask;

    public UdpWheelSource(int listenPort = DefaultListenPort)
    {
        _udp = new UdpClient(listenPort);
    }

    public LinkStatistics Statistics => _stats;

    /// <summary>Receive loop background mein shuru karo.</summary>
    public void Start()
    {
        if (_receiveTask is not null) return;
        _cts = new CancellationTokenSource();
        _receiveTask = Task.Run(() => ReceiveLoopAsync(_cts.Token));
    }

    private async Task ReceiveLoopAsync(CancellationToken token)
    {
        while (!token.IsCancellationRequested)
        {
            UdpReceiveResult result;
            try
            {
                result = await _udp.ReceiveAsync(token).ConfigureAwait(false);
            }
            catch (OperationCanceledException) { break; }
            catch (SocketException)
            {
                // AUDIT FIX: lagataar socket error par garam loop (100% CPU)
                // ki bajaye chhota sa waqfa — error ki wajah jo bhi ho,
                // receive dobara koshish hoti rahegi.
                try { await Task.Delay(100, token).ConfigureAwait(false); }
                catch (OperationCanceledException) { break; }
                catch (ObjectDisposedException) { break; }   // Dispose ke doran CTS gaya to khamoshi se ruk jao
                continue;
            }
            catch (ObjectDisposedException) { break; }

            ProcessDatagram(result.Buffer);
        }
    }

    /// <summary>
    /// Ek datagram ka poora amal: parse, stats, latest slot.
    /// Receive loop yahi bulata hai; tests baghair socket ke yahi
    /// istemal karte hain (har mahol mein UDP loopback dastiyab nahi).
    /// </summary>
    internal void ProcessDatagram(ReadOnlySpan<byte> data)
    {
        var parse = WheelPacket.TryParse(data, out var packet);
        if (parse == PacketParseResult.Ok)
        {
            lock (_slotLock)
            {
                // AUDIT FIX: 5 s se zyada khamoshi ke baad pehla packet —
                // link naye sire se bana hai (masalan ESP reboot). Sequence
                // baseline saaf karo warna ye packet bari "loss" gin liya
                // jata tha aur LossPercent hamesha ke liye bigar jata tha.
                if (_hasLatest && Environment.TickCount64 - _lastValidMs > 5000)
                    _stats.ResetSequenceBaseline();
                _latest = packet;
                _hasLatest = true;
                _lastValidMs = Environment.TickCount64;
            }
        }
        _stats.Record(parse, packet.Sequence);
    }

    /// <summary>
    /// Sab se naya valid packet + us ki umar (ms). Data na ho ya
    /// <paramref name="maxAgeMs"/> se purana ho to false (watchdog yahi
    /// istemal karega).
    /// </summary>
    public bool TryGetLatest(out WheelPacket packet, out long ageMs, long maxAgeMs = long.MaxValue)
    {
        lock (_slotLock)
        {
            packet = _latest;
            if (!_hasLatest)
            {
                ageMs = long.MaxValue;
                return false;
            }
            ageMs = Environment.TickCount64 - _lastValidMs;
            return ageMs <= maxAgeMs;
        }
    }

    /// <summary>Aakhri valid packet ko kitni der hui (kabhi na aya ho to null).</summary>
    public long? LastValidAgeMs
    {
        get
        {
            lock (_slotLock)
                return _hasLatest ? Environment.TickCount64 - _lastValidMs : null;
        }
    }

    public void Dispose()
    {
        _cts?.Cancel();
        _udp.Dispose();
        // AUDIT FIX: receive task ke khatam hone ka intezar kiye baghair
        // CTS dispose ho jata tha — loop ke Task.Delay par
        // ObjectDisposedException task ko fault kar deta tha (unobserved).
        try { _receiveTask?.Wait(500); } catch { /* band hote waqt khamoshi */ }
        _cts?.Dispose();
    }
}
