// Phase F — Workstream 3 (Games SW-14/A2 + mDNS D33 + pin-map board push).
// Core-side tests: CRC32, config blob (§3.6), EspAddress mDNS resolve (D33).
// App-side tests (GameLauncher Stop SEC-E-10, restore-on-exit, live reload,
// PinMapBoardPush) RSH.App.Tests mein hain — RSH.App yahan reference nahi
// ho sakta (net10.0-windows WPF).
using System.Net;
using System.Net.Sockets;
using RSH.Core.Net;
using RSH.Core.Protocol;

namespace RSH.Tests;

public sealed class PhaseFWorkstream3Tests
{
    // ---- CRC32 (§3.6 trailer; firmware config_transaction.h rsh_crc32) ----

    [Fact]
    public void Crc32_CheckValues_MatchFirmware()
    {
        Assert.Equal(0xCBF43926u, Crc32.Compute("123456789"u8));
        Assert.Equal(0x00000000u, Crc32.Compute(ReadOnlySpan<byte>.Empty));
    }

    // ---- Config blob (§3.6) ----

    [Fact]
    public void ConfigBlob_Build_FactoryLikeBlob_HasRightShapeAndValidates()
    {
        // Firmware factory blob jaisa (config_transaction.h): 3 axes + 4 buttons.
        var records = new List<ConfigModuleRecord>
        {
            new(Type: 1, Slot: 0, Flags: 0, PinA: 34),
            new(Type: 1, Slot: 1, Flags: 0, PinA: 35),
            new(Type: 1, Slot: 2, Flags: 0, PinA: 36),
            new(Type: 3, Slot: 0, Flags: 0, PinA: 25),
            new(Type: 3, Slot: 1, Flags: 0, PinA: 26),
            new(Type: 3, Slot: 2, Flags: 0, PinA: 27),
            new(Type: 3, Slot: 3, Flags: 0, PinA: 33),
        };
        byte[] blob = ConfigBlob.Build(records, cfgVersion: 1);
        Assert.Equal(8 + 7 * 16 + 4, blob.Length); // firmware factory blob = 124 B
        Assert.Equal((byte)'R', blob[0]);
        Assert.Equal((byte)'S', blob[1]);
        Assert.Equal((byte)'H', blob[2]);
        Assert.Equal((byte)'G', blob[3]);
        Assert.Equal(2, blob[4]); // schema
        Assert.Equal(7, blob[5]); // moduleCount
        Assert.Equal(1, blob[6] | (blob[7] << 8)); // cfgVersion LE
        Assert.True(ConfigBlob.TryValidateStructure(blob, out string? error), error);
    }

    [Fact]
    public void ConfigBlob_TamperedByte_FailsCrc()
    {
        byte[] blob = ConfigBlob.Build([new ConfigModuleRecord(Type: 1, Slot: 0, Flags: 0, PinA: 34)], cfgVersion: 1);
        blob[10] ^= 0xFF; // record byte bigaro — CRC trailer ab match nahi karega
        Assert.False(ConfigBlob.TryValidateStructure(blob, out string? error));
        Assert.Contains("CRC32", error);
    }

    [Fact]
    public void ConfigBlob_BadMagic_And_WrongLength_Fail()
    {
        byte[] blob = ConfigBlob.Build([new ConfigModuleRecord(Type: 1, Slot: 0, Flags: 0, PinA: 34)], cfgVersion: 1);
        blob[0] = (byte)'X';
        Assert.False(ConfigBlob.TryValidateStructure(blob, out _));
        Assert.False(ConfigBlob.TryValidateStructure(blob.AsSpan(0, blob.Length - 1), out _));
    }

    [Fact]
    public void ConfigBlob_EmptyModules_BuildsHeaderPlusTrailer()
    {
        byte[] blob = ConfigBlob.Build([], cfgVersion: 3);
        Assert.Equal(12, blob.Length);
        Assert.Equal(0, blob[5]);
        Assert.True(ConfigBlob.TryValidateStructure(blob, out _));
    }

