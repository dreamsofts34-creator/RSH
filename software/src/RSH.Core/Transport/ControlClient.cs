// RSH signed control client (PROTOCOL-V2.1.md §3.2/§3.3) — GET_CAPS,
// REBOOT aur baaqi HMAC-signed commands ka ek hi rasta.
//
// Rules (§3.2), sab yahan laagu:
//  - Har request 3× tak retry (250 ms timeout), har retry mein reqId
//    WAHI, counter NAYA (retry naya execution hai — commands idempotent
//    honi chahiye).
//  - Reply tab hi maani jati hai jab: structure OK ho, cmd = req|0x80 ho,
//    reqId/counter match karein, aur HMAC tag verify ho. BAD_TAG reply
//    ko sirf tab maanta hai jab reqId/counter match karein (§3.2) —
//    is liye structure pehle, tag baad mein check hota hai.
//  - Tag fail (ghalat key/nonce) par foran haar — retry se key theek
//    nahi hoti.
//  - Key/nonce kabhi log nahi hote, kabhi hard-code nahi — caller
//    (BoardSession) se aate hain. Key store (DPAPI, Phase E WS5:
//    RSH.Core.Security.KeyStore) se key aaye to bhi yahan sirf memory
//    mein rehti hai; Dispose par saaf ho jati hai.
using System.Text;
using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>Signed control command ka nateeja: reply payload (status byte samet) ya null + wajah.</summary>
public sealed record ControlResult(
    byte[]? ReplyPayload,
    ControlStatus? ReplyStatus,
    string? Error,
    int Attempts)
{
    public bool Succeeded => ReplyPayload is not null && ReplyStatus == ControlStatus.Ok;
}

/// <summary>
/// HMAC-signed control commands (port 4212). Ek instance = ek session
/// (ek nonce): counter per-session monotonic hai, is liye instance ko
/// dobara istemal karo, har command par naya client mat banao.
/// </summary>
public sealed class ControlClient : IDisposable
{
    /// <summary>Ek request ka timeout (§3.2: 250 ms).</summary>
    public static readonly TimeSpan RequestTimeout = TimeSpan.FromMilliseconds(250);
    /// <summary>Ek request ki zyada se zyada koshishein (§3.2: 3×).</summary>
    public const int MaxAttempts = 3;

    private readonly IControlTransport _transport;
    private readonly byte[] _key;
    private readonly byte[] _nonce;
    private readonly bool _ownsTransport;
    private uint _nextCounter;
    private ushort _nextReqId;
    private bool _disposed;

    /// <summary>
    /// SEC-E-02: SET_PASSWORD rate limit — process-wide (BoardSession har
    /// call par naya ControlClient banata hai, is liye instance-level
    /// limit kabhi trigger nahi hoti). Ek SET_PASSWORD attempt ke 60 s
    /// ke andar doosri attempt reject hoti hai — board par brute-force /
    /// accidental double-send ka rasta band hai. Limit hit par saaf
    /// error milta hai (ControlResult.Error), khamoshi se drop nahi.
    /// Sirf asal send budget khata hai — validation fail (ArgumentException)
    /// se pehle check hota hai, is liye ghalat password budget nahi khata.
    /// </summary>
    public static readonly TimeSpan SetPasswordMinInterval = TimeSpan.FromSeconds(60);
    private static readonly object s_setPasswordRateLock = new();
    private static DateTimeOffset? s_lastSetPasswordAttemptUtc;
    /// <summary>Tests ke liye: rate-limit clock (default: asal waqt).</summary>
    internal static Func<DateTimeOffset> SetPasswordClockForTest { get; set; } = static () => DateTimeOffset.UtcNow;
    /// <summary>Tests ke liye: rate-limit state reset (clock bhi asal par wapas).</summary>
    internal static void ResetSetPasswordRateLimitForTest()
    {
        lock (s_setPasswordRateLock)
        {
            s_lastSetPasswordAttemptUtc = null;
            SetPasswordClockForTest = static () => DateTimeOffset.UtcNow;
        }
    }

