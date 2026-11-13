using System.Security.Cryptography;
using System.Text;
using RSH.Core.Firmware;
using RSH.Core.Net;
using RSH.Core.Utilities;
using RSH.Core.Windows;

namespace RSH.Tests;

/// <summary>Phase C — Frontend (A1/A4/A5/A8) + SEC-C-01/02/03/05/07/09 tests.</summary>
public class PhaseCAppTests
{
    // ---------- A1: FirmwareImageValidator ----------

    /// <summary>Synthetic valid ESP32 app image banayo (header + 1 segment + trailing SHA-256 + signed trailer).</summary>
    private static byte[] BuildImage(string version, byte magic = 0xE9, bool corruptSha = false,
        bool includeVersionMarker = true, int segmentDataSize = 70_000, bool signed = true)
    {
        // Asal firmware jaisa: webota.cpp alag string literals jorta hai,
        // is liye marker aur version ke darmiyan NUL byte hota hai.
        string marker = includeVersionMarker ? $"Version <b>\0{version}\0</b>" : "no version here";
        byte[] payload = Encoding.ASCII.GetBytes("<html>" + marker + "</html>");
        byte[] data = new byte[segmentDataSize];
        Array.Copy(payload, data, Math.Min(payload.Length, data.Length));
        using var ms = new MemoryStream();
        byte[] header = new byte[24];
        header[0] = magic;
        header[1] = 1; // segment count
        ms.Write(header);
        byte[] seg = new byte[8];
        BitConverter.GetBytes(0x3F400020u).CopyTo(seg, 0);
        BitConverter.GetBytes(data.Length).CopyTo(seg, 4);
        ms.Write(seg);
        ms.Write(data);
        // Asal esptool layout (bin_image.py append_checksum + save):
        // segments ke baad 16-byte block ke aakhri byte par 1 checksum byte
        // (seed 0xEF, segment data bytes ka XOR), phir SHA-256 [0..digestStart)
        // ka. digestStart = (cursor + 16) & ~15. Purana builder seedha 16-byte
        // pad + SHA likhta tha — wahi ghalat assumption jo validator mein thi
        // (Phase F W1: 16-aligned segment end par asal image reject hoti thi).
        int cursor = (int)ms.Length;
        int digestStart = (cursor + 16) & ~15;
        while (ms.Length < digestStart - 1) ms.WriteByte(0);
        byte checksum = 0xEF;
        foreach (byte b in data) checksum ^= b;
        ms.WriteByte(checksum);
        byte[] image = ms.ToArray();
        byte[] sha = SHA256.HashData(image);
        if (corruptSha) sha[0] ^= 0xFF;
        byte[] withSha = image.Concat(sha).ToArray();
        return signed ? WithTestTrailer(withSha) : withSha;
    }

    /// <summary>Format-valid signed trailer lagao (test-signing flow).
    /// Signature yahan random bytes hain — app validator sirf trailer
    /// format check karta hai; crypto authenticity board-side hai
    /// (host §20 + sign-image.py --verify). Asal signed artifact
    /// firmware-signed.bin hota hai (sign_firmware.py, RSH_SIGNING_KEY).</summary>
    private static byte[] WithTestTrailer(byte[] image)
    {
        byte[] trailer = new byte[FirmwareImageValidator.SignatureTrailerSizeBytes];
        Random.Shared.NextBytes(trailer.AsSpan(0, 64));
        Encoding.ASCII.GetBytes("RSHS").CopyTo(trailer, 64);
        return image.Concat(trailer).ToArray();
    }

    [Fact]
    public void ValidImage_Passes_WithVersionAndSha()
    {
        var r = FirmwareImageValidator.Validate(BuildImage("4.0.0"));
        Assert.True(r.IsValid, r.Error);
        Assert.Equal(FirmwareImageValidator.ImageLayout.AppOnly, r.Layout);
        Assert.Equal(new Version(4, 0, 0), r.FirmwareVersion);
        Assert.Equal(64, r.FileSha256Hex.Length);
        Assert.Equal(64, r.ImageSha256Hex!.Length);
    }

    [Fact]
    public void NewerVersion_Passes()
    {
        Assert.True(FirmwareImageValidator.Validate(BuildImage("4.0.1")).IsValid);
        Assert.True(FirmwareImageValidator.Validate(BuildImage("4.1.0")).IsValid);
    }

