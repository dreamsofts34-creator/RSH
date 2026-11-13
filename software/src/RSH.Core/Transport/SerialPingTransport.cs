// RSH wired USB-serial transport — ping transport (Phase G, D26 /
// OPEN-20). IPingTransport ka serial implementation: NTP-style ping
// frame ("RSHN", §9) SerialLink ke zariye jata hai — wahi NtpPingClient,
// wahi SLO (E2E p99 ≤ 5 ms wired, §3). UDP transport (UdpPingTransport)
// jaisa hi contract: ExchangeAsync timeout par null.
namespace RSH.Core.Transport;

/// <summary>Serial ping transport: ping request bhejo, reply ka intezar karo. Timeout ya koi reply na aaye to null.</summary>
public sealed class SerialPingTransport : IPingTransport
{
    private readonly SerialLink _link;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private bool _disposed;

    /// <summary><paramref name="link"/> ki malkiyat nahi leta — link SerialWheelSource ya AppServices ka rehta hai.</summary>
    public SerialPingTransport(SerialLink link)
    {
        ArgumentNullException.ThrowIfNull(link);
        _link = link;
    }

    /// <summary>Link (tests ke liye).</summary>
    public SerialLink Link => _link;

    public async Task<byte[]?> ExchangeAsync(byte[] request, TimeSpan timeout, CancellationToken cancellationToken)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        await _gate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            return await _link.ExchangePingAsync(request, timeout, cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            _gate.Release();
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _gate.Dispose();
    }
}
