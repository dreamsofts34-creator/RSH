using System.Net;
using System.Net.Sockets;
using System.Text;

namespace RSH.Core.Transport;

/// <summary>
/// ESP ko zinda rakhne wala hissa: har 1 second HELO bhejta hai
/// (firmware 2 s tak intezar karta hai, phir khamosh ho jata hai).
///
/// SEC-D-08 (Phase D): HELO 6 byte ka hota hai — "HELO" + proto
/// <c>0x02</c> + sessionFlag (PROTOCOL-V2.1.md §2.1). Purana 4-byte
/// HELO (na proto, na sessionFlag) hijack ka rasta tha: firmware
/// pehle 4 bytes dekhte hi pcIp overwrite kar deta tha. Ab board
/// proto check karta hai (firmware W3) aur PC hamesha v2 HELO bhejta
/// hai: sessionFlag 0 = resume (purana nonce wapas), 1 = naya session.
///
/// SEC-D-01 (Phase D): unauthenticated RSTX restart HATA DIYA GAYA
/// hai — restart sirf HMAC-signed REBOOT (0x09) se hota hai, jo
/// <see cref="BoardSession.SendRebootAsync"/> karta hai (key+nonce
/// chahiye). Is class mein restart ka koi rasta nahi raha.
///
/// Do istemal:
/// <list type="bullet">
/// <item>Standalone: apna socket (ephemeral local port, T10) — jab
/// koi data socket na ho.</item>
/// <item>Delegate: <see cref="UdpWheelSource.SendHelloV2Async"/> jaisi
/// bahari sender — HELO data socket se jata hai (source port =
/// listen port), taake board (Phase D firmware, jo HELO ke
/// remotePort par data bhejega) data usi ephemeral port par bheje.
/// AppServices isi mode mein chalata hai.</item>
/// </list>
/// </summary>
public sealed class HelloClient : IDisposable
{
    public const string DefaultEspIp = "192.168.4.1";

    /// <summary>
    /// Board ka HELO port (destination). Ye board ki taraf ka
    /// well-known port hai (firmware PORT_HELLO) — PC ki taraf se
    /// ephemeral nahi ho sakta: board 4210 par sunta hai, is liye
    /// HELO hamesha yahin jata hai. Ephemeral sirf PC ka source
    /// port hota hai (dekho <see cref="LocalPort"/>).
    /// </summary>
    public const int EspHelloPort = 4210;

    /// <summary>
    /// v2 HELO frame (6 B, §2.1): "HELO" | proto 0x02 | sessionFlag
    /// (0 = resume, 1 = naya session). 4-byte HELO legacy v1 hai —
    /// us ka koi reply nahi aata aur firmware use hijack kar sakta
    /// tha (SEC-D-08), is liye naya code hamesha yahi bhejta hai.
    /// </summary>
    public static byte[] BuildHello(bool newSession = false) =>
        [(byte)'H', (byte)'E', (byte)'L', (byte)'O', 0x02, (byte)(newSession ? 1 : 0)];

    private static readonly byte[] HelloBytes = BuildHello(newSession: false);

    private readonly UdpClient? _udp;
    private readonly Func<CancellationToken, Task>? _sendHelloAsync;
    private readonly string _espIp;
    private readonly int _espPort;
    private CancellationTokenSource? _cts;
    private Task? _loopTask;

    /// <summary>
    /// Standalone HELO sender: apna socket, source port
    /// <paramref name="localPort"/> (0 = ephemeral — OS deta hai).
    /// </summary>
    public HelloClient(string espIp = DefaultEspIp, int espPort = EspHelloPort, int localPort = 0)
    {
        if (espPort is < 1 or > 65535)
            throw new ArgumentOutOfRangeException(nameof(espPort), "Board ka HELO port 1..65535 ke andar ho.");
        if (localPort is < 0 or > 65535)
            throw new ArgumentOutOfRangeException(nameof(localPort), "Local port 0..65535 ke andar ho (0 = ephemeral).");
        _espIp = espIp;
        _espPort = espPort;
        _udp = new UdpClient(localPort);
    }

    /// <summary>
    /// Delegate HELO sender: har tick par <paramref name="sendHelloAsync"/>
    /// bulaya jata hai (misal data socket se — source port = listen port).
    /// Is mode mein is class ka koi socket nahi hota (<see cref="LocalPort"/> -1).
    /// </summary>
    public HelloClient(Func<CancellationToken, Task> sendHelloAsync, string espIp = DefaultEspIp, int espPort = EspHelloPort)
    {
        ArgumentNullException.ThrowIfNull(sendHelloAsync);
        if (espPort is < 1 or > 65535)
            throw new ArgumentOutOfRangeException(nameof(espPort), "Board ka HELO port 1..65535 ke andar ho.");
        _sendHelloAsync = sendHelloAsync;
        _espIp = espIp;
        _espPort = espPort;
    }

    /// <summary>Board ka IP (HELO isi ko jata hai).</summary>
    public string EspIp => _espIp;
    /// <summary>Board ka HELO port (4210).</summary>
    public int EspPort => _espPort;

    /// <summary>Asal local (source) port — HELO isi port se jata hai. Delegate mode mein -1 (koi socket nahi).</summary>
    public int LocalPort => _udp is null ? -1 : ((IPEndPoint)_udp.Client.LocalEndPoint!).Port;

    public bool IsRunning => _loopTask is { IsCompleted: false };
    private long _hellosSent;
    // AUDIT FIX: plain ++ loop thread par aur read UI thread se — 64-bit
    // counter ka ghair-atomic adla-badli. Ab Interlocked.
    public long HellosSent => Interlocked.Read(ref _hellosSent);

    public void Start()
    {
        if (IsRunning) return;
        _cts = new CancellationTokenSource();
        _loopTask = Task.Run(() => LoopAsync(_cts.Token));
    }

    public void Stop()
    {
        _cts?.Cancel();
    }

    private async Task LoopAsync(CancellationToken token)
    {
        while (!token.IsCancellationRequested)
        {
            try
            {
                if (_sendHelloAsync is not null)
                    await _sendHelloAsync(token).ConfigureAwait(false);
                else
                    await _udp!.SendAsync(HelloBytes.AsMemory(), _espIp, _espPort, token).ConfigureAwait(false);
                Interlocked.Increment(ref _hellosSent);
            }
            catch (OperationCanceledException) { break; }
            catch (ObjectDisposedException) { break; }   // AUDIT FIX: Dispose ke doran socket gaya to task fault na ho
            catch (SocketException) { /* network na ho to agle second phir koshish */ }

            try { await Task.Delay(1000, token).ConfigureAwait(false); }
            catch (OperationCanceledException) { break; }
            catch (ObjectDisposedException) { break; }
        }
    }

    public void Dispose()
    {
        Stop();
        _udp?.Dispose();
        _cts?.Dispose();
    }
}