    [Fact]
    public void OldVersion_Rejected_AntiDowngrade()
    {
        var r = FirmwareImageValidator.Validate(BuildImage("3.3.0"));
        Assert.False(r.IsValid);
        Assert.Contains("Downgrade", r.Error);
    }

    [Fact]
    public void WrongMagic_Rejected()
    {
        var r = FirmwareImageValidator.Validate(BuildImage("4.0.0", magic: 0x00));
        Assert.False(r.IsValid);
        Assert.Contains("Magic", r.Error);
    }

    [Fact]
    public void BadSha_Rejected()
    {
        var r = FirmwareImageValidator.Validate(BuildImage("4.0.0", corruptSha: true));
        Assert.False(r.IsValid);
        Assert.Contains("SHA-256", r.Error);
    }

    [Fact]
    public void MissingVersion_Rejected()
    {
        var r = FirmwareImageValidator.Validate(BuildImage("4.0.0", includeVersionMarker: false));
        Assert.False(r.IsValid);
        Assert.Contains("version", r.Error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void TruncatedFile_Rejected()
    {
        byte[] full = BuildImage("4.0.0");
        Assert.False(FirmwareImageValidator.Validate(full.Take(1000).ToArray()).IsValid);
        Assert.False(FirmwareImageValidator.Validate(full.Take(full.Length - 10).ToArray()).IsValid);
    }

    [Fact]
    public void OversizeFile_Rejected()
    {
        var r = FirmwareImageValidator.Validate(new byte[FirmwareImageValidator.MaxFileSizeBytes + 1]);
        Assert.False(r.IsValid);
        Assert.Contains("bari", r.Error);
    }

    [Fact]
    public void MergedFactoryImage_Rejected()
    {
        byte[] app = BuildImage("4.0.0");
        byte[] merged = new byte[FirmwareImageValidator.MergedAppOffset + app.Length];
        Array.Fill(merged, (byte)0xFF);
        merged[FirmwareImageValidator.MergedBootloaderOffset] = 0xE9;
        merged[FirmwareImageValidator.MergedBootloaderOffset + 1] = 3;
        Array.Copy(app, 0, merged, FirmwareImageValidator.MergedAppOffset, app.Length);
        var r = FirmwareImageValidator.Validate(merged);
        Assert.False(r.IsValid);
        Assert.Equal(FirmwareImageValidator.ImageLayout.Merged, r.Layout);
        Assert.Contains("merged", r.Error, StringComparison.OrdinalIgnoreCase);
    }

    // ---------- FW-9 (Phase D): signed trailer (SEC-C-06 / SEC-D-04) ----------

    [Fact]
    public void SignedImage_Passes_AndIsSigned()
    {
        var r = FirmwareImageValidator.Validate(BuildImage("4.0.0"));
        Assert.True(r.IsValid, r.Error);
        Assert.True(r.IsSigned);
    }

    [Fact]
    public void UnsignedImage_Rejected()
    {
        var r = FirmwareImageValidator.Validate(BuildImage("4.0.0", signed: false));
        Assert.False(r.IsValid);
        Assert.False(r.IsSigned);
        Assert.Contains("signed", r.Error, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void BadTrailerMagic_Rejected()
    {
        byte[] image = BuildImage("4.0.0");
        image[^1] = (byte)'X'; // trailer magic "RSHS" kharab karo
        var r = FirmwareImageValidator.Validate(image);
        Assert.False(r.IsValid);
        Assert.Contains("signed", r.Error, StringComparison.OrdinalIgnoreCase);
    }

    // ---------- FW-9 (Phase D): WebOTA CSRF token (SEC-D-12) ----------

    [Fact]
    public void CsrfToken_Extracted_FromMetaTag()
    {
        string token = new string('a', 32); // 32 hex chars — 'a' hex hai
        string html = "<html><head><meta name=\"csrf-token\" content=\"" + token + "\"></head></html>";
        Assert.Equal(token, WebOta.TryExtractCsrfToken(html));
    }

    [Fact]
    public void CsrfToken_MissingMeta_ReturnsNull()
    {
        Assert.Null(WebOta.TryExtractCsrfToken("<html><head></head></html>"));
        Assert.Null(WebOta.TryExtractCsrfToken(""));
    }

    [Fact]
    public void CsrfToken_Malformed_ReturnsNull()
    {
        // 31 chars — chhota token.
        string html = "<html><head><meta name=\"csrf-token\" content=\"" + new string('b', 31) + "\"></head></html>";
        Assert.Null(WebOta.TryExtractCsrfToken(html));
        // Non-hex char.
        html = "<html><head><meta name=\"csrf-token\" content=\"" + new string('z', 32) + "\"></head></html>";
        Assert.Null(WebOta.TryExtractCsrfToken(html));
    }

    private static string? FindRepoFile(string relative)
    {
        string? dir = AppContext.BaseDirectory;
        for (int i = 0; i < 10 && dir is not null; i++)
        {
            string candidate = Path.Combine(dir, relative);
            if (File.Exists(candidate)) return candidate;
            dir = Directory.GetParent(dir)?.FullName;
        }
        return null;
    }

    [Fact]
    public void RealFirmwareBin_Passes()
    {
        string? path = FindRepoFile(Path.Combine("firmware", "rsh", ".pio", "build", "esp32dev", "firmware.bin"));
        if (path is null) Assert.Skip("Real firmware.bin maujood nahi (pio build nahi hua) — synthetic tests cover karte hain.");
        // pio ka firmware.bin raw app image hai (unsigned — signing build
        // step firmware-signed.bin banata hai). Test-signing flow: asal
        // binary ki structure (magic, ESP SHA, version marker) check
        // karne ke liye format-valid trailer in-memory lagao.
        var r = FirmwareImageValidator.Validate(WithTestTrailer(File.ReadAllBytes(path)));
        Assert.True(r.IsValid, r.Error);
        Assert.True(r.IsSigned);
        Assert.Equal(new Version(4, 0, 0), r.FirmwareVersion);
    }

    [Fact]
    public void RealSignedFirmwareBin_Passes_IfPresent()
    {
        // Asal signing step ka output (RSH_SIGNING_KEY set hone par banta
        // hai) — is ko seedha validate karo, bina in-memory trailer ke.
        string? path = FindRepoFile(Path.Combine("firmware", "rsh", ".pio", "build", "esp32dev", "firmware-signed.bin"));
        if (path is null) Assert.Skip("firmware-signed.bin maujood nahi (RSH_SIGNING_KEY ke baghair build unsigned rehta hai).");
        string? rawPath = FindRepoFile(Path.Combine("firmware", "rsh", ".pio", "build", "esp32dev", "firmware.bin"));
        if (rawPath is not null && File.GetLastWriteTimeUtc(path) < File.GetLastWriteTimeUtc(rawPath))
            Assert.Skip("firmware-signed.bin purana hai (firmware.bin se pehle ka) — naya signed build nahi hua.");
        var r = FirmwareImageValidator.Validate(File.ReadAllBytes(path));
        Assert.True(r.IsValid, r.Error);
        Assert.True(r.IsSigned);
        Assert.Equal(new Version(4, 0, 0), r.FirmwareVersion);
    }

    [Fact]
    public void RealMergedBin_RejectedAsMerged()
    {
        string? path = FindRepoFile(Path.Combine("firmware", "rsh", ".pio", "build", "esp32dev", "RSH-Firmware-DevKitV1-v4.0.0.bin"));
        if (path is null) Assert.Skip("Real merged bin maujood nahi — synthetic merged test cover karta hai.");
        var r = FirmwareImageValidator.Validate(File.ReadAllBytes(path));
        Assert.False(r.IsValid);
        Assert.Equal(FirmwareImageValidator.ImageLayout.Merged, r.Layout);
    }

    // ---------- SEC-C-03: EspIp validation ----------

    [Theory]
    [InlineData("192.168.4.1", true)]
    [InlineData("192.168.0.100", true)]
    [InlineData("10.0.0.5", true)]
    [InlineData("172.16.0.9", true)]
    [InlineData("169.254.1.2", true)]
    [InlineData("attacker.example/x", false)]
    [InlineData("attacker.example", false)]
    [InlineData("8.8.8.8", false)]
    [InlineData("127.0.0.1", false)]
    [InlineData("192.168.4.1:8080", false)]
    [InlineData("192.168.4.1/update", false)]
    [InlineData("user@192.168.4.1", false)]
    [InlineData("256.1.1.1", false)]
    [InlineData("", false)]
    [InlineData(null, false)]
    public void EspIp_Validation(string? value, bool expectedValid)
    {
        bool ok = EspAddress.TryValidate(value, out var address, out string? error);
        Assert.Equal(expectedValid, ok);
        if (expectedValid)
        {
            Assert.NotNull(address);
            Assert.Equal(value, address!.ToString());
            Assert.Null(error);
        }
        else
        {
            Assert.NotNull(error);
        }
    }

    [Fact]
    public void EspIp_SanitizeOrDefault_FallsBack()
    {
        Assert.Equal("192.168.4.1", EspAddress.SanitizeOrDefault("attacker.example/x"));
        Assert.Equal("192.168.4.1", EspAddress.SanitizeOrDefault(""));
        Assert.Equal("10.0.0.5", EspAddress.SanitizeOrDefault("10.0.0.5"));
    }

    // ---------- A4: Single instance + bring-to-front signal ----------

    [Fact]
    public async Task TwoInstances_SecondSignalsFirst()
    {
        string suffix = Guid.NewGuid().ToString("N");
        string mutex = $@"Global\RSH.Test.{suffix}";
        string pipe = $"RSH.Test.{suffix}";
        using var first = new SingleInstanceCoordinator(mutex, pipe);
        Assert.True(first.TryAcquire());
        var signalled = new TaskCompletionSource();
        first.StartListening(() => signalled.TrySetResult());

        using var second = new SingleInstanceCoordinator(mutex, pipe);
        Assert.False(second.TryAcquire());
        // Load par listener task / pipe server ke tayyar hone mein der ho
        // sakti hai — signal chand dafa dobara bhejo, deadline tak wait
        // karo (fixed 2 s timeout par ye test load mein flaky tha).
        bool ok = false;
        var deadline = DateTime.UtcNow.AddSeconds(10);
        while (!ok && DateTime.UtcNow < deadline)
        {
            if (SingleInstanceCoordinator.SignalExisting(pipe, timeoutMs: 2000))
            {
                var done = await Task.WhenAny(signalled.Task, Task.Delay(1000, TestContext.Current.CancellationToken));
                ok = done == signalled.Task;
            }
            if (!ok)
                await Task.Delay(200, TestContext.Current.CancellationToken);
        }
        Assert.True(ok, "Second instance ka activate signal pehli instance tak nahi pahuncha.");
    }

    [Fact]
    public void FirstInstance_Acquires()
    {
        string suffix = Guid.NewGuid().ToString("N");
        using var first = new SingleInstanceCoordinator($@"Global\RSH.Test.{suffix}", $"RSH.Test.{suffix}");
        Assert.True(first.TryAcquire());
    }

    // ---------- A5: Autostart verify ----------

    private sealed class FakeStore : IAutostartStore
    {
        public string? Value;
        public bool ThrowOnWrite;
        public bool DropWrites;
        public string? ReadValue() => Value;
        public void WriteValue(string v)
        {
            if (ThrowOnWrite) throw new UnauthorizedAccessException("registry denied");
            if (!DropWrites) Value = v;
        }
        public void DeleteValue()
        {
            if (ThrowOnWrite) throw new UnauthorizedAccessException("registry denied");
            if (!DropWrites) Value = null;
        }
    }

    [Fact]
    public void Autostart_Enable_Verified()
    {
        var store = new FakeStore();
        var svc = new AutostartService(store, () => @"C:\RSH\RSH.exe");
        Assert.True(svc.TrySetAutostart(true, out string? error), error);
        Assert.Equal("\"C:\\RSH\\RSH.exe\" --minimized", store.Value);
        Assert.True(svc.GetAutostart());
    }

    [Fact]
    public void Autostart_Disable_Verified()
    {
        var store = new FakeStore { Value = "\"C:\\RSH\\RSH.exe\" --minimized" };
        var svc = new AutostartService(store, () => @"C:\RSH\RSH.exe");
        Assert.True(svc.TrySetAutostart(false, out string? error), error);
        Assert.Null(store.Value);
        Assert.False(svc.GetAutostart());
    }

    [Fact]
    public void Autostart_WriteThrows_Fails()
    {
        var store = new FakeStore { ThrowOnWrite = true };
        var svc = new AutostartService(store, () => @"C:\RSH\RSH.exe");
        Assert.False(svc.TrySetAutostart(true, out string? error));
        Assert.NotNull(error);
        Assert.False(svc.GetAutostart());
    }

    [Fact]
    public void Autostart_SilentDropDetectedByReadBack()
    {
        // Write "kamyab" lekin read-back purana — verify pakarta hai (A5 ka asal bug).
        var store = new FakeStore { DropWrites = true };
        var svc = new AutostartService(store, () => @"C:\RSH\RSH.exe");
        Assert.False(svc.TrySetAutostart(true, out string? error));
        Assert.NotNull(error);
    }

    [Fact]
    public void Autostart_EmptyExePath_Fails()
    {
        var store = new FakeStore();
        var svc = new AutostartService(store, () => "");
        Assert.False(svc.TrySetAutostart(true, out _));
    }

    // ---------- A8: Debouncer ----------

    // Timing tests: fixed sleep ke bajaye deadline tak poll karo — shared
    // VM par parallel builds ke doran ThreadPool/timer 100 ms delay se
    // zyada late ho sakta hai; test ko load par flaky nahi hona chahiye.
    private static async Task WaitForCountAsync(Func<int> getCount, int expected, int timeoutMs = 5000)
    {
        var deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
        while (DateTime.UtcNow < deadline)
        {
            if (getCount() == expected) return;
            await Task.Delay(25, TestContext.Current.CancellationToken);
        }
        Assert.Equal(expected, getCount());
    }

    [Fact]
    public async Task Debouncer_RapidCalls_RunOnce()
    {
        using var d = new Debouncer(TimeSpan.FromMilliseconds(100));
        int count = 0;
        for (int i = 0; i < 10; i++) d.Debounce(() => Interlocked.Increment(ref count));
        await WaitForCountAsync(() => count, 1);
        // Thora aur ruko — dobara to nahi chala (sirf ek dafa chalna chahiye).
        await Task.Delay(200, TestContext.Current.CancellationToken);
        Assert.Equal(1, count);
    }

    [Fact]
    public async Task Debouncer_SecondBurst_RunsAgain()
    {
        using var d = new Debouncer(TimeSpan.FromMilliseconds(100));
        int count = 0;
        d.Debounce(() => Interlocked.Increment(ref count));
        await WaitForCountAsync(() => count, 1);
        d.Debounce(() => Interlocked.Increment(ref count));
        await WaitForCountAsync(() => count, 2);
    }

    /// <summary>
    /// Re-arm ka deterministic proof: agar timer dobara arm na hota ya
    /// state reset na hoti to burst 2 pehle hi 5 s timeout par fail ho
    /// jata — 20 bursts lagatar chalne chahiye.
    /// </summary>
    [Fact]
    public async Task Debouncer_ManyBursts_EachRunsOnce()
    {
        using var d = new Debouncer(TimeSpan.FromMilliseconds(50));
        int count = 0;
        for (int burst = 1; burst <= 20; burst++)
        {
            d.Debounce(() => Interlocked.Increment(ref count));
            await WaitForCountAsync(() => count, burst);
        }
        Assert.Equal(20, count);
    }

    /// <summary>Callback ke andar se dobara Debounce — timer usi waqt dobara arm hota hai.</summary>
    [Fact]
    public async Task Debouncer_RearmFromCallback_RunsAgain()
    {
        using var d = new Debouncer(TimeSpan.FromMilliseconds(50));
        int count = 0;
        d.Debounce(() =>
        {
            if (Interlocked.Increment(ref count) == 1)
                d.Debounce(() => Interlocked.Increment(ref count));
        });
        await WaitForCountAsync(() => count, 2);
    }

    [Fact]
    public void Debouncer_Flush_RunsImmediately()
    {
        using var d = new Debouncer(TimeSpan.FromSeconds(10));
        int count = 0;
        d.Debounce(() => count++);
        Assert.True(d.HasPending);
        d.Flush();
        Assert.Equal(1, count);
        Assert.False(d.HasPending);
    }
}
