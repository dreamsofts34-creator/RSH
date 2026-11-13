using System.Buffers.Binary;
using RSH.Core.Protocol;
using RSH.Core.Security;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>
/// Phase E Workstream 5 (PC-side): SET_PASSWORD (0x0C) + DPAPI key store
/// (SEC-D-05 CLOSED) + BoardSession store integration.
///
/// Firmware taraf (host tests §22): handler + validator + deferred
/// callback. Yahan: ControlClient.SetPasswordAsync frame/validation,
/// KeyStore save/load/delete, aur BoardSession ka store se key lena +
/// password save karna.
///
/// SEC-E-02 (Phase F): SetPasswordAsync ab process-wide rate limit
/// rakhta hai — is liye ye class PhaseFWorkstream2Tests ke saath ek hi
/// collection mein hai (parallel run mein ek doosre ko rate-limit na karein).
/// </summary>
[Collection("SetPasswordRateLimit")]
public class PhaseEWorkstream5Tests
{
    public PhaseEWorkstream5Tests()
    {
        // SEC-E-02 (Phase F): SetPasswordAsync ka process-wide rate
        // limit har test se pehle reset — warna is class ke kayi tests
        // (har ek SET_PASSWORD bhejta hai) 60 s ke andar ek doosre ko
        // rate-limit kar dete (ye test isolation hai, production
        // behaviour nahi badalta).
        ControlClient.ResetSetPasswordRateLimitForTest();
    }

    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();
    private static readonly byte[] OtherNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xB0 + i)).ToArray();
    private static readonly byte[] TestMac = { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF };
    private const string TestBoardId = "aabbccddeeff";

    private static HelloReply Hello(byte flags, byte[] nonce, byte[] key) =>
        new(flags, 0, TestMac.ToArray(), 4, 0, 0, nonce.ToArray(), key.ToArray(), new byte[24]);

    /// <summary>Fake control transport: har request par responder chalao (null = timeout).</summary>
    private sealed class FakeControlTransport : IControlTransport
    {
        private readonly Func<byte[], byte[]?> _responder;
        public List<byte[]> CapturedFrames { get; } = new();
        public FakeControlTransport(Func<byte[], byte[]?> responder) => _responder = responder;
        public Task<byte[]?> ExchangeAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken)
        {
            CapturedFrames.Add(frame.ToArray());
            return Task.FromResult(_responder(frame));
        }
        public void Dispose() { }
    }

    /// <summary>Control reply frame banao: request ke reqId/counter echo, reply cmd, payload.</summary>
    private static byte[] ReplyFor(byte[] request, byte[] payload, byte[] key, byte[] nonce)
    {
        ushort reqId = BinaryPrimitives.ReadUInt16LittleEndian(request.AsSpan(5, 2));
        uint counter = BinaryPrimitives.ReadUInt32LittleEndian(request.AsSpan(7, 4));
        return ControlFrameCodec.Build((byte)(request[4] | 0x80), reqId, counter, payload, key, nonce);
    }

    /// <summary>GET_CAPS reply payload (status + §3.4 body, 1 pin entry).</summary>
    private static byte[] CapsPayload() =>
    [
        0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        4, 0, 0, 0x02, 0x01, 0x08, 0x40, 0x04, 6, 0x00, 0x01,
        34, 0x01, 0x01, 0x05, 0, 0, 0, 0,
    ];

    /// <summary>In-memory test double (IKeyStore) — BoardSession integration ke liye.</summary>
    private sealed class FakeKeyStore : IKeyStore
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
        public bool TrySavePassword(string boardId, string password) { Passwords[boardId] = password; return true; }
        public bool TryLoadPassword(string boardId, out string? password)
        {
            if (Passwords.TryGetValue(boardId, out string? p)) { password = p; return true; }
            password = null;
            return false;
        }
        public bool Delete(string boardId) { Keys.Remove(boardId); Passwords.Remove(boardId); return true; }
    }

    // ================= KeyStore =================

    [Fact]
    public void KeyStore_BoardIdForMac_HexLowercase_AndInvalidMacThrows()
    {
        Assert.Equal(TestBoardId, KeyStore.BoardIdForMac(TestMac));
        Assert.Throws<ArgumentException>(() => KeyStore.BoardIdForMac(new byte[5]));
        Assert.Throws<ArgumentNullException>(() => KeyStore.BoardIdForMac(null!));
    }

    [Fact]
    public void KeyStore_SaveLoad_Delete_RoundTrip()
    {
        // Temp root: Windows par DPAPI blob wahin banta hai; Linux par
        // store in-memory hai (koi file nahi banti — yehi degrade hai).
        string root = Path.Combine(Path.GetTempPath(), "rsh-ws5-test-" + Guid.NewGuid().ToString("N"));
        using var store = new KeyStore(root);
        try
        {
            Assert.False(store.TryLoadKey(TestBoardId, out byte[]? missing) && missing is not null);
            Assert.False(store.TryLoadPassword(TestBoardId, out string? missingPw) && missingPw is not null);

            Assert.True(store.TrySaveKey(TestBoardId, TestKey));
            Assert.True(store.TryLoadKey(TestBoardId, out byte[]? loaded));
            Assert.Equal(TestKey, loaded);

            // Load ki hui copy mutate karo — store ki copy mutasir na ho.
            loaded![0] ^= 0xFF;
            Assert.True(store.TryLoadKey(TestBoardId, out byte[]? loaded2));
            Assert.Equal(TestKey, loaded2);

            Assert.True(store.TrySavePassword(TestBoardId, "NewPassw0rd!"));
            Assert.True(store.TryLoadPassword(TestBoardId, out string? pw));
            Assert.Equal("NewPassw0rd!", pw);

            // Doosra boardId alag hai — koi cross-talk nahi.
            Assert.False(store.TryLoadKey("001122334455", out _));

            Assert.True(store.Delete(TestBoardId));
            Assert.False(store.TryLoadKey(TestBoardId, out _));
            Assert.False(store.TryLoadPassword(TestBoardId, out _));
            Assert.True(store.Delete(TestBoardId)); // idempotent
        }
        finally
        {
            if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        }
    }

    [Fact]
    public void KeyStore_NonWindows_IsInMemoryOnly_NoFilesCreated()
    {
        if (OperatingSystem.IsWindows())
        {
            // Windows par ye test laagu nahi — DPAPI file round-trip
            // KeyStore_Windows_Dpapi_FileRoundTrip mein hai.
            return;
        }
        string root = Path.Combine(Path.GetTempPath(), "rsh-ws5-test-" + Guid.NewGuid().ToString("N"));
        using var store = new KeyStore(root);
        Assert.False(store.IsPersistent);
        Assert.True(store.TrySaveKey(TestBoardId, TestKey));
        Assert.True(store.TryLoadKey(TestBoardId, out byte[]? loaded));
        Assert.Equal(TestKey, loaded);
        Assert.False(Directory.Exists(root)); // koi file/dir nahi bani
    }

    [Fact]
    public void KeyStore_Windows_Dpapi_FileRoundTrip()
    {
        if (!OperatingSystem.IsWindows())
        {
            // DPAPI sirf Windows par hai — Linux par in-memory degrade
            // upar wale test mein hai.
            return;
        }
        string root = Path.Combine(Path.GetTempPath(), "rsh-ws5-test-" + Guid.NewGuid().ToString("N"));
        using var store = new KeyStore(root);
        try
        {
            Assert.True(store.IsPersistent);
            Assert.True(store.TrySaveKey(TestBoardId, TestKey));
            Assert.True(File.Exists(Path.Combine(root, TestBoardId + ".key")));
            // Blob plaintext key nahi hai — DPAPI overhead ke saath bada hota hai.
            byte[] blob = File.ReadAllBytes(Path.Combine(root, TestBoardId + ".key"));
            Assert.NotEqual(32, blob.Length);
            Assert.False(blob.AsSpan(0, 32).SequenceEqual(TestKey));
            Assert.True(store.TryLoadKey(TestBoardId, out byte[]? loaded));
            Assert.Equal(TestKey, loaded);
            Assert.True(store.TrySavePassword(TestBoardId, "NewPassw0rd!"));
            Assert.True(store.TryLoadPassword(TestBoardId, out string? pw));
            Assert.Equal("NewPassw0rd!", pw);
            Assert.True(store.Delete(TestBoardId));
            Assert.False(File.Exists(Path.Combine(root, TestBoardId + ".key")));
            Assert.False(File.Exists(Path.Combine(root, TestBoardId + ".pwd")));
        }
        finally
        {
            if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        }
    }

    [Fact]
    public void KeyStore_InvalidInputs_Throw()
    {
        using var store = new KeyStore(Path.Combine(Path.GetTempPath(), "rsh-ws5-test-" + Guid.NewGuid().ToString("N")));
        Assert.Throws<ArgumentException>(() => store.TrySaveKey("xyz", TestKey));
        Assert.Throws<ArgumentException>(() => store.TrySaveKey("aabbccddeeff00", TestKey));
        Assert.Throws<ArgumentException>(() => store.TrySaveKey(TestBoardId, new byte[16]));
        Assert.Throws<ArgumentNullException>(() => store.TrySaveKey(TestBoardId, null!));
        Assert.Throws<ArgumentException>(() => store.TryLoadKey("xyz", out _));
        Assert.Throws<ArgumentNullException>(() => store.TrySavePassword(TestBoardId, null!));
    }

    // ================= ControlClient.SetPasswordAsync =================

    [Fact]
    public void SetPassword_ValidateNewPassword_Boundaries()
    {
        Assert.Null(ControlClient.ValidateNewPassword("Abcdef12")); // 8, ok
        Assert.Null(ControlClient.ValidateNewPassword(new string('a', 63))); // 63, ok
        Assert.Null(ControlClient.ValidateNewPassword("!~Az29#x")); // 0x21/0x7E ok
        Assert.NotNull(ControlClient.ValidateNewPassword(null));
        Assert.NotNull(ControlClient.ValidateNewPassword("Abc12")); // 5, chhota
        Assert.NotNull(ControlClient.ValidateNewPassword(new string('a', 64))); // 64, bara
        Assert.NotNull(ControlClient.ValidateNewPassword("Abc def12")); // space mana
        Assert.NotNull(ControlClient.ValidateNewPassword("Abcdef1\u007f")); // DEL mana
    }

    [Fact]
    public async Task SetPasswordAsync_FrameShape_Cmd0x0C_PayloadLenPlusPassword()
    {
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, [0x00], TestKey, TestNonce));
        using var client = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
        var result = await client.SetPasswordAsync("NewPassw0rd!", TestContext.Current.CancellationToken);
        Assert.True(result.Succeeded);
        Assert.Equal(ControlStatus.Ok, result.ReplyStatus);
        Assert.Single(transport.CapturedFrames);
        byte[] frame = transport.CapturedFrames[0];
        Assert.Equal(0x0C, frame[4]); // SET_PASSWORD
        Assert.Equal(13, BinaryPrimitives.ReadUInt16LittleEndian(frame.AsSpan(11, 2))); // len = 1 + 12
        Assert.Equal(12, frame[13]); // newLen
        Assert.Equal("NewPassw0rd!", System.Text.Encoding.ASCII.GetString(frame, 14, 12));
    }

    [Fact]
    public async Task SetPasswordAsync_InvalidPassword_Throws_NoFrameSent()
    {
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, [0x00], TestKey, TestNonce));
        using var client = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
        await Assert.ThrowsAsync<ArgumentException>(() => client.SetPasswordAsync("short", TestContext.Current.CancellationToken));
        await Assert.ThrowsAsync<ArgumentException>(() => client.SetPasswordAsync(new string('a', 64), TestContext.Current.CancellationToken));
        await Assert.ThrowsAsync<ArgumentException>(() => client.SetPasswordAsync("has space1", TestContext.Current.CancellationToken));
        Assert.Empty(transport.CapturedFrames);
    }

    [Fact]
    public async Task SetPasswordAsync_BoardUnsupported_ReturnsStatus_NotSuccess()
    {
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, [0x06], TestKey, TestNonce)); // UNSUPPORTED
        using var client = new ControlClient("127.0.0.1", TestKey, TestNonce, transport);
        var result = await client.SetPasswordAsync("NewPassw0rd!", TestContext.Current.CancellationToken);
        Assert.False(result.Succeeded);
        Assert.Equal(ControlStatus.Unsupported, result.ReplyStatus);
    }

    // ================= BoardSession + store =================

    [Fact]
    public void BoardSession_UpdateFromHello_SavesKeyToStore()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey)); // unpaired + window
        Assert.True(store.TryLoadKey(TestBoardId, out byte[]? saved));
        Assert.Equal(TestKey, saved);
    }

    [Fact]
    public void BoardSession_PairedHello_LoadsKeyFromStore_CanSendControl()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey)); // pairing: key store mein
        // Naya session (naya process jaisa) — paired HELLO mein key zero hai.
        using var session2 = new BoardSession(store);
        Assert.False(session2.HasKey);
        session2.UpdateFromHello(Hello(0x00, OtherNonce, new byte[32])); // paired, key zero
        Assert.True(session2.CanSendControl); // store se key mili
        Assert.True(session2.HasKey); // promote hui
    }

    [Fact]
    public async Task BoardSession_SetPassword_SavesPasswordToStore()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, [0x00], TestKey, TestNonce));
        var (sent, error) = await session.SetPasswordAsync("127.0.0.1", "NewPassw0rd!", transport, cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(sent, error);
        Assert.Null(error);
        Assert.True(store.TryLoadPassword(TestBoardId, out string? savedPw));
        Assert.Equal("NewPassw0rd!", savedPw);
        Assert.Single(transport.CapturedFrames);
        Assert.Equal(0x0C, transport.CapturedFrames[0][4]);
    }

    [Fact]
    public async Task BoardSession_SetPassword_WithoutKey_Fails_NoFrame()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x00, TestNonce, new byte[32])); // paired HELLO: nonce hai, key zero
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, [0x00], TestKey, TestNonce));
        var (sent, error) = await session.SetPasswordAsync("127.0.0.1", "NewPassw0rd!", transport, cancellationToken: TestContext.Current.CancellationToken);
        Assert.False(sent);
        Assert.NotNull(error);
        Assert.Contains("key", error!, StringComparison.OrdinalIgnoreCase);
        Assert.Empty(transport.CapturedFrames);
    }

    [Fact]
    public async Task BoardSession_SetPassword_InvalidPassword_Throws_NoFrame()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, [0x00], TestKey, TestNonce));
        await Assert.ThrowsAsync<ArgumentException>(() =>
            session.SetPasswordAsync("127.0.0.1", "short", transport, cancellationToken: TestContext.Current.CancellationToken));
        Assert.Empty(transport.CapturedFrames);
        Assert.False(store.TryLoadPassword(TestBoardId, out _));
    }

    [Fact]
    public async Task BoardSession_GetCapsAsync_ViaStoreKey_WorksAfterPairing()
    {
        var store = new FakeKeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(Hello(0x09, TestNonce, TestKey));
        using var session2 = new BoardSession(store);
        session2.UpdateFromHello(Hello(0x00, OtherNonce, new byte[32]));
        var transport = new FakeControlTransport(frame =>
            ReplyFor(frame, CapsPayload(), TestKey, OtherNonce));
        var (caps, error) = await session2.GetCapsAsync("127.0.0.1", transport, cancellationToken: TestContext.Current.CancellationToken);
        Assert.Null(error);
        Assert.NotNull(caps);
        Assert.Equal(4, caps!.FwMajor);
        Assert.Equal(0, caps.FwMinor);
    }
}