    // ---- mDNS resolve (D33): rsh.local pehle, fallback 192.168.4.1 ----

    [Fact]
    public async Task ResolveAsync_MdnsPrivateIp_ReturnsIt_NoFallback()
    {
        var (ok, address, usedFallback, error) = await EspAddress.ResolveAsync(
            "rsh.local",
            (_, _) => Task.FromResult<IPAddress[]?>([IPAddress.Parse("192.168.4.1")]),
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(ok, error);
        Assert.Equal("192.168.4.1", address?.ToString());
        Assert.False(usedFallback);
    }

    [Fact]
    public async Task ResolveAsync_MdnsPublicIp_FallsBackToDefault()
    {
        // Sandbox jaisa wildcard DNS: rsh.local public/sinkhole IP deta hai —
        // SEC-C-03 guard: aisa address kabhi istemal nahi hota, fallback milta hai.
        var (ok, address, usedFallback, _) = await EspAddress.ResolveAsync(
            "rsh.local",
            (_, _) => Task.FromResult<IPAddress[]?>([IPAddress.Parse("198.18.33.210")]),
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(ok);
        Assert.Equal(EspAddress.DefaultEspIp, address?.ToString());
        Assert.True(usedFallback);
    }

    [Fact]
    public async Task ResolveAsync_MdnsLookupThrows_FallsBackToDefault()
    {
        var (ok, address, usedFallback, _) = await EspAddress.ResolveAsync(
            "rsh.local",
            (_, _) => throw new SocketException((int)SocketError.HostNotFound),
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(ok);
        Assert.Equal(EspAddress.DefaultEspIp, address?.ToString());
        Assert.True(usedFallback);
    }

    [Fact]
    public async Task ResolveAsync_MdnsNoAnswer_FallsBackToDefault()
    {
        var (ok, address, usedFallback, _) = await EspAddress.ResolveAsync(
            "rsh.local",
            (_, _) => Task.FromResult<IPAddress[]?>([]),
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(ok);
        Assert.Equal(EspAddress.DefaultEspIp, address?.ToString());
        Assert.True(usedFallback);
    }

    [Fact]
    public async Task ResolveAsync_PrivateLiteral_DoesNotTouchDns()
    {
        bool dnsCalled = false;
        var (ok, address, usedFallback, _) = await EspAddress.ResolveAsync(
            "10.0.0.5",
            (_, _) => { dnsCalled = true; return Task.FromResult<IPAddress[]?>(null); },
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(ok);
        Assert.Equal("10.0.0.5", address?.ToString());
        Assert.False(usedFallback);
        Assert.False(dnsCalled);
    }

    [Fact]
    public async Task ResolveAsync_OtherHostname_Rejects_NoFallback()
    {
        var (ok, address, usedFallback, error) = await EspAddress.ResolveAsync("attacker.example", cancellationToken: TestContext.Current.CancellationToken);
        Assert.False(ok);
        Assert.Null(address);
        Assert.False(usedFallback);
        Assert.NotNull(error);
    }

    [Fact]
    public async Task ResolveAsync_PublicLiteral_Rejects()
    {
        var (ok, _, _, _) = await EspAddress.ResolveAsync("8.8.8.8", cancellationToken: TestContext.Current.CancellationToken);
        Assert.False(ok);
    }

    [Fact]
    public void IsMdnsName_AcceptsCaseAndTrailingDot()
    {
        Assert.True(EspAddress.IsMdnsName("rsh.local"));
        Assert.True(EspAddress.IsMdnsName("RSH.LOCAL"));
        Assert.True(EspAddress.IsMdnsName("rsh.local."));
        Assert.False(EspAddress.IsMdnsName("192.168.4.1"));
        Assert.False(EspAddress.IsMdnsName("rsh2.local"));
        Assert.False(EspAddress.IsMdnsName(null));
    }
}
