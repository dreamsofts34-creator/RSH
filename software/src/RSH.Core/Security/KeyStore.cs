// RSH key store (Phase E WS5, KEY-LIFECYCLE.md §4, PROTOCOL-V2.1.md §4.1/§5.3)
// — device key (32 B) + Wi-Fi/OTA password ka app-side store.
//
// Windows: DPAPI, CurrentUser scope (CryptProtectData, UI_FORBIDDEN) —
// blob `%AppData%\RSH\keys\<boardId>.key` / `.pwd` mein rehta hai. DPAPI
// user-bound hai, app-bound nahi (KEY-LIFECYCLE §4): usi Windows user ke
// tor par chalne wala koi bhi process decrypt kar sakta hai — is liye key
// ki hifazat ka asal inhesar Windows user account + PC ki hifazat par hai.
// DPAPI machine reinstall / user profile delete hone par key KHO JATI hai
// — hal re-pairing hai (§6), backup nahi. Koi export / cloud sync nahi.
//
// Non-Windows (Linux sandbox samet): DPAPI maujood nahi — store sirf
// in-memory rehta hai (IsPersistent == false), koi crash nahi, koi
// plaintext file kabhi nahi banti. Is ka matlab: Linux par key/password
// process khatam hote hi bhool jate hain — dobara pairing lazmi hai.
//
// BoardId: board MAC ke 12 hex chars (lowercase, baghair separator) —
// D21 binding ke liye BoardMac se banta hai; key/password kabhi MAC se
// derive NAHI hote (R-H10), MAC sirf file ka naam hai.
//
// Graceful degrade: save/load fail ho (DPAPI error, IO error) to false
// milta hai, exception nahi — caller (BoardSession) session key se kaam
// chalata hai aur user ko saaf wajah batata hai. Sirf programmer errors
// (ghalat boardId format, key 32 B ki nahi, null) exception dete hain.
using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using System.Text;

namespace RSH.Core.Security;

/// <summary>Device key + Wi-Fi/OTA password ka store (boardId = MAC hex, 12 chars).</summary>
public interface IKeyStore
{
    /// <summary>True = store restart ke baad bhi rehta hai (Windows DPAPI). False = sirf in-memory (non-Windows).</summary>
    bool IsPersistent { get; }

    /// <summary>32 B device key save karo. Fail ho to false (ghalat key length = ArgumentException).</summary>
    bool TrySaveKey(string boardId, byte[] key);
    /// <summary>Device key load karo. Na mile ya kharab ho to false + key null.</summary>
    bool TryLoadKey(string boardId, out byte[]? key);
    /// <summary>Wi-Fi/OTA password save karo (DPAPI blob, §5.3). Fail ho to false.</summary>
    bool TrySavePassword(string boardId, string password);
    /// <summary>Wi-Fi/OTA password load karo. Na mile ya kharab ho to false + password null.</summary>
    bool TryLoadPassword(string boardId, out string? password);
    /// <summary>Dono blobs mitayo (key + password). Idempotent — kuch na ho to bhi true.</summary>
    bool Delete(string boardId);
}

/// <summary>
/// DPAPI key store (Windows) / in-memory store (non-Windows).
/// P/Invoke (crypt32) istemal hota hai taake System.Security.Cryptography.
/// ProtectedData NuGet package ki zaroorat na pare — RSH.Core ka koi
/// naya package dependency nahi hai.
/// </summary>
public sealed class KeyStore : IKeyStore, IDisposable
{
    private const uint CryptProtectUiForbidden = 0x1;

    [StructLayout(LayoutKind.Sequential)]
    private struct DataBlob
    {
        public int cbData;
        public IntPtr pbData;
    }

