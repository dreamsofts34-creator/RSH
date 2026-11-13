using System.Buffers.Binary;
using RSH.Core.Protocol;
using RSH.Core.Security;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>
/// Phase F Workstream 1 (Cybersecurity): SEC-E-04 (Wi-Fi password
/// settings.json se DPAPI KeyStore mein), SEC-E-09 (rotation hygiene),
/// SEC-E-01 ka app-side hissa (SetPassword save-failure warning).
///
/// AppSettings (migration + settings.json mein password na hona + backup
/// JSON) ke tests yahan nahi — RSH.Tests sirf RSH.Core reference karta
/// hai; RSH.App WPF/Windows-only hai aur Linux sandbox mein build nahi
/// hota. Wo tests scratch console verification se hue hain (docs/
/// SECURITY-REVIEW-PHASE-F-W1.md §5 mein command + natija).
/// </summary>
[Collection("SetPasswordRateLimit")]
public class PhaseFWorkstream1Tests
{
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] OtherKey = Enumerable.Range(0, 32).Select(i => (byte)(0xFF - i)).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();
    private static readonly byte[] OtherNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xB0 + i)).ToArray();
    private static readonly byte[] TestMac = { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF };
    private const string TestBoardId = "aabbccddeeff";

    private static HelloReply Hello(byte flags, byte[] nonce, byte[] key) =>
        new(flags, 0, TestMac.ToArray(), 4, 0, 0, nonce.ToArray(), key.ToArray(), new byte[24]);

    private sealed class FakeControlTransport : IControlTransport
    {
        private readonly Func<byte[], byte[]?> _responder;
        public FakeControlTransport(Func<byte[], byte[]?> responder) => _responder = responder;
        public Task<byte[]?> ExchangeAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken) =>
            Task.FromResult(_responder(frame));
        public void Dispose() { }
    }

    private static byte[] ReplyFor(byte[] request, byte[] payload, byte[] key, byte[] nonce)
    {
        ushort reqId = BinaryPrimitives.ReadUInt16LittleEndian(request.AsSpan(5, 2));
        uint counter = BinaryPrimitives.ReadUInt32LittleEndian(request.AsSpan(7, 4));
        return ControlFrameCodec.Build((byte)(request[4] | 0x80), reqId, counter, payload, key, nonce);
    }

    /// <summary>In-memory test double (IKeyStore) — PhaseEWorkstream5Tests jaisa.</summary>
    private class FakeKeyStore : IKeyStore
    {
        public bool IsPersistent => false;
        public Dictionary<string, byte[]> Keys { get; } = new(StringComparer.OrdinalIgnoreCase);
        public Dictionary<string, string> Passwords { get; } = new(StringComparer.OrdinalIgnoreCase);
        public bool TrySaveKey(string boardId, byte[] key) { Keys[boardId] = key.ToArray(); return true; }
        public bool TryLoadKey(string boardId, out byte[]? key)
        {
            if (Keys.TryGetValue(boardId, out byte[]? k)) { key = k.ToArray(); return true; }
            key = null;
            return false;
        }
        public virtual bool TrySavePassword(string boardId, string password) { Passwords[boardId] = password; return true; }
        public bool TryLoadPassword(string boardId, out string? password)
        {
            if (Passwords.TryGetValue(boardId, out string? p)) { password = p; return true; }
            password = null;
            return false;
        }
        public bool Delete(string boardId) { Keys.Remove(boardId); Passwords.Remove(boardId); return true; }
    }

    /// <summary>Password save hamesha fail karta hai (DPAPI/IO fail ka simulation).</summary>
    private sealed class FailingPasswordStore : FakeKeyStore
    {
        public override bool TrySavePassword(string boardId, string password) => false;
    }

    // ================= SEC-E-04: password read path (KeyStore) =================

    [Fact]
    public void TryGetWifiPassword_NoHello_False()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        Assert.False(session.TryGetWifiPassword(out string? pw));
        Assert.Null(pw);
    }

    [Fact]
    public void TryGetWifiPassword_AfterHelloAndSave_ReturnsPassword()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        Assert.False(session.TryGetWifiPassword(out _)); // abhi save nahi hua
        Assert.True(session.TrySaveWifiPassword("NewPassw0rd!"));
        Assert.True(session.TryGetWifiPassword(out string? pw));
        Assert.Equal("NewPassw0rd!", pw);
    }

    [Fact]
    public void TrySaveWifiPassword_InvalidPassword_NotStored()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        Assert.False(session.TrySaveWifiPassword("short")); // 5 chars, §5.2 fail
        Assert.False(session.TrySaveWifiPassword("has space1"));
        Assert.False(store.TryLoadPassword(TestBoardId, out _));
    }

    [Fact]
    public void TrySaveWifiPassword_NoBoardMac_False()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        Assert.False(session.TrySaveWifiPassword("NewPassw0rd!")); // HELLO nahi aaya
    }

    // ================= SEC-E-04: legacy migration =================

    [Fact]
    public void MigrateLegacyPassword_EmptyStore_Migrates()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        Assert.True(session.MigrateLegacyPassword("LegacyPass1!"));
        Assert.True(store.TryLoadPassword(TestBoardId, out string? pw));
        Assert.Equal("LegacyPass1!", pw);
    }

    [Fact]
    public void MigrateLegacyPassword_StoreAlreadyHasPassword_DoesNotOverwrite()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        Assert.True(session.TrySaveWifiPassword("NewPassw0rd!"));
        // Legacy (purani settings wali) stale hai — nayi ko overwrite nahi karti,
        // lekin caller ke liye "handled" hai (legacy discard ki ja sakti hai).
        Assert.True(session.MigrateLegacyPassword("LegacyPass1!"));
        Assert.True(store.TryLoadPassword(TestBoardId, out string? pw));
        Assert.Equal("NewPassw0rd!", pw);
    }

    [Fact]
    public void MigrateLegacyPassword_NoBoardMac_False_EmptyOrInvalid_Handled()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        Assert.False(session.MigrateLegacyPassword("LegacyPass1!")); // HELLO nahi aaya
        Assert.True(session.MigrateLegacyPassword("")); // kuch migrate karna hi nahi
        Assert.True(session.MigrateLegacyPassword(null));
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        // Ghalat format (purana default jaisa) store nahi hota, discard hota hai.
        Assert.True(session.MigrateLegacyPassword("change-me-123"));
        Assert.False(store.TryLoadPassword(TestBoardId, out _));
    }

    // ================= SEC-E-09: rotation hygiene =================

    [Fact]
    public void Rotation_NewKeyPurgesStalePassword()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        Assert.True(session.TrySaveWifiPassword("NewPassw0rd!"));
        Assert.True(store.TryLoadPassword(TestBoardId, out _));

        // KEY_RESET / nayi pairing: nayi key, naya nonce — purana password stale hai.
        session.UpdateFromHello(Hello(0x09, OtherNonce, OtherKey));
        Assert.True(store.TryLoadKey(TestBoardId, out byte[]? savedKey));
        Assert.Equal(OtherKey, savedKey);
        Assert.False(store.TryLoadPassword(TestBoardId, out _)); // stale password mit gaya
        Assert.False(session.TryGetWifiPassword(out _));
    }

    [Fact]
    public void Rotation_SameKeyKeepsPassword()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        Assert.True(session.TrySaveWifiPassword("NewPassw0rd!"));
        // Dobara HELLO, wahi key (routine reboot) — password rehta hai.
        session.UpdateFromHello(Hello(0x00, OtherNonce, TestKey));
        Assert.True(store.TryLoadPassword(TestBoardId, out string? pw));
        Assert.Equal("NewPassw0rd!", pw);
    }

    [Fact]
    public void WipeSessionKey_ClearsMemoryKey_StoreIntact_Repromotes()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        Assert.True(session.HasKey);
        Assert.True(session.TrySaveWifiPassword("NewPassw0rd!"));

        session.WipeSessionKey();
        Assert.False(session.HasKey); // RAM se saaf — restart ka intezar nahi
        Assert.True(store.TryLoadKey(TestBoardId, out _)); // store ko haath nahi laga
        Assert.True(store.TryLoadPassword(TestBoardId, out _)); // password bhi mehfooz

        // Agli control command par key store se dobara promote hoti hai —
        // nonce/counter wahi session jaari rakhte hain.
        Assert.True(session.CanSendControl);
        Assert.True(session.HasKey);
    }

    [Fact]
    public void WipeSessionKey_NoKey_NoThrow_Disposed_Throws()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.WipeSessionKey(); // key hai hi nahi — no-op, no throw
        Assert.False(session.HasKey);
        var session2 = new BoardSession(store);
        session2.Dispose();
        Assert.Throws<ObjectDisposedException>(() => session2.WipeSessionKey());
    }

    [Fact]
    public void ForgetBoardCredentials_DeletesKeyAndPassword()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        Assert.True(session.TrySaveWifiPassword("NewPassw0rd!"));
        Assert.True(session.ForgetBoardCredentials());
        Assert.False(session.HasKey);
        Assert.False(store.TryLoadKey(TestBoardId, out _));
        Assert.False(store.TryLoadPassword(TestBoardId, out _));
        Assert.True(session.ForgetBoardCredentials()); // idempotent
    }

    [Fact]
    public async Task SetPassword_SaveFails_ReturnsSentTrueWithWarning()
    {
        // Board ne password badal diya, DPAPI save fail hua — (true, warning),
        // (false, ...) nahi: board badal gaya hai, jhoot mat bolo.
        ControlClient.ResetSetPasswordRateLimitForTest(); // SEC-E-02: 60 s static limiter
        var store = new FailingPasswordStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, [0x00], TestKey, TestNonce));
        try
        {
            var (sent, error) = await session.SetPasswordAsync("127.0.0.1", "NewPassw0rd!", transport, cancellationToken: TestContext.Current.CancellationToken);
            Assert.True(sent);
            Assert.NotNull(error);
            Assert.Contains("KEY_RESET", error!, StringComparison.Ordinal);
            Assert.False(store.TryLoadPassword(TestBoardId, out _));
        }
        finally
        {
            // Good neighbour: apni timestamp peeche mat chhoro — doosri
            // test classes (PhaseE, jin mein reset nahi hai) warna 60 s
            // limiter se takra kar flaky fail hoti hain.
            ControlClient.ResetSetPasswordRateLimitForTest();
        }
    }

    [Fact]
    public async Task SetPassword_SaveSucceeds_ReturnsSentTrueNoError()
    {
        ControlClient.ResetSetPasswordRateLimitForTest(); // SEC-E-02: 60 s static limiter
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, [0x00], TestKey, TestNonce));
        try
        {
            var (sent, error) = await session.SetPasswordAsync("127.0.0.1", "NewPassw0rd!", transport, cancellationToken: TestContext.Current.CancellationToken);
            Assert.True(sent);
            Assert.Null(error);
            Assert.True(store.TryLoadPassword(TestBoardId, out string? pw));
            Assert.Equal("NewPassw0rd!", pw);
        }
        finally
        {
            // Good neighbour — upar dekho.
            ControlClient.ResetSetPasswordRateLimitForTest();
        }
    }

    // ================= Validator regression (gate mein mila bug) =================

    /// <summary>
    /// Esptool-layout image banao: header + 1 segment + checksum byte (16-byte
    /// block ka aakhri byte) + SHA-256 + signed trailer. segmentDataSize aisa
    /// chuno ke segments 16-byte boundary par khatam hon (cursor % 16 == 0) —
    /// wahi case jis par purana validator valid image reject karta tha.
    /// </summary>
    private static byte[] BuildEsptoolLayoutImage(int segmentDataSize, bool corruptSha = false)
    {
        string marker = "Version <b>\0" + "4.0.0" + "\0</b>";
        byte[] payload = System.Text.Encoding.ASCII.GetBytes("<html>" + marker + "</html>");
        byte[] data = new byte[segmentDataSize];
        Array.Copy(payload, data, Math.Min(payload.Length, data.Length));
        using var ms = new MemoryStream();
        byte[] header = new byte[24];
        header[0] = 0xE9;
        header[1] = 1;
        ms.Write(header);
        byte[] seg = new byte[8];
        BitConverter.GetBytes(0x3F400020u).CopyTo(seg, 0);
        BitConverter.GetBytes(data.Length).CopyTo(seg, 4);
        ms.Write(seg);
        ms.Write(data);
        int cursor = (int)ms.Length;
        int digestStart = (cursor + 16) & ~15;
        while (ms.Length < digestStart - 1) ms.WriteByte(0);
        byte checksum = 0xEF;
        foreach (byte b in data) checksum ^= b;
        ms.WriteByte(checksum);
        byte[] image = ms.ToArray();
        byte[] sha = System.Security.Cryptography.SHA256.HashData(image);
        if (corruptSha) sha[0] ^= 0xFF;
        byte[] withSha = image.Concat(sha).ToArray();
        byte[] trailer = new byte[RSH.Core.Firmware.FirmwareImageValidator.SignatureTrailerSizeBytes];
        Random.Shared.NextBytes(trailer.AsSpan(0, 64));
        System.Text.Encoding.ASCII.GetBytes("RSHS").CopyTo(trailer, 64);
        return withSha.Concat(trailer).ToArray();
    }

    [Fact]
    public void Validator_AlignedSegmentEnd_AcceptsRealEsptoolLayout()
    {
        // cursor = 24 + 8 + 70000 = 70032, % 16 == 0 — purana validator
        // yahan "SHA-256 mismatch" deta tha (asal firmware.bin jaisa).
        var r = RSH.Core.Firmware.FirmwareImageValidator.Validate(BuildEsptoolLayoutImage(70_000));
        Assert.True(r.IsValid, r.Error);
        Assert.Equal(new Version(4, 0, 0), r.FirmwareVersion);
    }

    [Fact]
    public void Validator_AlignedSegmentEnd_CorruptSha_Rejected()
    {
        var r = RSH.Core.Firmware.FirmwareImageValidator.Validate(BuildEsptoolLayoutImage(70_000, corruptSha: true));
        Assert.False(r.IsValid);
    }

    [Fact]
    public void Validator_NonAlignedSegmentEnd_AcceptsRealEsptoolLayout()
    {
        // cursor = 24 + 8 + 70001 = 70033, % 16 == 1 — purana code bhi
        // yahan pass karta tha (regression guard: fix ne is case ko nahi tora).
        var r = RSH.Core.Firmware.FirmwareImageValidator.Validate(BuildEsptoolLayoutImage(70_001));
        Assert.True(r.IsValid, r.Error);
    }

    // ================= KeyStore: atomic blob write (Windows) =================

    [Fact]
    public void KeyStore_AtomicWrite_NoTempFilesLeft()
    {
        if (!OperatingSystem.IsWindows())
        {
            // Atomic temp+move path sirf persistent (Windows) store mein hai.
            return;
        }
        string root = Path.Combine(Path.GetTempPath(), "rsh-ws1-test-" + Guid.NewGuid().ToString("N"));
        using var store = new KeyStore(root);
        try
        {
            Assert.True(store.TrySaveKey(TestBoardId, TestKey));
            Assert.True(store.TrySavePassword(TestBoardId, "NewPassw0rd!"));
            Assert.True(store.TrySaveKey(TestBoardId, OtherKey)); // overwrite — purana temp nahi rehta
            Assert.Empty(Directory.GetFiles(root, "*.tmp-*"));
            Assert.True(store.TryLoadKey(TestBoardId, out byte[]? loaded));
            Assert.Equal(OtherKey, loaded);
        }
        finally
        {
            if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        }
    }
}