    /// <summary>
    /// <paramref name="key"/> 32 B device key, <paramref name="nonce"/> 16 B
    /// session nonce (HELLO reply se). <paramref name="transport"/> null ho
    /// to <paramref name="boardHost"/>:<paramref name="boardPort"/> par
    /// asal UDP transport banta hai (ye instance us ki malik hoti hai).
    /// <paramref name="firstCounter"/>: session ka agla counter — ek
    /// session mein dobara client bane to counter yahin se jari rehta hai
    /// (warna naya client counter 0 se shuru karke REPLAY khata hai, §3.2).
    /// </summary>
    public ControlClient(string boardHost, byte[] key, byte[] nonce, IControlTransport? transport = null, int boardPort = ProtocolV2.ControlPort, uint firstCounter = 0)
    {
        ArgumentNullException.ThrowIfNull(key);
        ArgumentNullException.ThrowIfNull(nonce);
        if (key.Length != 32) throw new ArgumentException("Device key 32 bytes ki honi chahiye (§4.1).", nameof(key));
        if (nonce.Length != 16) throw new ArgumentException("Nonce 16 bytes ka hona chahiye (§2.3).", nameof(nonce));
        _key = key.ToArray();
        _nonce = nonce.ToArray();
        _transport = transport ?? new UdpControlTransport(boardHost, boardPort);
        _ownsTransport = transport is null;
        _nextReqId = (ushort)Random.Shared.Next(1, 60000);
        _nextCounter = firstCounter;
    }

    /// <summary>Tests ke liye: pehla reqId fix karo (deterministic frames).</summary>
    internal ushort NextReqIdForTest { set => _nextReqId = value; }

    /// <summary>Agli request ka counter (session monotonic).</summary>
    public uint NextCounter => _nextCounter;

    /// <summary>SET_PASSWORD password rule (§5.2, firmware password.h jaisa): 8..63 chars, printable ASCII 0x21–0x7E (space nahi).</summary>
    public const int MinPasswordLength = 8;
    /// <summary>SET_PASSWORD password rule (§5.2): zyada se zyada 63 chars (WPA2-PSK ki had bhi yehi hai).</summary>
    public const int MaxPasswordLength = 63;

    /// <summary>
    /// Naya password validate karo (firmware rsh_password_validate jaisa rule).
    /// Valid ho to null, warna user ko dikhane qaabil wajah. Wave 2 dialog
    /// bhi yahi istemal karega (board tak ghalat password kabhi nahi jata).
    /// </summary>
    public static string? ValidateNewPassword(string? password)
    {
        if (password is null) return "Password nahi diya gaya.";
        if (password.Length < MinPasswordLength || password.Length > MaxPasswordLength)
            return $"Password {MinPasswordLength}..{MaxPasswordLength} characters ka hona chahiye (§5.2).";
        foreach (char ch in password)
            if (ch < 0x21 || ch > 0x7E)
                return "Password mein sirf printable ASCII ho sakta hai (0x21–0x7E, space nahi) (§5.2).";
        return null;
    }