    [DllImport("crypt32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CryptProtectData(
        ref DataBlob pDataIn, string? szDataDescr, IntPtr pOptionalEntropy,
        IntPtr pvReserved, IntPtr pPromptStruct, uint dwFlags, out DataBlob pDataOut);

    [DllImport("crypt32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CryptUnprotectData(
        ref DataBlob pDataIn, out string? pszDataDescr, IntPtr pOptionalEntropy,
        IntPtr pvReserved, IntPtr pPromptStruct, uint dwFlags, out DataBlob pDataOut);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LocalFree(IntPtr hMem);

    private readonly string _root;
    private readonly ConcurrentDictionary<string, byte[]> _memoryKeys = new(StringComparer.OrdinalIgnoreCase);
    private readonly ConcurrentDictionary<string, string> _memoryPasswords = new(StringComparer.OrdinalIgnoreCase);
    private bool _disposed;

    /// <summary>
    /// DATA-AI FIX: DPAPI blob ki size cap (64 KB — asal blob chand sau
    /// bytes ka hota hai). Pehle File.ReadAllBytes uncapped tha: koi bhi
    /// local process keys folder mein GBs ka planted blob rakh kar OOM
    /// kara sakta tha, aur OutOfMemoryException BoardSession ke catch
    /// (sirf ObjectDisposed/InvalidOperation) se bahir nikal jata tha.
    /// Cap se bara blob kharab mana jata hai (false — re-pairing).
    /// </summary>
    public const long MaxBlobBytes = 65_536;

    public bool IsPersistent { get; }

    /// <summary>Default root: Windows `%AppData%\RSH\keys`, warna `~/.config/RSH/keys` (sirf naam — non-Windows par file banti hi nahi).</summary>
    public KeyStore() : this(DefaultRoot())
    {
    }

    /// <summary>Tests ke liye: root directory do (Windows par DPAPI blob wahin likha jata hai).</summary>
    internal KeyStore(string rootDirectory)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(rootDirectory);
        _root = rootDirectory;
        IsPersistent = OperatingSystem.IsWindows();
    }

    private static string DefaultRoot()
    {
        if (OperatingSystem.IsWindows())
            return Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "RSH", "keys");
        string? xdg = Environment.GetEnvironmentVariable("XDG_CONFIG_HOME");
        string baseDir = string.IsNullOrWhiteSpace(xdg)
            ? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".config")
            : xdg;
        return Path.Combine(baseDir, "RSH", "keys");
    }

    /// <summary>Board MAC (6 B) se store boardId banao: 12 lowercase hex chars. MAC sirf naam hai — key/password is se derive nahi hote (R-H10).</summary>
    public static string BoardIdForMac(byte[] boardMac)
    {
        ArgumentNullException.ThrowIfNull(boardMac);
        if (boardMac.Length != 6) throw new ArgumentException("Board MAC 6 bytes ka hona chahiye.", nameof(boardMac));
        return Convert.ToHexString(boardMac).ToLowerInvariant();
    }

    private static string ValidateBoardId(string boardId)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(boardId);
        if (boardId.Length != 12) throw new ArgumentException("BoardId 12 hex characters ka hona chahiye (MAC hex).", nameof(boardId));
        foreach (char ch in boardId)
            if (!Uri.IsHexDigit(ch)) throw new ArgumentException("BoardId mein sirf hex characters ho sakte hain.", nameof(boardId));
        return boardId.ToLowerInvariant();
    }

    public bool TrySaveKey(string boardId, byte[] key)
    {
        string id = ValidateBoardId(boardId);
        ArgumentNullException.ThrowIfNull(key);
        if (key.Length != 32) throw new ArgumentException("Device key 32 bytes ki honi chahiye (§4.1).", nameof(key));
        return TrySave(id, ".key", key);
    }

    public bool TryLoadKey(string boardId, out byte[]? key)
    {
        string id = ValidateBoardId(boardId);
        key = null;
        if (!TryLoad(id, ".key", out byte[]? plain)) return false;
        if (plain is null || plain.Length != 32) { Clear(plain); return false; }
        key = plain;
        return true;
    }

    public bool TrySavePassword(string boardId, string password)
    {
        string id = ValidateBoardId(boardId);
        ArgumentNullException.ThrowIfNull(password);
        char[] chars = password.ToCharArray();
        byte[] bytes = Encoding.UTF8.GetBytes(chars);
        try
        {
            return TrySave(id, ".pwd", bytes);
        }
        finally
        {
            Array.Clear(chars);
            Array.Clear(bytes);
        }
    }

    public bool TryLoadPassword(string boardId, out string? password)
    {
        string id = ValidateBoardId(boardId);
        password = null;
        if (!TryLoad(id, ".pwd", out byte[]? plain)) return false;
        if (plain is null) return false;
        try
        {
            password = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true)
                .GetString(plain);
            return true;
        }
        catch (DecoderFallbackException)
        {
            return false;
        }
        finally
        {
            Clear(plain);
        }
    }

    public bool Delete(string boardId)
    {
        string id = ValidateBoardId(boardId);
        bool ok = true;
        if (_memoryKeys.TryRemove(id, out byte[]? k)) Clear(k);
        _memoryPasswords.TryRemove(id, out _);
        if (!IsPersistent) return ok;
        try
        {
            string keyPath = Path.Combine(_root, id + ".key");
            string pwdPath = Path.Combine(_root, id + ".pwd");
            if (File.Exists(keyPath)) File.Delete(keyPath);
            if (File.Exists(pwdPath)) File.Delete(pwdPath);
        }
        catch (IOException) { ok = false; }
        catch (UnauthorizedAccessException) { ok = false; }
        return ok;
    }

    private bool TrySave(string boardId, string suffix, byte[] plain)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        if (!IsPersistent)
        {
            // Non-Windows: sirf in-memory (koi plaintext file nahi).
            if (suffix == ".key") _memoryKeys[boardId] = plain.ToArray();
            else _memoryPasswords[boardId] = Encoding.UTF8.GetString(plain);
            return true;
        }
        byte[]? blob = Protect(plain);
        if (blob is null) return false;
        try
        {
            Directory.CreateDirectory(_root);
            // SEC-E-09 (rotation hygiene): blob ko seedha overwrite mat
            // karo — aadha likha blob agle load par "kharab blob" ban kar
            // key/password dono kho deta (fail-closed to re-pairing, lekin
            // rotation ke beech mein crash se bachna behtar hai). Temp file
            // + atomic move: crash par ya to purana blob rehta hai ya naya.
            string target = Path.Combine(_root, boardId + suffix);
            string temp = target + ".tmp-" + Guid.NewGuid().ToString("N");
            try
            {
                File.WriteAllBytes(temp, blob);
                File.Move(temp, target, overwrite: true);
            }
            catch
            {
                try { if (File.Exists(temp)) File.Delete(temp); } catch (IOException) { } catch (UnauthorizedAccessException) { }
                throw;
            }
            return true;
        }
        catch (IOException) { return false; }
        catch (UnauthorizedAccessException) { return false; }
        finally
        {
            Clear(blob);
        }
    }

    private bool TryLoad(string boardId, string suffix, out byte[]? plain)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        plain = null;
        if (!IsPersistent)
        {
            if (suffix == ".key")
            {
                if (!_memoryKeys.TryGetValue(boardId, out byte[]? k)) return false;
                plain = k.ToArray();
                return true;
            }
            if (!_memoryPasswords.TryGetValue(boardId, out string? p)) return false;
            plain = Encoding.UTF8.GetBytes(p);
            return true;
        }
        byte[] blob;
        try
        {
            string path = Path.Combine(_root, boardId + suffix);
            if (!File.Exists(path)) return false;
            // Capped read (MaxBlobBytes) — File.ReadAllBytes kabhi poori
            // file RAM mein na laye (upar dekho).
            using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
            if (stream.Length > MaxBlobBytes) return false;
            using var capped = new MemoryStream();
            byte[] buffer = new byte[8192];
            int read;
            long total = 0;
            while ((read = stream.Read(buffer, 0, buffer.Length)) > 0)
            {
                total += read;
                if (total > MaxBlobBytes) return false;
                capped.Write(buffer, 0, read);
            }
            blob = capped.ToArray();
        }
        catch (IOException) { return false; }
        catch (UnauthorizedAccessException) { return false; }
        catch (OutOfMemoryException) { return false; }
        plain = Unprotect(blob);
        Clear(blob);
        return plain is not null;
    }

    /// <summary>DPAPI protect (Windows only — caller IsPersistent check karta hai). Fail ho to null.</summary>
    private static byte[]? Protect(byte[] plain)
    {
        IntPtr inPtr = IntPtr.Zero;
        DataBlob outBlob = default;
        try
        {
            inPtr = Marshal.AllocHGlobal(plain.Length);
            Marshal.Copy(plain, 0, inPtr, plain.Length);
            var inBlob = new DataBlob { cbData = plain.Length, pbData = inPtr };
            if (!CryptProtectData(ref inBlob, null, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero,
                    CryptProtectUiForbidden, out outBlob))
                return null;
            if (outBlob.cbData <= 0 || outBlob.pbData == IntPtr.Zero) return null;
            var result = new byte[outBlob.cbData];
            Marshal.Copy(outBlob.pbData, result, 0, outBlob.cbData);
            return result;
        }
        catch (DllNotFoundException) { return null; }
        catch (EntryPointNotFoundException) { return null; }
        finally
        {
            if (inPtr != IntPtr.Zero)
            {
                Marshal.Copy(new byte[plain.Length], 0, inPtr, plain.Length);
                Marshal.FreeHGlobal(inPtr);
            }
            if (outBlob.pbData != IntPtr.Zero) LocalFree(outBlob.pbData);
        }
    }

    /// <summary>DPAPI unprotect (Windows only). Fail ho to null.</summary>
    private static byte[]? Unprotect(byte[] blob)
    {
        IntPtr inPtr = IntPtr.Zero;
        DataBlob outBlob = default;
        try
        {
            inPtr = Marshal.AllocHGlobal(blob.Length);
            Marshal.Copy(blob, 0, inPtr, blob.Length);
            var inBlob = new DataBlob { cbData = blob.Length, pbData = inPtr };
            if (!CryptUnprotectData(ref inBlob, out _, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero,
                    CryptProtectUiForbidden, out outBlob))
                return null;
            if (outBlob.cbData <= 0 || outBlob.pbData == IntPtr.Zero) return null;
            var result = new byte[outBlob.cbData];
            Marshal.Copy(outBlob.pbData, result, 0, outBlob.cbData);
            return result;
        }
        catch (DllNotFoundException) { return null; }
        catch (EntryPointNotFoundException) { return null; }
        finally
        {
            if (inPtr != IntPtr.Zero) Marshal.FreeHGlobal(inPtr);
            if (outBlob.pbData != IntPtr.Zero) LocalFree(outBlob.pbData);
        }
    }

    private static void Clear(byte[]? bytes)
    {
        if (bytes is not null) Array.Clear(bytes);
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        foreach (byte[] k in _memoryKeys.Values) Clear(k);
        _memoryKeys.Clear();
        _memoryPasswords.Clear();
    }
}
