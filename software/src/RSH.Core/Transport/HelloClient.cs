using System.Net.Sockets;
using System.Text;

namespace RSH.Core.Transport;

/// <summary>
/// ESP ko zinda rakhne wala hissa: har 1 second "HELO" bhejta hai
/// (firmware 2 s tak intezar karta hai, phir khamosh ho jata hai).
/// Firmware v3 ka remote restart ("RSTX") bhi yahin se jata hai.
/// </summary>
public sealed class HelloClient : IDisposable
{
    public const string DefaultEspIp = "192.168.4.1";
    public const int EspHelloPort = 4210;

    private static readonly byte[] HelloBytes = Encoding.ASCII.GetBytes("HELO");
    private static readonly byte[] RestartBytes = Encoding.ASCII.GetBytes("RSTX");

    private readonly UdpClient _udp = new();
    private readonly string _espIp;
    private readonly int _espPort;
    private CancellationTokenSource? _cts;
    private Task? _loopTask;

    public HelloClient(string espIp = DefaultEspIp, int espPort = EspHelloPort)
    {
        _espIp = espIp;
        _espPort = espPort;
    }

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
                await _udp.SendAsync(HelloBytes.AsMemory(), _espIp, _espPort, token).ConfigureAwait(false);
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

    /// <summary>Firmware ko remote restart ka hukm (sirf v3 firmware samajhta hai).</summary>
    public async Task SendRestartAsync()
    {
        await _udp.SendAsync(RestartBytes, RestartBytes.Length, _espIp, _espPort).ConfigureAwait(false);
    }

    public void Dispose()
    {
        Stop();
        _udp.Dispose();
        _cts?.Dispose();
    }
}
