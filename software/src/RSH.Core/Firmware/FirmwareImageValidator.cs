using System.Net;
using System.Security.Cryptography;
using System.Text;
using NSec.Cryptography;

namespace RSH.Core.Firmware;

/// <summary>
/// RSH firmware image (.bin) ki upload se pehle validation — A1 + security
/// review SEC-C-01/02/05/07/09.
///
/// Checks, sab in-memory <c>byte[]</c> par (SEC-C-09 TOCTOU: file ek dafa
/// parhi jati hai, validate wahi buffer hota hai jo bheja jata hai):
/// <list type="bullet">
/// <item>Size — cap 2 MB (SEC-C-07; app partition 0x140000 = 1,310,720 B
/// se bara image bhi reject); chhoti/khaali file reject.</item>
/// <item>Magic — ESP32 app-image magic <c>0xE9</c> image ke shuru mein
/// (SEC-C-01). Merged/factory image (app 0x10000 par) pehchani jati hai
/// aur reject hoti hai — <c>/update</c> ko app-only <c>firmware.bin</c>
/// chahiye, merged image board par fail hogi.</item>
/// <item>SHA — ESP-IDF app image ke aakhir mein 32-byte SHA-256 digest
/// (padded image par) dobara compute kar ke milaya jata hai. Ye
/// <b>integrity</b> check hai, authenticity nahi (SEC-D-04): hamlaawar
/// sahi SHA ke saath ghalat image bhi bana sakta hai. Asliyat sirf
/// Ed25519 signature se sabit hoti hai, jo board verify karta hai
/// (neeche, SEC-C-06).</item>
/// <item>Version — image ke andar WebOTA page ke chip
/// <c>Version &lt;b&gt;X.Y.Z&lt;/b&gt;</c> se nikali jati hai. Note:
/// <c>esp_app_desc.version</c> ESP-IDF framework ka version hai
/// ("esp-idf: v4.4.7..."), RSH firmware version nahi — use kabhi
/// version ke tor par na parho. Anti-downgrade (SEC-C-05): version
/// &lt; 4.0.0 reject hoti hai. Live GET_CAPS se board ka current version
/// milana Phase D mein aa gaya hai (GetCapsClient + FirmwareUploadGate —
/// upload se pehle compare, upload ke baad confirm); session key na ho
/// (SEC-D-05) to sirf ye floor check rehta hai.
/// <b>SEC-D-04 note:</b> version marker sirf version batata hai,
/// authenticity nahi — hamlaawar marker daal kar version <c>9.9.9</c>
/// likh sakta hai. Marker check signature ka badal nahi hai.</item>
/// <item>Signature trailer — file ke aakhir mein 68 bytes
/// (<c>[Ed25519 sig 64 B]["RSHS"]</c>, docs/SIGNED-OTA.md). Trailer na
/// ho to image unsigned hai → reject. Signature ki cryptographic
/// verify AB board-side (webota.cpp, verify-before-apply) AUR app-side
/// dono hoti hai (SEC-C-06 app-side, defense-in-depth): app
/// <see cref="VerifySignature"/> se baked release public key se
/// signature check karta hai — ghalat signature wali image fail-closed
/// reject hoti hai, board tak kabhi nahi pahunchti. BCL (.NET 10) mein
/// Ed25519 verify nahi hai, is liye NSec (libsodium) istemal hota hai.</item>
/// </list>
/// </summary>
public static class FirmwareImageValidator
{
    /// <summary>Upload file ka size cap (SEC-C-07) — is se bari file parhi hi nahi jati.</summary>
    public const int MaxFileSizeBytes = 2 * 1024 * 1024;

    /// <summary>App partition (default.csv app0/app1) ka size — image is se bara nahi ho sakta.</summary>
    public const int AppPartitionSizeBytes = 0x140000;

    /// <summary>Chhoti se chhoti maqool image (header + 1 segment + SHA).</summary>
    public const int MinFileSizeBytes = 64 * 1024;

    /// <summary>Anti-downgrade floor (SEC-C-05) — is se purani firmware kabhi upload nahi hoti.</summary>
    public static readonly Version MinimumFirmwareVersion = new(4, 0, 0);

