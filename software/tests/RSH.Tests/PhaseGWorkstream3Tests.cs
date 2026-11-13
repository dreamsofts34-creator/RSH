using RSH.Core.Security;

namespace RSH.Tests;

/// <summary>
/// Phase G — Workstream 3: C8 UI labels (source-text — WPF app Linux par
/// run nahi hota, is liye RSH.App ki cheezein source par check hoti hain;
/// PhaseFWorkstream6Tests pattern) + C9 secure WLAN temp file (behavioral,
/// RSH.Core) + C10 dt flake ka test yahan nahi — wo EngineStopDtTests mein
/// hai (median + dheeli mean hadd).
/// </summary>
public class PhaseGWorkstream3Tests
{
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

    private static string ReadRepoFileOrSkip(string relative)
    {
        string? path = FindRepoFile(relative);
        if (path is null) Assert.Skip($"Repo file nahi mili (test run ki layout alag hai): {relative}");
        return File.ReadAllText(path);
    }

    private static string NewTempDir()
    {
        string dir = Path.Combine(Path.GetTempPath(), "rsh-g-ws3-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        return dir;
    }

    // ================= C8 — UI labels sach hon =================

    [Fact]
    public void C8_TuningLabels_HandbrakeShowsSlider()
    {
        string tuning = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "CalibrationTuning.cs"));
        Assert.Contains("vJoy Slider", tuning, StringComparison.Ordinal);
        Assert.DoesNotContain("no vJoy mapping yet", tuning);
        // Mini-card: v2 par asal % (HandbrakeNorm), "no input" nahi.
        Assert.Contains("state.HandbrakeNorm", tuning, StringComparison.Ordinal);
        Assert.Contains("state.ClutchNorm", tuning, StringComparison.Ordinal);
        // "Not sending" note sirf tab jab v2 stream na ho.
        Assert.Contains("!HasV2Stream", tuning, StringComparison.Ordinal);
    }

    [Fact]
    public void C8_WindowsIntegration_UsesSecureTempFile_NotFixedName()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "Services", "WindowsIntegration.cs"));
        Assert.Contains("WlanProfileTempFile", src, StringComparison.Ordinal);
        Assert.Contains("DeleteStaleFiles", src, StringComparison.Ordinal);
        // Purani fixed-path construction khatam (comment mein zikr rehta hai — wo theek hai).
        Assert.DoesNotContain("GetTempPath(), \"rsh-wlan.xml\"", src);
        Assert.DoesNotContain("File.WriteAllText(tmp, xml)", src);
        Assert.DoesNotContain("temp safai best-effort", src);
    }

    // ================= C9 — WlanProfileTempFile =================

    [Fact]
    public void C9_Create_WritesContent_WithUniqueNames()
    {
        string dir = NewTempDir();
        try
        {
            using var a = WlanProfileTempFile.Create("<xml>secret-password</xml>", dir);
            using var b = WlanProfileTempFile.Create("<xml>secret-password</xml>", dir);
            Assert.NotEqual(a.Path, b.Path);
            Assert.StartsWith(Path.Combine(dir, WlanProfileTempFile.FilePrefix), a.Path, StringComparison.Ordinal);
            Assert.EndsWith(WlanProfileTempFile.FileSuffix, a.Path, StringComparison.Ordinal);
            Assert.Equal("<xml>secret-password</xml>", File.ReadAllText(a.Path));
            Assert.Equal("<xml>secret-password</xml>", File.ReadAllText(b.Path));
            Assert.False(a.IsDeleted);
        }
        finally { Directory.Delete(dir, recursive: true); }
    }

    [Fact]
    public void C9_Create_RestrictsPermissions_OwnerOnly()
    {
        string dir = NewTempDir();
        try
        {
            using var tmp = WlanProfileTempFile.Create("secret", dir);
            if (!OperatingSystem.IsWindows())
            {
                // Linux /tmp shared hai — mode lazmi 0600 hai.
                var mode = File.GetUnixFileMode(tmp.Path);
                Assert.Equal(UnixFileMode.UserRead | UnixFileMode.UserWrite, mode);
            }
            else
            {
                Assert.Skip("Windows ACL branch — Linux sandbox mein verify nahi ho sakta (Phase H, Windows PC).");
            }
        }
        finally { Directory.Delete(dir, recursive: true); }
    }

    [Fact]
    public void C9_OverwriteWithZeros_ZeroesBytes_LengthPreserved()
    {
        string dir = NewTempDir();
        try
        {
            using var tmp = WlanProfileTempFile.Create("SECRET-PASSWORD-1234", dir);
            int len = File.ReadAllBytes(tmp.Path).Length;
            tmp.OverwriteWithZeros();
            byte[] after = File.ReadAllBytes(tmp.Path);
            Assert.Equal(len, after.Length);
            Assert.All(after, b => Assert.Equal(0, b));
        }
        finally { Directory.Delete(dir, recursive: true); }
    }

    [Fact]
    public void C9_SecureDelete_RemovesFile_AndIsIdempotent()
    {
        string dir = NewTempDir();
        try
        {
            var tmp = WlanProfileTempFile.Create("SECRET-PASSWORD-1234", dir);
            string path = tmp.Path;
            Assert.True(File.Exists(path));
            Assert.True(tmp.SecureDelete());
            Assert.True(tmp.IsDeleted);
            Assert.False(File.Exists(path));
            // Dobara — koi exception nahi, phir bhi true.
            Assert.True(tmp.SecureDelete());
            tmp.Dispose();
            tmp.Dispose();
        }
        finally { Directory.Delete(dir, recursive: true); }
    }

    [Fact]
    public void C9_Dispose_SecureDeletes_EvenOnExceptionPath()
    {
        string dir = NewTempDir();
        string path;
        try
        {
            using (var tmp = WlanProfileTempFile.Create("SECRET", dir))
            {
                path = tmp.Path;
                Assert.True(File.Exists(path));
            }   // using se bahar — Dispose ne secure-delete kiya hona chahiye
            Assert.False(File.Exists(path));
        }
        finally { Directory.Delete(dir, recursive: true); }
    }

    [Fact]
    public void C9_DeleteStaleFiles_RemovesOld_LeavesFresh_AndIgnoresOthers()
    {
        string dir = NewTempDir();
        try
        {
            // Purani crash leftover (grace se purani).
            string stale = Path.Combine(dir, WlanProfileTempFile.FilePrefix + Guid.NewGuid().ToString("N") + ".xml");
            File.WriteAllText(stale, "OLD-SECRET");
            File.SetLastWriteTimeUtc(stale, DateTime.UtcNow - TimeSpan.FromHours(1));
            // Zinda run ki nayi file — haath nahi lagna chahiye.
            string fresh = Path.Combine(dir, WlanProfileTempFile.FilePrefix + Guid.NewGuid().ToString("N") + ".xml");
            File.WriteAllText(fresh, "LIVE-SECRET");
            // Doosre naam — haath nahi lagna chahiye.
            string other = Path.Combine(dir, "other.xml");
            File.WriteAllText(other, "x");
            string wrongSuffix = Path.Combine(dir, WlanProfileTempFile.FilePrefix + Guid.NewGuid().ToString("N") + ".txt");
            File.WriteAllText(wrongSuffix, "x");

            int deleted = WlanProfileTempFile.DeleteStaleFiles(dir);

            Assert.Equal(1, deleted);
            Assert.False(File.Exists(stale));
            Assert.True(File.Exists(fresh));
            Assert.True(File.Exists(other));
            Assert.True(File.Exists(wrongSuffix));
        }
        finally { Directory.Delete(dir, recursive: true); }
    }

    [Fact]
    public void C9_Create_RestrictsBeforeWritingSecret_BugG2()
    {
        // BUG-G2 (W3 verification mein mila): Create() pehle secret likhta
        // tha, phir permissions restrict karta tha — beech ki chhoti window
        // mein %TEMP% par nazar rakhne wala local process password parh
        // sakta tha. Tarteeb lazmi hai: khaali file → restrict → secret.
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.Core", "Security", "WlanProfileTempFile.cs"));
        int restrictAt = src.IndexOf("RestrictPermissions(path);", StringComparison.Ordinal);
        int writeAt = src.IndexOf("Encoding.UTF8.GetBytes(xmlContent)", StringComparison.Ordinal);
        Assert.True(restrictAt > 0, "RestrictPermissions call nahi mili");
        Assert.True(writeAt > 0, "secret write nahi mila");
        Assert.True(restrictAt < writeAt, "BUG-G2: secret likhne se pehle permissions restrict honi chahiye");
    }

    [Fact]
    public void C9_CrashSimulation_StaleFile_SweptOnNextRun()
    {
        string dir = NewTempDir();
        try
        {
            // Crash simulate karo: file banao, Dispose KABHI mat bulao
            // (process mar gaya) — sirf reference chhoro.
            var tmp = WlanProfileTempFile.Create("CRASH-SECRET", dir);
            string path = tmp.Path;
            Assert.True(File.Exists(path));
            tmp = null;
            GC.Collect();
            // Waqt guzra (file purani) — agla connect sweep karta hai.
            File.SetLastWriteTimeUtc(path, DateTime.UtcNow - TimeSpan.FromHours(1));
            int deleted = WlanProfileTempFile.DeleteStaleFiles(dir);
            Assert.Equal(1, deleted);
            Assert.False(File.Exists(path));
        }
        finally { Directory.Delete(dir, recursive: true); }
    }
}