    /// <summary>
    /// SET_PASSWORD (cmd 0x0C, §5.2/§3.3) bhejo: payload = newLen u8 +
    /// password ASCII bytes. Ghalat password par <see cref="ArgumentException"/>
    /// (board tak kuch nahi jata). Kamyabi par board password AP + WebOTA
    /// dono ke liye apply karta hai aur AP restart karta hai — connected
    /// clients drop honge, dobara naye password se join karna hoga. Reply
    /// OK restart se pehle aata hai (§5.2).
    /// SEC-E-02: pichli SET_PASSWORD attempt ke 60 s ke andar call ho to
    /// board tak kuch nahi jata — ControlResult mein saaf rate-limit
    /// error milta hai (khamoshi se drop nahi, exception bhi nahi —
    /// BoardSession ise (false, wajah) banata hai).
    /// </summary>
    public async Task<ControlResult> SetPasswordAsync(string newPassword, CancellationToken cancellationToken = default)
    {
        string? error = ValidateNewPassword(newPassword);
        if (error is not null) throw new ArgumentException(error, nameof(newPassword));
        lock (s_setPasswordRateLock)
        {
            if (s_lastSetPasswordAttemptUtc is { } last &&
                SetPasswordClockForTest() - last < SetPasswordMinInterval)
            {
                double waitSeconds = (SetPasswordMinInterval - (SetPasswordClockForTest() - last)).TotalSeconds;
                return new ControlResult(null, null,
                    $"SET_PASSWORD rate limit: pichli koshish ke {SetPasswordMinInterval.TotalSeconds:F0} s ke andar dobara password nahi badla ja sakta — {Math.Ceiling(waitSeconds)} s intezar karo, phir dobara koshish karo (SEC-E-02).",
                    0);
            }
            s_lastSetPasswordAttemptUtc = SetPasswordClockForTest();
        }
        byte[] payload = new byte[1 + newPassword.Length];
        payload[0] = (byte)newPassword.Length;
        Encoding.ASCII.GetBytes(newPassword, payload.AsSpan(1));
        try
        {
            return await SendCommandAsync(ControlCommand.SetPassword, payload, cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            Array.Clear(payload);
        }
    }

    /// <summary>
    /// Ek signed command bhejo, reply ka intezar karo. Reply payload
    /// (status byte samet) mile to <see cref="ControlResult.Succeeded"/>
    /// sirf tab true jab status OK ho — status != OK bhi ek (na-kaam)
    /// reply hai, timeout nahi.
    /// </summary>
    public async Task<ControlResult> SendCommandAsync(ControlCommand command, ReadOnlyMemory<byte> payload, CancellationToken cancellationToken = default)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        ushort reqId = _nextReqId++;
        if (_nextReqId == 0) _nextReqId = 1; // reqId 0 se bacho (wrap par)
        string? lastError = null;

        for (int attempt = 1; attempt <= MaxAttempts; attempt++)
        {
            uint counter = _nextCounter++;
            byte[] frame = ControlFrameCodec.Build((byte)command, reqId, counter, payload.Span, _key, _nonce);
            byte[]? replyBytes;
            try
            {
                replyBytes = await _transport.ExchangeAsync(frame, RequestTimeout, cancellationToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                return new ControlResult(null, null, "Control request cancel hui.", attempt);
            }
            if (replyBytes is null)
            {
                lastError = $"Board se {RequestTimeout.TotalMilliseconds} ms mein jawab nahi aaya (koshish {attempt}/{MaxAttempts}).";
                continue;
            }

            // Pehle structure (BAD_TAG reply ka reqId/counter parhne ke liye),
            // phir reqId/counter/cmd match, phir tag verify (§3.2).
            var structure = ControlFrameCodec.TryParseStructure(replyBytes, out var reply);
            if (structure != ControlParseResult.Ok)
            {
                lastError = $"Reply ka structure ghalat hai ({structure}) — dobara koshish nahi (board ne dobara execute nahi karna).";
                return new ControlResult(null, null, lastError, attempt);
            }
            if (reply.Cmd != ((byte)command | 0x80) || reply.ReqId != reqId || reply.Counter != counter)
            {
                // Stale/ghalat reply (kisi aur request ki — UDP par purani
                // reply der se aa sakti hai): is request ki nahi hai, is
                // liye IGNORE karo aur dobara koshish karo (§3.2 "ignore")
                // — pehle foran fail ho jata tha, ek stale datagram poora
                // command maar deta tha. Retry naya counter bhejta hai,
                // is liye board ise naya execution samajhta hai.
                lastError = "Reply is request se match nahi karti (cmd/reqId/counter) — ghalat/paraani reply, ignore ki gayi.";
                continue;
            }
            var verified = ControlFrameCodec.TryParse(replyBytes, _key, _nonce, out reply);
            if (verified == ControlParseResult.BadTag)
            {
                // Key/nonce ghalat — retry se theek nahi hoga.
                return new ControlResult(null, null, "Reply ka HMAC tag verify nahi hua (BAD_TAG) — key ya nonce ghalat hai.", attempt);
            }
            if (verified != ControlParseResult.Ok)
            {
                lastError = $"Reply verify nahi hui ({verified}).";
                return new ControlResult(null, null, lastError, attempt);
            }
            var status = reply.ReplyStatus;
            if (status is null)
                return new ControlResult(reply.Payload, null, "Reply payload khaali hai — status byte nahi.", attempt);
            return new ControlResult(reply.Payload, status, status == ControlStatus.Ok ? null : $"Board ne {status} ({(byte)status.Value}) diya.", attempt);
        }

        return new ControlResult(null, null, lastError ?? "Board se jawab nahi aaya.", MaxAttempts);
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        if (_ownsTransport) _transport.Dispose();
        Array.Clear(_key);
        Array.Clear(_nonce);
    }
}
