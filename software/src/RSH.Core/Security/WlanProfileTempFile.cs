// RSH WLAN profile temp file (Phase G C9) — Wi-Fi password (plaintext XML)
// ka secure temp file.
//
// Pehle: WindowsIntegration.ConnectToWheelWifi fixed naam
// `%TEMP%\rsh-wlan.xml` mein plaintext likhta tha aur best-effort delete
// karta tha — crash par file reh jati thi (password samet), naam
// predictable tha (pehle se plant / race ho sakta tha), aur permissions
// default thay (%TEMP% shared hai).
//
// Ab:
//  - Unique naam (`rsh-wlan-<guid>.xml`) — predictable nahi, CreateNew
//    kabhi maujood file ko overwrite nahi karta (symlink plant nahi).
//  - Restricted permissions — Windows: DACL sirf current user (owner-only);
//    Unix: mode 0600. Best-effort hai (fail ho to setup nahi rukta —
//    file phir bhi unique naam + foran delete se mehfooz hai). Tarteeb
//    ahem hai: pehle khaali file, phir restrict, TAB secret likho
//    (BUG-G2 fix) — password kabhi default permissions mein nahi hota.
//  - Secure delete — pehle poora content zeros se overwrite (Flush(true)
//    ke saath), phir delete. `Dispose` idempotent hai; `using` se
//    har rasta (success / failure / exception) delete karta hai.
//  - Crash-safe — crash par file reh bhi jaye to agla Connect
//    `DeleteStaleFiles` se purani files saaf karta hai (10 min grace:
//    zinda run ki file ko haath nahi lagta).
//
// Iman-dari se: overwrite flash (SSD) par best-effort hai — wear-levelling
// purane blocks ko foran mita nahi sakta. Asal hifazat owner-only
// permissions + unique naam + foran delete + stale sweep se hai; password
// bhi per-device random hai (KEY-LIFECYCLE §4), koi shared default nahi.
// netsh ka hissa (Windows-only) yahan nahi — wo WindowsIntegration mein
// hai; ye class sirf file ka kaam karti hai taake sandbox mein test ho sake
// (RSH.App.Tests Linux par chal hi nahi sakta — WindowsDesktop runtime
// nahi; is liye helper RSH.Core mein hai, AutostartService pattern jaisa).
using System.Runtime.Versioning;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;

namespace RSH.Core.Security;

/// <summary>WLAN profile XML (password samet) ke liye secure, crash-safe temp file.</summary>
public sealed class WlanProfileTempFile : IDisposable
{
    /// <summary>Temp files ka prefix — stale sweep sirf isi pattern ko chhooti hai.</summary>
    public const string FilePrefix = "rsh-wlan-";

    /// <summary>Temp files ka suffix.</summary>
    public const string FileSuffix = ".xml";

    /// <summary>Stale sweep ki grace: is se nayi file zinda run ki ho sakti hai — haath mat lagao.</summary>
    public static readonly TimeSpan StaleGrace = TimeSpan.FromMinutes(10);

    /// <summary>Banayi hui file ka poora path.</summary>
    public string Path { get; }

    /// <summary>File delete ho chuki (SecureDelete/Dispose ke baad).</summary>
    public bool IsDeleted { get; private set; }

    private bool _disposed;

    private WlanProfileTempFile(string path) => Path = path;

