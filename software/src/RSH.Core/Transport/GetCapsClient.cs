// RSH GET_CAPS client (PROTOCOL-V2.1.md §3.3 cmd 0x01, §3.4 body) —
// PC-side client: board id, limits, pin table, Wi-Fi channel parhe.
// SEC-C-05 (anti-downgrade) ke liye version compare ka source hai:
// upload se pehle image version ka muqabla isi se hota hai
// (FirmwareUploadGate), aur upload ke baad SEC-C-02 confirm bhi isi se.
// Transport/signing ControlClient karta hai — yahan sirf GET_CAPS
// ka matlab (payload → BoardCaps) hai.
using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>GET_CAPS client — ek session (ek nonce) ke liye ek instance istemal karo (counter monotonic hai).</summary>
public sealed class GetCapsClient : IDisposable
{
    private readonly ControlClient _control;
    private bool _disposed;

    /// <summary>Asal UDP transport ke saath client banao (key 32 B, nonce 16 B — HELLO reply / BoardSession se).</summary>
    public GetCapsClient(string boardHost, byte[] key, byte[] nonce, int boardPort = ProtocolV2.ControlPort)
        : this(new ControlClient(boardHost, key, nonce, boardPort: boardPort))
    {
    }

    /// <summary>Tests ke liye: fake transport/control de sakte hain.</summary>
    public GetCapsClient(ControlClient control)
    {
        _control = control;
    }

    /// <summary>Aakhri koshish ki wajah (null ho to kamyab ya abhi koshish nahi hui).</summary>
    public string? LastError { get; private set; }

    /// <summary>Session ka agla counter (BoardSession isi se counter jari rakhta hai — REPLAY se bachne ke liye).</summary>
    public uint NextCounter => _control.NextCounter;

    /// <summary>Kitni koshishein huin (aakhri call mein).</summary>
    public int LastAttempts { get; private set; }

    /// <summary>Board se capabilities mango. Na mile to null + <see cref="LastError"/> mein wajah.</summary>
    public async Task<BoardCaps?> GetCapsAsync(CancellationToken cancellationToken = default)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        var result = await _control.SendCommandAsync(ControlCommand.GetCaps, ReadOnlyMemory<byte>.Empty, cancellationToken).ConfigureAwait(false);
        LastAttempts = result.Attempts;
        if (result.ReplyPayload is null)
        {
            LastError = result.Error ?? "GET_CAPS ka jawab nahi aaya.";
            return null;
        }
        if (!BoardCaps.TryParse(result.ReplyPayload, out var caps, out string? parseError))
        {
            LastError = parseError ?? "GET_CAPS reply parse nahi hui.";
            return null;
        }
        LastError = null;
        return caps;
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _control.Dispose();
    }
}
