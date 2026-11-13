// RSH board session (PROTOCOL-V2.1.md §2.2/§2.3, §4) — PC-side
// HELLO/control session ki halat: nonce + device key.
//
// Nonce har HELLO reply se aata hai (UpdateFromHello). Key sirf
// unpaired + pairing-window-ke-andar HELLO mein aati hai — paired
// board key ZERO bhejta hai, is liye HasKey aksar false rehta hai.
// Phase E WS5: key ab DPAPI key store (RSH.Core.Security.KeyStore,
// SEC-D-05 CLOSED) mein rehti hai — pairing ke waqt HELLO se mili key
// foran store mein save hoti hai, aur paired board se dobara milne par
// (HELLO mein key zero) session store se key le leta hai. Is liye
// GET_CAPS/REBOOT/SET_PASSWORD pairing window ke bahar bhi kaam karte
// hain. Non-Windows par store sirf in-memory hai (restart par key kho
// jati hai — dobara pairing lazmi hai). Key kabhi log/disk-plain mein
// nahi aati; Dispose par memory saaf hoti hai.
// Key store aane se pehle GET_CAPS/REBOOT sirf us waqt kaam karte thay
// jab session key maujood ho (pairing window) — wo had ab khatam hai.
using RSH.Core.Protocol;
using RSH.Core.Security;

namespace RSH.Core.Transport;

/// <summary>Board session: HELLO se nonce/key, aur un se signed control commands (GET_CAPS, REBOOT, SET_PASSWORD).</summary>
public sealed class BoardSession : IDisposable
{
    private byte[]? _key;
    private byte[]? _nonce;
    private uint _nextCounter;
    private bool _disposed;
    private readonly IKeyStore _keyStore;
    private readonly bool _ownsKeyStore;

    /// <summary>
    /// Board session banao. <paramref name="keyStore"/> null ho to default
    /// store (Windows: DPAPI; non-Windows: in-memory) banta hai aur is
    /// session ki malkiyat mein rehta hai (Dispose par band hota hai).
    /// </summary>
    public BoardSession(IKeyStore? keyStore = null)
    {
        _keyStore = keyStore ?? new KeyStore();
        _ownsKeyStore = keyStore is null;
    }

    /// <summary>Key store (device key + Wi-Fi/OTA password) — tests / Wave 2 ke liye.</summary>
    public IKeyStore KeyStore => _keyStore;

    /// <summary>Session key maujood hai? (Sirf unpaired HELLO se aati hai — paired board zero bhejta hai.)</summary>
    public bool HasKey => _key is not null && _key.AsSpan().IndexOfAnyExcept((byte)0) >= 0;
    /// <summary>Session nonce maujood hai? (Har HELLO reply se aata hai.)</summary>
    public bool HasNonce => _nonce is not null;
    /// <summary>
    /// Signed control commands bheje ja sakte hain? (Key + nonce dono chahiye —
    /// key session se ya key store se aa sakti hai.) Pehli dafa store se key
    /// milne par wo session key ban jati hai (promote), taake baar-baar
    /// store na parhna pare.
    /// </summary>
    public bool CanSendControl => !_disposed && HasNonce && TryGetKey(out _);
    /// <summary>Aakhri HELLO reply ka board MAC (pairing/binding ke liye, D21) — null jab HELLO na aaya ho.</summary>
    public byte[]? BoardMac { get; private set; }
    /// <summary>Aakhri HELLO reply ka firmware version — null jab HELLO na aaya ho.</summary>
    public Version? HelloFirmwareVersion { get; private set; }