    /// <summary>
    /// Unique temp file banao aur XML likho. Directory na ho to ban jati hai.
    /// Naam pehle se maujood ho (race/plant) to exception — overwrite kabhi nahi.
    /// Tarteeb: pehle khaali file banao, phir permissions sakht karo, TAB
    /// secret likho — password kabhi default-permissions wali file mein
    /// nahi hota (BUG-G2, W3 verification mein mila: pehle likhna phir
    /// restrict karna ek chhoti window chhorta tha jis mein %TEMP% par
    /// nazar rakhne wala local process password parh sakta tha).
    /// </summary>
    public static WlanProfileTempFile Create(string xmlContent, string? tempDirectory = null)
    {
        ArgumentNullException.ThrowIfNull(xmlContent);
        string dir = string.IsNullOrWhiteSpace(tempDirectory)
            ? System.IO.Path.GetTempPath()
            : tempDirectory;
        Directory.CreateDirectory(dir);
        string path = System.IO.Path.Combine(dir, FilePrefix + Guid.NewGuid().ToString("N") + FileSuffix);
        // Khaali file — is mein abhi koi secret nahi, is liye is lamhe ki
        // default permissions se koi farq nahi parta.
        using (new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None)) { }
        RestrictPermissions(path);
        try
        {
            using (var stream = new FileStream(path, FileMode.Open, FileAccess.Write, FileShare.None))
            {
                byte[] bytes = Encoding.UTF8.GetBytes(xmlContent);
                stream.Write(bytes, 0, bytes.Length);
                stream.Flush(flushToDisk: true);
            }
        }
        catch
        {
            // Aadhi likhi file peeche na chhoro — secret ka hissa bhi na rahe.
            try { File.Delete(path); } catch { /* best-effort */ }
            throw;
        }
        return new WlanProfileTempFile(path);
    }

    /// <summary>
    /// File ke permissions sakht karo — Windows: owner-only DACL; Unix: 0600.
    /// Best-effort: fail ho to kuch nahi hota (caller ko exception nahi).
    /// </summary>
    private static void RestrictPermissions(string path)
    {
        try
        {
            if (OperatingSystem.IsWindows())
                RestrictPermissionsWindows(path);
            else
                File.SetUnixFileMode(path, UnixFileMode.UserRead | UnixFileMode.UserWrite);
        }
        catch
        {
            // Best-effort — unique naam + foran secure delete phir bhi hai.
        }
    }

    /// <summary>
    /// Windows-only DACL (alag method taake Linux par is code ka JIT/load
    /// kabhi na ho — WindowsIdentity/FileSecurity sirf yahan istemal hote hain).
    /// Call site OperatingSystem.IsWindows() se guarded hai; attribute
    /// CA1416 (platform analyzer) ko bhi batata hai (BUG-G1 fix ka hissa).
    /// </summary>
    [SupportedOSPlatform("windows")]
    private static void RestrictPermissionsWindows(string path)
    {
        var security = new FileSecurity();
        // Inheritance band — %TEMP% ki default ACL (jo aur logon ko bhi
        // parhne de sakti hai) yahan na aaye; sirf current user.
        security.SetAccessRuleProtection(isProtected: true, preserveInheritance: false);
        var user = WindowsIdentity.GetCurrent().User;
        if (user is not null)
            security.AddAccessRule(new FileSystemAccessRule(user, FileSystemRights.FullControl, AccessControlType.Allow));
        // .NET 5+ mein static File.SetAccessControl nahi hai (sirf
        // .NET Framework / purana package) — FileInfo instance method hai.
        // Ye latent compile error thi (net10.0 par clean build se mili —
        // Phase G build mein expose hui; BUG-G1).
        new FileInfo(path).SetAccessControl(security);
    }

    /// <summary>
    /// Poora content zeros se overwrite karo (length wahi rehti hai),
    /// disk par flush ke saath. File na ho to exception.
    /// </summary>
    public void OverwriteWithZeros()
    {
        if (IsDeleted) return;
        using var stream = new FileStream(Path, FileMode.Open, FileAccess.Write, FileShare.None);
        byte[] zeros = new byte[4096];
        long remaining = stream.Length;
        while (remaining > 0)
        {
            int n = (int)Math.Min(zeros.Length, remaining);
            stream.Write(zeros, 0, n);
            remaining -= n;
        }
        stream.Flush(flushToDisk: true);
    }

    /// <summary>
    /// Secure delete: pehle overwrite (best-effort), phir delete.
    /// Idempotent — dobara bulao to true, koi exception nahi.
    /// True = file ab maujood nahi (pehle se hi na ho to bhi true).
    /// </summary>
    public bool SecureDelete()
    {
        if (IsDeleted) return true;
        try { OverwriteWithZeros(); }
        catch { /* overwrite best-effort — delete phir bhi karo */ }
        try { File.Delete(Path); }
        catch { /* delete fail — neeche check batayega */ }
        IsDeleted = !File.Exists(Path);
        return IsDeleted;
    }

    /// <summary>Secure delete (idempotent, kabhi throw nahi karta).</summary>
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        try { SecureDelete(); }
        catch { /* Dispose kabhi throw nahi karta */ }
    }

    /// <summary>
    /// Crash-safe sweep: `rsh-wlan-*.xml` pattern ki PURANI files (grace se
    /// zyada purani) overwrite + delete karo. Zinda run ki nayi file,
    /// doosre naam ki files, aur locked files ko haath nahi lagta.
    /// Return: kitni files delete hui.
    /// </summary>
    public static int DeleteStaleFiles(string? tempDirectory = null)
    {
        string dir = string.IsNullOrWhiteSpace(tempDirectory)
            ? System.IO.Path.GetTempPath()
            : tempDirectory;
        if (!Directory.Exists(dir)) return 0;
        int deleted = 0;
        DateTime cutoff = DateTime.UtcNow - StaleGrace;
        foreach (string file in Directory.EnumerateFiles(dir, FilePrefix + "*" + FileSuffix))
        {
            try
            {
                if (File.GetLastWriteTimeUtc(file) >= cutoff) continue;   // zinda run ki ho sakti hai
                try
                {
                    using var stream = new FileStream(file, FileMode.Open, FileAccess.Write, FileShare.None);
                    byte[] zeros = new byte[4096];
                    long remaining = stream.Length;
                    while (remaining > 0)
                    {
                        int n = (int)Math.Min(zeros.Length, remaining);
                        stream.Write(zeros, 0, n);
                        remaining -= n;
                    }
                    stream.Flush(flushToDisk: true);
                }
                catch { /* overwrite best-effort */ }
                File.Delete(file);
                deleted++;
            }
            catch { /* locked / race — agli dafa sahi */ }
        }
        return deleted;
    }
}
