using System.IO.Compression;
using RSH.Core.Profiles;

namespace RSH.Core.Utilities;

/// <summary>
/// SEC-E-07: backup-zip restore ka zip-bomb guard (pure — RSH.Tests se test hota hai).
///
/// Backup zip sirf 3 files ka hota hai (profiles/settings/pin-map), is
/// liye caps sakht hain — aur har cap overwrite se PEHLE lagta hai:
/// cap se zyada ho to reject (khaali ya aadha restore nahi).
///
/// <list type="bullet">
/// <item>entries ≤ 16 (backup mein 3 hoti hain);</item>
/// <item>sirf allowlist naam (<c>profiles.json</c>, <c>settings.json</c>,
/// <c>pin-map.json</c>) — koi directory, koi traversal (<c>..</c>,
/// <c>/</c>, drive), koi extra file nahi (zip-slip band);</item>
/// <item>per-file uncompressed cap: profiles 1 MB
/// (<see cref="ProfileStore.MaxFileBytes"/>), settings/pin-map 64 KB;</item>
/// <item>kul uncompressed ≤ 2 MB;</item>
/// <item>declared <c>Length</c> header jhoot bol sakta hai — asal bytes
/// gin kar copy hote hain (streaming, chunk-wise), cap cross hote hi
/// reject (file kabhi poori RAM mein nahi aati).</item>
/// </list>
/// </summary>
public static class BackupZipGuard
{
    /// <summary>Backup zip mein zyada se zyada entries (asal backup mein 3 hoti hain).</summary>
    public const int MaxBackupEntries = 16;
    /// <summary>Kul uncompressed size cap (2 MB — asal backup chand KB ka hota hai).</summary>
    public const long MaxTotalUncompressedBytes = 2_097_152;
    /// <summary>Zip file khud ki size cap (compressed, 4 MB).</summary>
    public const long MaxZipFileBytes = 4_194_304;
    /// <summary>settings.json / pin-map.json ki per-file cap (64 KB).</summary>
    public const long MaxSmallFileBytes = 65_536;

    /// <summary>Backup zip mein sirf ye naam allowed hain (exact — koi path nahi).</summary>
    public static readonly string[] AllowedEntryNames = ["profiles.json", "settings.json", "pin-map.json"];

    /// <summary>Entry naam ki per-file cap (profiles 1 MB, baqi 64 KB).</summary>
    public static long PerFileCapBytes(string entryName) =>
        entryName == "profiles.json" ? ProfileStore.MaxFileBytes : MaxSmallFileBytes;

    /// <summary>
    /// Backup zip ko caps ke saath <paramref name="destDir"/> mein nikalo.
    /// True = tamam entries mehfooz tareeqe se nikli (kuch overwrite nahi
    /// hua — destDir caller ka temp dir hai); false + <paramref name="error"/>
    /// = zip reject (extract aadha bhi ho to caller temp dir hata deta hai).
    /// </summary>
    public static bool TryExtractBackupZip(string zipPath, string destDir, out string? error)
    {
        error = null;
        try
        {
            var zipInfo = new FileInfo(zipPath);
            if (!zipInfo.Exists)
            {
                error = "Backup file nahi mili.";
                return false;
            }
            if (zipInfo.Length > MaxZipFileBytes)
            {
                error = $"Backup file is too large (max {MaxZipFileBytes / 1024 / 1024} MB).";
                return false;
            }
            Directory.CreateDirectory(destDir);
            using var archive = ZipFile.OpenRead(zipPath);
            var entries = archive.Entries;
            if (entries.Count == 0)
            {
                error = "Backup zip khaali hai.";
                return false;
            }
            if (entries.Count > MaxBackupEntries)
            {
                error = $"Backup zip has {entries.Count} entries — at most {MaxBackupEntries} are allowed (zip-bomb guard, SEC-E-07).";
                return false;
            }
            long total = 0;
            foreach (var entry in entries)
            {
                if (!AllowedEntryNames.Contains(entry.FullName, StringComparer.Ordinal))
                {
                    error = $"Backup zip has an unexpected entry '{entry.FullName}' — only profiles.json, settings.json and pin-map.json are allowed.";
                    return false;
                }
                long cap = PerFileCapBytes(entry.FullName);
                if (entry.Length < 0 || entry.Length > cap)
                {
                    error = $"Backup entry '{entry.FullName}' is too large ({entry.Length} bytes, max {cap} bytes).";
                    return false;
                }
                string destPath = Path.Combine(destDir, entry.FullName);
                long copied = 0;
                using (var source = entry.Open())
                using (var dest = new FileStream(destPath, FileMode.Create, FileAccess.Write, FileShare.None))
                {
                    byte[] buffer = new byte[8192];
                    int read;
                    while ((read = source.Read(buffer, 0, buffer.Length)) > 0)
                    {
                        copied += read;
                        total += read;
                        if (copied > cap || total > MaxTotalUncompressedBytes)
                        {
                            error = $"Backup entry '{entry.FullName}' is too large (uncompressed data exceeds the {cap / 1024} KB cap).";
                            return false;
                        }
                        dest.Write(buffer, 0, read);
                    }
                }
            }
            return true;
        }
        catch (InvalidDataException ex)
        {
            error = $"Backup zip is not valid ({ex.Message}).";
            return false;
        }
        catch (IOException ex)
        {
            error = $"Backup zip could not be read ({ex.Message}).";
            return false;
        }
        catch (UnauthorizedAccessException ex)
        {
            error = $"Backup zip could not be read ({ex.Message}).";
            return false;
        }
        catch (Exception ex)
        {
            error = $"Backup zip could not be read ({ex.Message}).";
            return false;
        }
    }
}
