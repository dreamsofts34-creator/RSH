using RSH.App.Services;
using RSH.App.ViewModels;
using RSH.Core.Pipeline;

namespace RSH.Tests;

/// <summary>
/// Data &amp; AI Round 2 (W7, 2026-11-14): Round 1 ke baad bache hue
/// data-integrity gaps — EncoderTracker span safety, PinMapStore
/// TryParseFile validation, AppSettingsStore 3-generation backup.
/// </summary>
public class DataAiRound2Tests
{
    private static string TempDir() =>
        Path.Combine(Path.GetTempPath(), "rsh-tests-r2", Guid.NewGuid().ToString("N"));

    private static void Cleanup(string dir)
    {
        try { if (Directory.Exists(dir)) Directory.Delete(dir, recursive: true); } catch { /* best-effort */ }
    }

    // ================= EncoderTracker: chhoti span par crash nahi =================

    [Fact]
    public void EncoderTracker_ShortCumulativeSpan_DoesNotThrow()
    {
        // W7-3: encoderCount=4 lekin span mein sirf 1 value — pehle
        // cumulative[..4] IndexOutOfRange phenkta tha (pipeline tick
        // ke andar). Ab clamp: maujooda encoder ka delta nikalta hai.
        var tracker = new EncoderTracker();
        var deltas = new short[EncoderTracker.MaxEncoders];
        tracker.Update([10], 4, deltas); // baseline
        tracker.Update([12], 4, deltas);
        Assert.Equal(2, deltas[0]);
        Assert.Equal(0, deltas[1]);
        Assert.Equal(0, deltas[2]);
        Assert.Equal(0, deltas[3]);
    }

    [Fact]
    public void EncoderTracker_EmptyCumulativeSpan_DoesNotThrowOrBaseline()
    {
        var tracker = new EncoderTracker();
        var deltas = new short[EncoderTracker.MaxEncoders];
        tracker.Update([], 4, deltas); // khaali span — crash nahi, baseline nahi
        Assert.Equal([0, 0, 0, 0], deltas);
        tracker.Update([5, 5, 5, 5], 4, deltas); // pehla poora packet baseline banta hai
        Assert.Equal([0, 0, 0, 0], deltas);
        tracker.Update([6, 5, 5, 5], 4, deltas);
        Assert.Equal(1, deltas[0]);
    }

    // ================= PinMapStore.TryParseFile: range/count bhi check =================

    [Fact]
    public void PinMapTryParseFile_RejectsOutOfRangePin()
    {
        string dir = TempDir();
        try
        {
            var store = new PinMapStore(dir);
            Directory.CreateDirectory(dir);
            File.WriteAllText(store.FilePath, """{"Steering":34,"Horn":999}""");
            // Pehle sirf parse hona kaafi tha — 999 "valid" lagta tha aur
            // Save ka backup guard use achi backup bana leta tha.
            Assert.False(PinMapStore.TryParseFile(store.FilePath, out _));
        }
        finally { Cleanup(dir); }
    }

    [Fact]
    public void PinMapTryParseFile_RejectsTooManyEntries()
    {
        string dir = TempDir();
        try
        {
            var store = new PinMapStore(dir);
            Directory.CreateDirectory(dir);
            var entries = string.Join(",", Enumerable.Range(0, 16).Select(i => $"\"Extra{i}\":null"));
            File.WriteAllText(store.FilePath, "{" + entries + "}");
            Assert.False(PinMapStore.TryParseFile(store.FilePath, out _));
        }
        finally { Cleanup(dir); }
    }

    [Fact]
    public void PinMapTryParseFile_AcceptsValidMap()
    {
        string dir = TempDir();
        try
        {
            var store = new PinMapStore(dir);
            Directory.CreateDirectory(dir);
            store.Save(PinMap.Defaults());
            Assert.True(PinMapStore.TryParseFile(store.FilePath, out var map));
            Assert.Equal(34, map!["Steering"]);
        }
        finally { Cleanup(dir); }
    }

    // ================= AppSettingsStore: 3-generation backup =================

    private static AppSettings SettingsWithId(int vjoyId) =>
        new() { VJoyDeviceId = vjoyId };

    [Fact]
    public void SettingsLoad_FallsBackToBak1WhenMainAndBakCorrupt()
    {
        string dir = TempDir();
        try
        {
            string file = Path.Combine(dir, "settings.json");
            var store = new AppSettingsStore(file);
            store.Save(SettingsWithId(2));
            store.Save(SettingsWithId(3));
            store.Save(SettingsWithId(4)); // main=4, .bak=3, .bak1=2
            File.WriteAllText(file, "{ corrupt");
            File.WriteAllText(file + ".bak", "{ corrupt");
            // Pehle sirf ek .bak tha — dono kharab hon to settings
            // hamesha ke liye defaults ho jati thin. Ab .bak1 se milti hai.
            Assert.Equal(2, store.Load().VJoyDeviceId);
        }
        finally { Cleanup(dir); }
    }

    [Fact]
    public void SettingsSave_RotatesThreeGenerations()
    {
        string dir = TempDir();
        try
        {
            string file = Path.Combine(dir, "settings.json");
            var store = new AppSettingsStore(file);
            store.Save(SettingsWithId(2));
            store.Save(SettingsWithId(3));
            store.Save(SettingsWithId(4));
            store.Save(SettingsWithId(5)); // main=5, .bak=4, .bak1=3, .bak2=2
            Assert.Equal(5, store.Load().VJoyDeviceId);
            Assert.Equal(4, new AppSettingsStore(file + ".bak").Load().VJoyDeviceId);
            Assert.Equal(3, new AppSettingsStore(file + ".bak1").Load().VJoyDeviceId);
            Assert.Equal(2, new AppSettingsStore(file + ".bak2").Load().VJoyDeviceId);
        }
        finally { Cleanup(dir); }
    }

    [Fact]
    public void SettingsSave_CorruptBak1DoesNotOverwriteGoodBak2()
    {
        string dir = TempDir();
        try
        {
            string file = Path.Combine(dir, "settings.json");
            var store = new AppSettingsStore(file);
            store.Save(SettingsWithId(2));
            store.Save(SettingsWithId(3));
            store.Save(SettingsWithId(4));
            store.Save(SettingsWithId(5)); // main=5, .bak=4, .bak1=3, .bak2=2
            File.WriteAllText(file + ".bak1", "{ corrupt");
            store.Save(SettingsWithId(6)); // .bak2 mein abhi bhi 2 hona chahiye
            Assert.Equal(2, new AppSettingsStore(file + ".bak2").Load().VJoyDeviceId);
            Assert.Equal(4, new AppSettingsStore(file + ".bak1").Load().VJoyDeviceId);
            Assert.Equal(5, new AppSettingsStore(file + ".bak").Load().VJoyDeviceId);
            Assert.Equal(6, store.Load().VJoyDeviceId);
        }
        finally { Cleanup(dir); }
    }
}