    /// <summary>Current shipping firmware version — PC-side source of
    /// truth. <c>firmware/rsh/config.h</c> ke <c>FIRMWARE_VERSION</c> se
    /// match lazmi hai; firmware bump par dono badlo (W4 Round 2: header
    /// jaisi hard-coded version strings dobara stale na hon — version
    /// yahan se lo, literal kahin aur na likho).</summary>
    public static readonly Version CurrentFirmwareVersion = new(4, 0, 1);

    /// <summary>Merged/factory image mein app image ka offset (partitions.csv: app0 @ 0x10000).</summary>
    public const int MergedAppOffset = 0x10000;

    /// <summary>Merged image mein bootloader ka offset (esptool merge_bin layout).</summary>
    public const int MergedBootloaderOffset = 0x1000;

    /// <summary>Signature trailer ka size (SEC-C-06): 64 B Ed25519 sig + 4 B magic.</summary>
    public const int SignatureTrailerSizeBytes = 68;

    /// <summary>Signature trailer ke aakhri 4 bytes.</summary>
    public static readonly byte[] SignatureMagicBytes = Encoding.ASCII.GetBytes("RSHS");

    /// <summary>
    /// Release signing public key (Ed25519, 32 B) — wahi jo
    /// <c>firmware/rsh/signing_pubkey.h</c> mein <c>kRshSigningPubkey</c> hai.
    /// Public key hai (secret nahi) — app mein bake karna mehfooz hai.
    /// SHA-256 fingerprint: 387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411.
    /// *** KEY ROTATION: key badle to DONO jagah lazmi update karo —
    /// firmware/rsh/signing_pubkey.h AUR ye field — warna board aur app
    /// alag keys se verify karenge aur signed images ek taraf reject
    /// hongi. Nayi key par fingerprint test
    /// (PhaseCAppTests.ReleasePubkey_Fingerprint_Matches) bhi update karo.
    /// </summary>
    internal static readonly byte[] ReleaseSigningPublicKey = Convert.FromHexString(
        "ba6f53e0adaf98295c86fef5819e85028def02b320b89955d9dee09afdce2ce7");

    /// <summary>Release signing public key ka SHA-256 fingerprint (hex, lowercase) — key-copy typo / rotation drift ka guard.</summary>
    internal const string ReleaseSigningPublicKeyFingerprintHex =
        "387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411";

    private const byte EspImageMagic = 0xE9;
    private const int ImageHeaderSize = 24;
    private const int SegmentHeaderSize = 8;
    private const int ImageShaSize = 32;

    /// <summary>Version chip jo RSH firmware ke WebOTA page mein hoti hai.</summary>
    private static readonly byte[] VersionMarker = Encoding.ASCII.GetBytes("Version <b>");

    public enum ImageLayout { Unknown, AppOnly, Merged }

    public sealed record ValidationResult(
        bool IsValid,
        string? Error,
        ImageLayout Layout,
        Version? FirmwareVersion,
        string FileSha256Hex,
        string? ImageSha256Hex,
        int SizeBytes,
        bool IsSigned);

    /// <summary>
    /// Ed25519 signature verify karo (sign-image.py / ota_validate.h wali
    /// construction): signature <c>Ed25519(privkey, SHA-256(image bytes))</c>
    /// hai — is liye pehle image ka SHA-256 nikalo, phir us digest par
    /// signature verify karo (digest hi signed message hai, poori image nahi).
    /// Ghalat length / ghalat key / crypto exception → false (fail-closed,
    /// kabhi throw nahi).
    /// </summary>
    internal static bool VerifySignature(ReadOnlySpan<byte> imageBytes, ReadOnlySpan<byte> signature, ReadOnlySpan<byte> publicKey)
    {
        if (signature.Length != 64 || publicKey.Length != 32) return false;
        try
        {
            byte[] digest = SHA256.HashData(imageBytes);
            var key = PublicKey.Import(SignatureAlgorithm.Ed25519, publicKey, KeyBlobFormat.RawPublicKey);
            return SignatureAlgorithm.Ed25519.Verify(key, digest, signature);
        }
        catch (Exception ex) when (ex is FormatException or CryptographicException or InvalidOperationException)
        {
            return false;
        }
    }

    /// <summary>Buffer validate karo (baked release public key se signature verify). Koi file I/O nahi — caller buffer ek dafa parh kar de.</summary>
    public static ValidationResult Validate(byte[] image) => Validate(image, ReleaseSigningPublicKey);