    /// <summary>
    /// Wi-Fi/OTA password key store se nikalo (SEC-E-04 — PROTOCOL §5.3:
    /// WebOTA auth ke liye app ko password chahiye; OS Wi-Fi profile se
    /// PSK wapas nahi milta, is liye DPAPI blob hi ghar hai). BoardMac
    /// na ho (abhi HELLO nahi aaya) ya blob na ho to false. Password
    /// kabhi settings.json se nahi parha jata.
    /// </summary>
    public bool TryGetWifiPassword(out string? password)
    {
        password = null;
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        if (BoardMac is null) return false;
        try
        {
            return _keyStore.TryLoadPassword(RSH.Core.Security.KeyStore.BoardIdForMac(BoardMac), out password);
        }
        catch (ObjectDisposedException) { return false; }
        catch (InvalidOperationException) { return false; }
    }

    /// <summary>
    /// Wi-Fi/OTA password key store mein save karo (misal Connection page
    /// par user ne jo password darj kar ke Wi-Fi join kiya). BoardMac na
    /// ho to false (caller in-memory rakhe, HELLO par migrate hoga).
    /// Ghalat format (§5.2: 8–63, printable ASCII) store nahi hota.
    /// </summary>
    public bool TrySaveWifiPassword(string password)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        ArgumentNullException.ThrowIfNull(password);
        if (ControlClient.ValidateNewPassword(password) is not null) return false;
        if (BoardMac is null) return false;
        try
        {
            return _keyStore.TrySavePassword(RSH.Core.Security.KeyStore.BoardIdForMac(BoardMac), password);
        }
        catch (ObjectDisposedException) { return false; }
        catch (InvalidOperationException) { return false; }
    }

    /// <summary>
    /// Purani jagah (settings.json plaintext — SEC-E-04 migration) se mila
    /// password key store mein lao. True = legacy value ab discard ki ja
    /// sakti hai: ya to migrate ho gayi, ya store mein pehle se (nayi)
    /// password maujood hai (purani settings wali stale hai — overwrite
    /// nahi hoti). False = BoardMac abhi maloom nahi — legacy value
    /// in-memory rakho, agle HELLO par dobara koshish karo.
    /// </summary>
    public bool MigrateLegacyPassword(string? legacyPassword)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        if (string.IsNullOrEmpty(legacyPassword)) return true; // kuch migrate karna hi nahi
        // Ghalat format (§5.2) kabhi board ka password nahi tha — store mat
        // karo, discard karo. "change-me-123" format to pass karta hai
        // (13 printable chars) lekin purana hard-coded default hai, kabhi
        // valid per-device password nahi tha (AppSettings.Sanitize jaisa) —
        // use bhi discard karo, store kabhi nahi.
        if (legacyPassword == "change-me-123") return true;
        if (ControlClient.ValidateNewPassword(legacyPassword) is not null) return true;
        if (BoardMac is null) return false;
        string boardId = RSH.Core.Security.KeyStore.BoardIdForMac(BoardMac);
        try
        {
            if (_keyStore.TryLoadPassword(boardId, out string? existing) && !string.IsNullOrEmpty(existing))
                return true; // store mein pehle se password hai — legacy stale hai, discard karo
            return _keyStore.TrySavePassword(boardId, legacyPassword);
        }
        catch (ObjectDisposedException) { return false; }
        catch (InvalidOperationException) { return false; }
    }

    /// <summary>
    /// In-memory session key ko foran saaf karo (SEC-E-09 / W2 follow-up):
    /// key RAM se mit jati hai, app restart ka intezar nahi. KeyStore ke
    /// blobs ko haath nahi lagta — agli control command par key store se
    /// dobara promote ho jati hai (TryGetKey). Nonce/counter nahi chheerte:
    /// counter board ke saath monotonic rehta hai, is liye wipe ke baad
    /// dobara promote hone par wahi session jaari rehta hai (counter reset
    /// karna replay/duplicate ka rasta kholta). `ForgetBoardCredentials`
    /// se farq: wo stored credentials bhi mitata hai; ye sirf RAM saaf
    /// karta hai (misal sensitive operation ke baad, ya user ke "lock" par).
    /// </summary>
    public void WipeSessionKey()
    {
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        ClearKey();
    }

    /// <summary>
    /// Is board ke tamam stored credentials (key + password) mitao aur
    /// session key bhi saaf karo — KEY_RESET ke baad dobara pairing se
    /// pehle, ya user ke "forget board" par. Stale key/password ke saath
    /// dobara try karna HMAC/auth fail par khatam hota hai (fail-closed),
    /// lekin saaf karna hi sahi rotation hygiene hai (SEC-E-09).
    /// </summary>
    public bool ForgetBoardCredentials()
    {
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        ClearKey();
        if (BoardMac is null) return true;
        try
        {
            return _keyStore.Delete(RSH.Core.Security.KeyStore.BoardIdForMac(BoardMac));
        }
        catch (ObjectDisposedException) { return false; }
        catch (InvalidOperationException) { return false; }
    }

    /// <summary>
    /// Session key nikalo: pehle memory, phir key store (BoardMac se).
    /// Store se mili key session mein promote hoti hai. Fail ho to false —
    /// caller ko saaf wajah milti hai, khamoshi se unsigned command kabhi
    /// nahi bhejta.
    /// </summary>
    private bool TryGetKey(out byte[] key)
    {
        key = null!;
        if (_key is not null)
        {
            key = _key;
            return true;
        }
        if (BoardMac is null) return false;
        byte[]? stored;
        try
        {
            if (!_keyStore.TryLoadKey(RSH.Core.Security.KeyStore.BoardIdForMac(BoardMac), out stored) || stored is null)
                return false;
        }
        catch (ObjectDisposedException) { return false; }
        catch (InvalidOperationException) { return false; }
        _key = stored; // promote: ab session key hai
        key = _key;
        return true;
    }

    /// <summary>HELLO reply se session update karo: nonce hamesha, key sirf jab nonzero ho (paired board zero bhejta hai — purani key kabhi overwrite nahi hoti). Naya nonce mile to counter 0 se shuru (§2.3). Nonzero key milte hi key store mein save hoti hai (SEC-D-05) — save fail ho to session phir bhi chalti hai (key memory mein hai).
    /// SEC-E-09 (rotation hygiene): nayi key purani stored key se mukhtalif
    /// ho to ye KEY_RESET / nayi pairing hai — board ne password bhi naya
    /// generate kiya hai (§5.4), is liye purana stored password STALE hai
    /// aur foran mitaya jata hai (Delete, phir nayi key save). Stale
    /// password ke saath Wi-Fi join / WebOTA auth fail hota — khamoshi se
    /// ghalat password istemal karne se behtar hai ke store khaali ho aur
    /// user ko dobara pairing ka saaf rasta mile.</summary>
    public void UpdateFromHello(HelloReply reply)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        if (_nonce is null || !reply.Nonce.AsSpan().SequenceEqual(_nonce))
            _nextCounter = 0; // naya nonce = naya session = counter 0 se
        _nonce = reply.Nonce.ToArray();
        BoardMac = reply.BoardMac.ToArray();
        HelloFirmwareVersion = new Version(reply.FwMajor, reply.FwMinor, reply.FwPatch);
        if (reply.DeviceKey.AsSpan().IndexOfAnyExcept((byte)0) >= 0)
        {
            string boardId = RSH.Core.Security.KeyStore.BoardIdForMac(BoardMac);
            bool rotated = false;
            try
            {
                if (_keyStore.TryLoadKey(boardId, out byte[]? stored) && stored is not null)
                    rotated = !stored.AsSpan().SequenceEqual(reply.DeviceKey.AsSpan());
            }
            catch (ObjectDisposedException) { /* store band — rotation check nahi, key phir bhi save karo */ }
            catch (InvalidOperationException) { /* store band — rotation check nahi, key phir bhi save karo */ }
            if (rotated)
                _keyStore.Delete(boardId); // purani key + STALE password dono mit gaye
            ClearKey();
            _key = reply.DeviceKey.ToArray();
            _keyStore.TrySaveKey(boardId, _key);
        }
    }

    private void ClearKey()
    {
        if (_key is not null) Array.Clear(_key);
        _key = null;
    }

    /// <summary>GET_CAPS client banao — key/nonce na hon to null + wajah (caller upload gate ko bataye). <paramref name="transport"/> sirf tests ke liye (fake transport); null ho to asal UDP transport banta hai.</summary>
    public GetCapsClient? CreateGetCapsClient(string boardHost, out string? error, IControlTransport? transport = null)
    {
        error = null;
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        if (!CanSendControl)
        {
            error = HasNonce
                ? "Board session key maujood nahi (na session mein, na key store mein — dobara pairing / KEY_RESET se nayi key lo). GET_CAPS nahi ho saka."
                : "Abhi tak HELLO reply nahi aaya — board se session (nonce/key) nahi mila. GET_CAPS nahi ho saka.";
            return null;
        }
        TryGetKey(out byte[] key);
        return new GetCapsClient(new ControlClient(boardHost, key, _nonce!, transport, firstCounter: _nextCounter));
    }

    /// <summary>
    /// HMAC-signed REBOOT (cmd 0x09, §3.3) bhejo — SEC-D-01: unauthenticated
    /// RSTX hata diya gaya hai; restart sirf isi se hota hai. Key/nonce
    /// na hon to (false, wajah) — RSTX fallback kabhi nahi (wo rasta
    /// band hai, khamoshi se purana rasta istemal karna security review
    /// ki khilaf-warzi hogi). Reply status OK ho to true.
    /// </summary>
    public async Task<(bool Sent, string? Error)> SendRebootAsync(string boardHost, IControlTransport? transport = null, CancellationToken cancellationToken = default)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        if (!CanSendControl)
        {
            return (false, HasNonce
                ? "Restart nahi ho saka: session key maujood nahi (na session mein, na key store mein — RSTX hata diya gaya, SEC-D-01; restart ke liye HMAC-signed REBOOT chahiye aur us ke liye pairing/key chahiye)."
                : "Restart nahi ho saka: abhi tak HELLO reply nahi aaya — board session nahi mila (RSTX hata diya gaya — SEC-D-01; restart sirf HMAC-signed REBOOT se hota hai).");
        }
        TryGetKey(out byte[] key);
        using var control = new ControlClient(boardHost, key, _nonce!, transport, firstCounter: _nextCounter);
        var result = await control.SendCommandAsync(ControlCommand.Reboot, ReadOnlyMemory<byte>.Empty, cancellationToken).ConfigureAwait(false);
        _nextCounter = control.NextCounter;
        if (result.ReplyPayload is null) return (false, result.Error ?? "REBOOT ka jawab nahi aaya.");
        if (result.ReplyStatus != ControlStatus.Ok) return (false, $"Board ne REBOOT par {result.ReplyStatus} diya.");
        return (true, null);
    }

    /// <summary>Pin-map push ke liye signed control sender (Phase F W3):
    /// SET_CFG/COMMIT/CONFIRM/REVERT PinMapBoardPush se aate hain — yahan
    /// sirf session (key/nonce/counter) lagta hai, command/payload caller
    /// ka hai. Key/nonce na hon to ControlResult.Error (koi unsigned
    /// command kabhi nahi — SendRebootAsync jaisa guard). Counter session
    /// ke saath monotonic rehta hai (har call ke baad _nextCounter taaza).
    /// <paramref name="transport"/> sirf tests ke liye (fake transport);
    /// null ho to asal UDP transport banta hai.</summary>
    public async Task<ControlResult> SendControlAsync(
        string boardHost, ControlCommand command, ReadOnlyMemory<byte> payload,
        IControlTransport? transport = null, CancellationToken cancellationToken = default)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        if (!CanSendControl) return new ControlResult(null, null,
            "Board session key/nonce maujood nahi — pehle HELLO/pairing chahiye.", 0);
        TryGetKey(out byte[] key);
        using var control = new ControlClient(boardHost, key, _nonce!, transport, firstCounter: _nextCounter);
        var result = await control.SendCommandAsync(command, payload, cancellationToken).ConfigureAwait(false);
        _nextCounter = control.NextCounter;
        return result;
    }

    /// HMAC-signed SET_PASSWORD (cmd 0x0C, §5.2) bhejo — board ka Wi-Fi/OTA
    /// password badlo (ek credential, AP + WebOTA dono). Ghalat password par
    /// <see cref="ArgumentException"/> (board tak kuch nahi jata). Key/nonce
    /// na hon to (false, wajah) — unsigned command kabhi nahi. Kamyabi par
    /// naya password key store mein save hota hai (§5.3: WebOTA auth ke liye
    /// app ko password chahiye; OS Wi-Fi profile se PSK wapas nahi milta).
    /// Board reply OK ke baad AP restart karta hai — PC dobara naye
    /// password se join karega; is call ke baad Wi-Fi dobara connect karna
    /// caller ki zimmedari hai.
    /// SEC-E-09: store save fail ho (DPAPI/IO) to board ka password badal
    /// chuka hota hai aur app ke paas naya password ka koi record nahi —
    /// ye (true, warning) hai, (false, ...) nahi (board badal gaya hai,
    /// jhoot mat bolo), aur warning mein KEY_RESET recovery likhna lazmi
    /// hai. Caller warning ko user ko dikhaye — khamoshi se nigalna mana hai.
    /// </summary>
    public async Task<(bool Sent, string? Error)> SetPasswordAsync(string boardHost, string newPassword, IControlTransport? transport = null, CancellationToken cancellationToken = default)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        if (!CanSendControl)
        {
            return (false, HasNonce
                ? "Password nahi badla ja saka: session key maujood nahi (na session mein, na key store mein — SET_PASSWORD ke liye HMAC-signed control chahiye)."
                : "Password nahi badla ja saka: abhi tak HELLO reply nahi aaya — board session nahi mila.");
        }
        TryGetKey(out byte[] key);
        using var control = new ControlClient(boardHost, key, _nonce!, transport, firstCounter: _nextCounter);
        var result = await control.SetPasswordAsync(newPassword, cancellationToken).ConfigureAwait(false);
        _nextCounter = control.NextCounter;
        if (result.ReplyPayload is null) return (false, result.Error ?? "SET_PASSWORD ka jawab nahi aaya.");
        if (result.ReplyStatus != ControlStatus.Ok) return (false, $"Board ne SET_PASSWORD par {result.ReplyStatus} diya.");
        if (BoardMac is not null)
        {
            bool saved = _keyStore.TrySavePassword(RSH.Core.Security.KeyStore.BoardIdForMac(BoardMac), newPassword);
            if (!saved)
                return (true, "Password board par badal gaya, lekin naya password secure store mein save nahi ho saka — WebOTA auth / Wi-Fi join ke liye password dobara darj karna hoga. Agar password bhool jaye to KEY_RESET (BOOT-hold ≥10 s) se naya password lo (PROTOCOL §5.4).");
        }
        return (true, null);
    }

    /// <summary>
    /// HMAC-signed LED_TEST (cmd 0x0A, PROTOCOL-V2.1 §3.7) bhejo — 8 B
    /// payload: patternId, r, g, b, periodMs u16 (LE), repeats, reserved 0.
    /// Pattern IDs (firmware led_patterns.h): 0 OFF, 1 SOLID, 2 BLINK,
    /// 3 CHASE, 4 RAINBOW. Default: RAINBOW, 1000 ms period, repeats 0
    /// (= 5 s chalao phir stop). LED module configured na ho ya pin
    /// allowed na ho to board UNSUPPORTED / PIN_CONFLICT deta hai —
    /// wo yahan (false, wajah) banta hai, khamoshi se nahi. Key/nonce
    /// na hon to bhi (false, wajah) — unsigned command kabhi nahi.
    /// Brightness (Phase H WS2): <paramref name="brightnessPct"/> (0..100)
    /// PC-side scaling hai — r/g/b us hisaab se kam karke bheje jate hain.
    /// </summary>
    public async Task<(bool Sent, string? Error)> SendLedTestAsync(string boardHost, byte patternId = 4, byte r = 0, byte g = 0, byte b = 0, ushort periodMs = 1000, byte repeats = 0, int brightnessPct = 100, IControlTransport? transport = null, CancellationToken cancellationToken = default)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(BoardSession));
        if (patternId > 4)
            return (false, "Unknown LED pattern — patterns are 0 OFF, 1 SOLID, 2 BLINK, 3 CHASE, 4 RAINBOW (§3.7).");
        if ((patternId is 2 or 3 or 4) && (periodMs < 100 || periodMs > 5000))
            return (false, "LED test period must be 100–5000 ms (§3.7).");
        if (brightnessPct < 0 || brightnessPct > 100)
            return (false, "LED test brightness must be 0–100%.");
        if (!CanSendControl)
        {
            return (false, HasNonce
                ? "LED test nahi bheja ja saka: session key maujood nahi (na session mein, na key store mein — LED_TEST ke liye HMAC-signed control chahiye)."
                : "LED test nahi bheja ja saka: abhi tak HELLO reply nahi aaya — board session nahi mila.");
        }
        TryGetKey(out byte[] key);
        // Brightness (Phase H WS2): PC-side scaling — r/g/b ko
        // brightnessPct% se zarb de kar bheja jata hai (integer math);
        // wire format mein koi brightness field nahi (§3.7), is liye
        // firmware ko tabdeeli nahi chahiye. 0% = strip kaali (pattern
        // phir bhi chalta hai). SEC-H-10: firmware total run time
        // 60 s par cap karta hai (repeats 255 bhi ~21 min nahi).
        byte[] payload = new byte[8];
        payload[0] = patternId;
        payload[1] = (byte)(r * brightnessPct / 100);
        payload[2] = (byte)(g * brightnessPct / 100);
        payload[3] = (byte)(b * brightnessPct / 100);
        payload[4] = (byte)(periodMs & 0xFF);
        payload[5] = (byte)(periodMs >> 8);
        payload[6] = repeats;
        payload[7] = 0;
        using var control = new ControlClient(boardHost, key, _nonce!, transport, firstCounter: _nextCounter);
        var result = await control.SendCommandAsync(ControlCommand.LedTest, payload, cancellationToken).ConfigureAwait(false);
        _nextCounter = control.NextCounter;
        if (result.ReplyPayload is null) return (false, result.Error ?? "LED_TEST ka jawab nahi aaya.");
        if (result.ReplyStatus != ControlStatus.Ok) return (false, $"Board ne LED_TEST par {result.ReplyStatus} diya.");
        return (true, null);
    }

    /// <summary>GET_CAPS ek call mein: client banao, caps mango, client band karo. Na mile to (null, wajah). <paramref name="transport"/> sirf tests ke liye (fake transport).</summary>
    public async Task<(BoardCaps? Caps, string? Error)> GetCapsAsync(string boardHost, IControlTransport? transport = null, CancellationToken cancellationToken = default)
    {
        var client = CreateGetCapsClient(boardHost, out string? error, transport);
        if (client is null) return (null, error);
        using (client)
        {
            var caps = await client.GetCapsAsync(cancellationToken).ConfigureAwait(false);
            _nextCounter = client.NextCounter;
            return (caps, caps is null ? client.LastError : null);
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        ClearKey();
        if (_nonce is not null) Array.Clear(_nonce);
        _nonce = null;
        BoardMac = null;
        HelloFirmwareVersion = null;
        if (_ownsKeyStore && _keyStore is IDisposable disposable) disposable.Dispose();
    }
}
