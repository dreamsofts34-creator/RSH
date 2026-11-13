// RSH control channel transport (PROTOCOL-V2.1.md §3.1/§3.2, port 4212).
// IPingTransport (NtpPing.cs) jaisa pattern: asal UDP transport + tests
// ke liye fake transport. Socket bind nahi hota — OS ephemeral port
// deta hai (T10). Control frames HMAC-signed hote hain — signing yahan
// nahi, ControlClient mein hai; transport sirf bytes bhejta/receive karta hai.
using System.Net.Sockets;

namespace RSH.Core.Transport;

/// <summary>Ek control exchange: frame bhejo, reply ka intezar karo. Timeout ya koi reply na aaye to null.</summary>
public interface IControlTransport : IDisposable
{
    Task<byte[]?> ExchangeAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken);
}

/// <summary>Asal UDP transport: board ke control port (4212) par connected socket (ephemeral source port).</summary>
public sealed class UdpControlTransport : IControlTransport
{
    private readonly UdpClient _udp;

    public UdpControlTransport(string boardHost, int boardPort = Protocol.ProtocolV2.ControlPort)
    {
        _udp = new UdpClient();
        _udp.Connect(boardHost, boardPort);
    }

    public async Task<byte[]?> ExchangeAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken)
    {
        try
        {
            await _udp.SendAsync(frame.AsMemory(), cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException) { return null; }
        catch (SocketException) { return null; }
        catch (ObjectDisposedException) { return null; }

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(timeout);
        try
        {
            var result = await _udp.ReceiveAsync(timeoutCts.Token).ConfigureAwait(false);
            return result.Buffer;
        }
        catch (OperationCanceledException) { return null; }
        catch (SocketException) { return null; }
        catch (ObjectDisposedException) { return null; }
    }

    public void Dispose() => _udp.Dispose();
}