    /// <summary>
    /// Buffer validate karo, di hui public key se signature verify (tests ke
    /// liye — throwaway keypair se signed test images; production rasta
    /// <see cref="Validate(byte[])"/> hai jo baked release key istemal karta hai).
    /// </summary>
    internal static ValidationResult Validate(byte[] image, ReadOnlySpan<byte> publicKey)
    {
        string fileSha = Convert.ToHexString(SHA256.HashData(image)).ToLowerInvariant();
        ValidationResult Fail(string error, ImageLayout layout, Version? version = null, string? imageSha = null) =>
            new(false, error, layout, version, fileSha, imageSha, image.Length, false);

        if (image.Length < MinFileSizeBytes)
            return Fail($"File bohat chhoti hai ({image.Length:N0} bytes) — ye RSH firmware image nahi lagti.", ImageLayout.Unknown);
        if (image.Length > MaxFileSizeBytes)
            return Fail($"File bohat bari hai ({image.Length:N0} bytes, cap {MaxFileSizeBytes:N0}) — upload nahi hogi.", ImageLayout.Unknown);

        // Layout pehchano: app-only image magic offset 0 par; merged image
        // 0xFF padding se shuru hoti hai, bootloader 0x1000 par, app 0x10000 par.
        ImageLayout layout;
        int appOffset;
        if (image[0] == EspImageMagic)
        {
            layout = ImageLayout.AppOnly;
            appOffset = 0;
        }
        else if (image.Length > MergedAppOffset + ImageHeaderSize
                 && image[MergedAppOffset] == EspImageMagic
                 && image[MergedBootloaderOffset] == EspImageMagic)
        {
            return Fail("Ye merged/factory image hai (bootloader + partitions + app) — /update ko app-only firmware.bin chahiye. Release se firmware.bin chuno, merged file nahi.",
                ImageLayout.Merged);
        }
        else
        {
            return Fail("Magic byte ghalat hai — file ESP32 firmware image nahi lagti (pehla byte 0xE9 hona chahiye).", ImageLayout.Unknown);
        }

        // Segment table parse karo (bounds-checked) aur image ka end nikalo.
        if (image.Length < appOffset + ImageHeaderSize)
            return Fail("File header ke liye bohat chhoti hai.", layout);
        int segmentCount = image[appOffset + 1];
        if (segmentCount is < 1 or > 16)
            return Fail($"Segment count na-jaiz hai ({segmentCount}) — image kharab hai.", layout);

        int cursor = appOffset + ImageHeaderSize;
        for (int s = 0; s < segmentCount; s++)
        {
            if (cursor + SegmentHeaderSize > image.Length)
                return Fail("Segment header file se bahir hai — image aadhi/kharab hai.", layout);
            uint dataLen = BitConverter.ToUInt32(image, cursor + 4);
            cursor += SegmentHeaderSize;
            if (dataLen > image.Length - cursor)
                return Fail($"Segment {s} ki length ({dataLen:N0}) file se bahir hai — image aadhi/kharab hai.", layout);
            cursor += (int)dataLen;
        }

        // ESP-IDF image: segments ke baad checksum byte, phir 32-byte SHA-256.
        // esptool (bin_image.py append_checksum + save): checksum byte 16-byte
        // block ke AAKHRI byte par hota hai — align (15 - cursor%16) bytes
        // aage, phir 1 byte — aur digest us ke BAAD shuru hota hai aur
        // [appOffset..digestStart) ko cover karta hai (checksum byte samet).
        // digestStart = (cursor + 16) & ~15. Ghalat assumption (digest seedha
        // (cursor+15)&~15 par) sirf tab kaam karti hai jab cursor%16 != 0 —
        // jab segments 16-byte boundary par khatam hon (jaise asal
        // firmware.bin: cursor 822144) to digest 16 bytes aage hota hai aur
        // purana code "SHA-256 mismatch" se valid image reject karta tha
        // (SEC-E-01 gate mein mila — Phase F W1; esptool source + asal image
        // par SHA verify kar ke sabit hua). Ye fail-closed tha (valid reject,
        // ghalat kabhi accept nahi), lekin upload ka rasta band karta tha.
        int digestStart = (cursor + 16) & ~15;
        if (digestStart + ImageShaSize > image.Length)
            return Fail("Image ka SHA-256 digest maujood nahi — file aadhi download hui lagti hai.", layout);
        if (digestStart + ImageShaSize > appOffset + AppPartitionSizeBytes)
            return Fail($"Image app partition ({AppPartitionSizeBytes:N0} bytes) se bari hai — board par fit nahi hogi.", layout);

        byte[] storedSha = image.AsSpan(digestStart, ImageShaSize).ToArray();
        byte[] computedSha = SHA256.HashData(image.AsSpan(appOffset, digestStart - appOffset));
        if (!CryptographicOperations.FixedTimeEquals(storedSha, computedSha))
            return Fail("SHA-256 mismatch — file badli hui ya aadhi-downloaded hai. Dobara download karo.", layout);
        string imageShaHex = Convert.ToHexString(computedSha).ToLowerInvariant();

        // Version: WebOTA page chip se. esp_app_desc.version istemal nahi
        // hota (wo ESP-IDF framework ka version hai, RSH ka nahi).
        Version? version = ExtractVersion(image, appOffset, digestStart);
        if (version is null)
            return Fail("Image mein RSH firmware version nahi mila — ye RSH firmware nahi lagti ya file kharab hai.", layout, imageSha: imageShaHex);
        if (version < MinimumFirmwareVersion)
            return Fail($"Downgrade reject: file firmware v{version} hai, minimum v{MinimumFirmwareVersion} chahiye (purani firmware mein maloom bugs hain).", layout, version, imageShaHex);

        // Signed trailer (SEC-C-06 / SEC-D-04): file ke aakhir mein 68 bytes
        // [Ed25519 sig 64 B]["RSHS"] hone chahiye. Trailer na ho to image
        // unsigned hai → reject (board waise bhi reject karega; yahan
        // fail-fast behtar hai). Trailer ho to signature CRYPTOGRAPHICALLY
        // verify hoti hai (app-side, defense-in-depth — board-side verify
        // webota.cpp mein rehti hai): ghalat signature → fail-closed reject.
        if (image.Length < digestStart + ImageShaSize + SignatureTrailerSizeBytes
            || !image.AsSpan(image.Length - SignatureMagicBytes.Length, SignatureMagicBytes.Length)
                .SequenceEqual(SignatureMagicBytes))
            return Fail("Image signed nahi hai — sirf signed official RSH release upload karo (signature trailer nahi mila).", layout, version, imageShaHex);
        if (!VerifySignature(image.AsSpan(0, image.Length - SignatureTrailerSizeBytes),
                image.AsSpan(image.Length - SignatureTrailerSizeBytes, 64), publicKey))
            return Fail("Image ki Ed25519 signature verify nahi hui — file badli hui ya kisi aur key se signed hai. Sirf official RSH release upload karo.", layout, version, imageShaHex);

        return new ValidationResult(true, null, layout, version, fileSha, imageShaHex, image.Length, true);
    }

