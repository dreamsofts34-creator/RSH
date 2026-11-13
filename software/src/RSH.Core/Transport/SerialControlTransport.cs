// RSH wired USB-serial transport — control transport (Phase G, D26 /
// OPEN-20). IControlTransport ka serial implementation: control frame
// ("RSHC", HMAC-signed — signing ControlClient mein hai, yahan nahi)
// SerialLink ke zariye jata hai aur reply wahi se aata hai. UDP
// transport (ControlTransport.cs) jaisa hi contract: ExchangeAsync
// timeout par null. Ek waqt mein ek hi exchange (SerialLink ka rule) —
// ControlClient sequential hai, is liye ye kabhi takrata nahi.
namespace RSH.Core.Transport;

/// <summary>Serial control transport: frame bhejo, reply ka intezar karo. Timeout ya koi reply na aaye to null.</summary>
public sealed class SerialControlTransport : IControlTransport
{
    private readonly SerialLink _link;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private bool _disposed;

    /// <summary><paramref name="link"/> ki malkiyat nahi leta — link SerialWheelSource ya AppServices ka rehta hai.</summary>
    public SerialControlTransport(SerialLink link)
    {
        ArgumentNullException.ThrowIfNull(link);
        _link = link;
    }

    /// <summary>Link (tests / BoardSession ke liye).</summary>
    public SerialLink Link => _link;

    public async Task<byte[]?> ExchangeAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        await _gate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            return await _link.ExchangeControlAsync(frame, timeout, cancellationToken).ConfigureAwait(false);
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