    private static Version? ExtractVersion(byte[] image, int from, int to)
    {
        int searchEnd = to - VersionMarker.Length;
        for (int i = from; i <= searchEnd; i++)
        {
            if (!image.AsSpan(i, VersionMarker.Length).SequenceEqual(VersionMarker))
                continue;
            // Marker ke baad version seedha nahi aata: webota.cpp alag
            // string literals jorta hai ("Version <b>" + FIRMWARE_VERSION),
            // is liye darmiyan mein NUL byte hota hai: "Version <b>\0 4.0.0\0".
            // Pehle digit tak skip karo, phir digit/. ka run parho.
            int p = i + VersionMarker.Length;
            int skipLimit = Math.Min(to, p + 8);
            while (p < skipLimit && (image[p] < (byte)'0' || image[p] > (byte)'9'))
                p++;
            int vStart = p;
            while (p < to && p - vStart < 16
                   && ((image[p] >= (byte)'0' && image[p] <= (byte)'9') || image[p] == (byte)'.'))
                p++;
            string text = Encoding.ASCII.GetString(image, vStart, p - vStart).Trim().TrimEnd('.');
            if (Version.TryParse(text, out var version) && version.Major > 0)
                return version;
            return null; // marker mila lekin version parse nahi hui — wazeh reject
        }
        return null;
    }
}
