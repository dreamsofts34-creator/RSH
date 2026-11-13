// Phase F — Workstream 3 (Games SW-14/A2 + pin-map board push + live reload).
// App-side tests: GameLauncher Stop (SEC-E-10), restore-on-exit (A2),
// Refresh attach (match naam + pehli jeet), AppServices live reload,
// PinMapBoardPush (SET_CFG/COMMIT/CONFIRM vs fake board peer).
//
// NOTE — ye project RSH.slnx mein NAHI hai (jaankari se): RSH.App
// Phase F W1 ke kaam ke baad build nahi hota (AppSettings.cs:35,47 —
// [JsonIgnore] par `using System.Text.Json.Serialization;` ghaib hai),
// is liye is project ko slnx mein daalna rig ka unit stage tor dega.
// W1 ka fix aate hi: RSH.slnx ke /tests/ folder mein
// `tests/RSH.App.Tests/RSH.App.Tests.csproj` add karo aur ye note hata do.
//
// Sandbox: WPF run nahi hota, lekin GameLauncher/AppServices/PinMapBoardPush
// mein koi WPF type nahi — ye tests Linux par chalte hain (stub processes
// /bin/sleep, /usr/bin/true). Real game launch / Steam scan / vJoy sirf
// Windows PC par verify honge (docs/GAME-COMPATIBILITY.md).
using System.Buffers.Binary;
using System.Diagnostics;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using RSH.App.Services;
using RSH.App.ViewModels;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.App.Tests;

/// <summary>Phase F W3 App tests — ek class (xUnit v3 class ke andar tests tartib se chalata hai).</summary>
public sealed class PhaseFWorkstream3AppTests : IDisposable
{
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();
    private static readonly byte[] OtherNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xB0 + i)).ToArray();

    private readonly List<Process> _ownedProcesses = [];
    private readonly List<AppServices> _ownedServices = [];
    private readonly string _xdgDir;

    public PhaseFWorkstream3AppTests()
    {
        _xdgDir = Path.Combine(Path.GetTempPath(), "rsh-w3-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_xdgDir);
        // AppServices %AppData% (Linux: XDG_CONFIG_HOME) mein likhta hai —
        // tests ke liye temp jagah, asal ~/.config ko haath nahi lagta.
        Environment.SetEnvironmentVariable("XDG_CONFIG_HOME", _xdgDir);
    }

    public void Dispose()
    {
        foreach (var proc in _ownedProcesses)
        {
            try { if (!proc.HasExited) proc.Kill(entireProcessTree: false); } catch (Exception) { }
            try { proc.Dispose(); } catch (Exception) { }
        }
        foreach (var services in _ownedServices)
        {
            try { services.Dispose(); } catch (Exception) { }
        }
        try { Directory.Delete(_xdgDir, recursive: true); } catch (Exception) { }
    }

    private AppServices CreateServices()
    {
        var services = new AppServices();
        _ownedServices.Add(services);
        return services;
    }

    private Process StartSleep(int seconds = 60)
    {
        var proc = Process.Start(new ProcessStartInfo("sleep", seconds.ToString()) { UseShellExecute = false });
        Assert.NotNull(proc);
        _ownedProcesses.Add(proc);
        return proc;
    }

    // ================= SEC-E-10: Stop sirf tracked process maarta hai =================

    [Fact]
    public async Task StopAsync_KillsOnlyTrackedProcess_SameNameDecoySurvives()
    {
        var services = CreateServices();
        var launcher = new GameLauncher(services);
        var entry = launcher.Entries[0]; // Assetto Corsa
        Process tracked = StartSleep();
        Process decoy = StartSleep(); // wahi naam ("sleep"), user ne khud khola hua doosra instance
        // IDs abhi capture karo — StopAsync tracked Process ko Dispose kar deta hai.
        int trackedId = tracked.Id;
        int decoyId = decoy.Id;
        launcher.AttachTo(entry, tracked, launchedByUs: true);
        Assert.Equal(GameLauncher.GameState.Running, entry.State);

        var (ok, _) = await launcher.StopAsync(entry);
        Assert.True(ok);
        // NOTE: StopAsync ke baad launcher tracked Process ko Dispose kar deta
        // hai (FinalizeExitAsync) — is liye HasExited parha nahi ja sakta.
        // Maut ka saboot: PID ab maujood nahi; decoy ka PID maujood + zinda hai.
        Assert.Throws<ArgumentException>(() => Process.GetProcessById(trackedId));
        using var decoyAlive = Process.GetProcessById(decoyId);
        Assert.False(decoyAlive.HasExited, "SEC-E-10: naam se milne wala doosra process NAHI marna chahiye tha.");
    }

    [Fact]
    public async Task StopAsync_AlreadyExitedProcess_IsNoopReady()
    {
        var services = CreateServices();
        var launcher = new GameLauncher(services);
        var entry = launcher.Entries[0];
        var proc = StartSleep();
        launcher.AttachTo(entry, proc, launchedByUs: true);
        proc.Kill(entireProcessTree: false);
        proc.WaitForExit();

        var (ok, _) = await launcher.StopAsync(entry);
        Assert.True(ok);
        Assert.Equal(GameLauncher.GameState.Ready, entry.State);
    }

    // ================= A2: restore-on-exit (SW-14) =================

    [Fact]
    public async Task RestoreOnExit_RestoresPreviousProfile_WhenGameExits()
    {
        int profileChanged = 0;
        var services = CreateServices();
        var launcher = new GameLauncher(services, onProfileChanged: () => profileChanged++);
        var entry = launcher.Entries[0]; // Assetto Corsa — ProcessNames: AssettoCorsa, acs

        // "Race" profile AssettoCorsa se bound hai; launch par "Default" -> "Race" switch hua tha.
        services.Profiles.Profiles.Add(WheelProfile.CreateDefault("Race") with { GameProcessName = "AssettoCorsa" });
        services.Profiles.ActiveProfileName = "Race";
        await services.SaveProfilesAndApplyAsync(TestContext.Current.CancellationToken);
        Assert.Equal("Race", services.ActiveProfile.Name);

        // Zinda stub process attach karo, fields pehle set karo, PHIR band karo —
        // pehle se band process attach karne par Exited event fields se pehle
        // finalize kar sakta tha (race).
        var proc = StartSleep();
        launcher.AttachTo(entry, proc, launchedByUs: true);
        entry.PreviousProfileName = "Default";
        entry.BoundProfileName = "Race";
        proc.Kill(entireProcessTree: false);
        proc.WaitForExit();

        launcher.Poll();
        bool restored = false;
        for (int i = 0; i < 50 && !restored; i++)
        {
            await Task.Delay(100, TestContext.Current.CancellationToken);
            restored = services.Profiles.ActiveProfileName == "Default"
                && entry.State == GameLauncher.GameState.Ready;
        }
        Assert.True(restored, "Game band hone par purani profile ('Default') wapas aani chahiye thi.");
        Assert.Equal(1, profileChanged);
    }

    [Fact]
    public async Task RestoreOnExit_UserChangedProfileMidGame_KeepsUserChoice()
    {
        var services = CreateServices();
        var launcher = new GameLauncher(services);
        var entry = launcher.Entries[0];
        services.Profiles.Profiles.Add(WheelProfile.CreateDefault("Race") with { GameProcessName = "AssettoCorsa" });
        services.Profiles.Profiles.Add(WheelProfile.CreateDefault("Drift") with { GameProcessName = "" });
        services.Profiles.ActiveProfileName = "Drift"; // user ne game chalte hue khud badal di
        await services.SaveProfilesAndApplyAsync(TestContext.Current.CancellationToken);

        var proc = StartSleep();
        launcher.AttachTo(entry, proc, launchedByUs: true);
        entry.PreviousProfileName = "Default";
        entry.BoundProfileName = "Race";
        proc.Kill(entireProcessTree: false);
        proc.WaitForExit();
        launcher.Poll();
        for (int i = 0; i < 50 && entry.State == GameLauncher.GameState.Running; i++)
            await Task.Delay(100, TestContext.Current.CancellationToken);
        Assert.Equal(GameLauncher.GameState.Ready, entry.State);
        Assert.Equal("Drift", services.Profiles.ActiveProfileName); // user ki pasand rehti hai
    }

    // ================= Refresh: naam se match + pehli jeet =================

    [Fact]
    public async Task RefreshAsync_AttachesRunningGameByProcessName_FirstWin()
    {
        if (!OperatingSystem.IsLinux())
            Assert.Skip("Stub-process trick Linux-only hai — Windows PC par asli game se verify hoga.");
        var services = CreateServices();
        var launcher = new GameLauncher(services);
        // /bin/sleep ki copy jiska naam game jaisa hai (comm = file naam).
        string dir = Path.Combine(_xdgDir, "fake-game");
        Directory.CreateDirectory(dir);
        string fakeExe = Path.Combine(dir, "AssettoCorsa");
        File.Copy("/bin/sleep", fakeExe);
#pragma warning disable CA1416 // Linux-only test (upar Assert.Skip guard) — Windows par kabhi nahi chalta
        File.SetUnixFileMode(fakeExe, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
#pragma warning restore CA1416
        var first = Process.Start(new ProcessStartInfo(fakeExe, "60") { UseShellExecute = false });
        var second = Process.Start(new ProcessStartInfo(fakeExe, "60") { UseShellExecute = false });
        Assert.NotNull(first);
        Assert.NotNull(second);
        _ownedProcesses.Add(first);
        _ownedProcesses.Add(second);

        await launcher.RefreshAsync(TestContext.Current.CancellationToken);
        var entry = launcher.Entries[0];
        Assert.Equal(GameLauncher.GameState.Running, entry.State);
        Assert.NotNull(entry.TrackedProcess);
        // Pehli jeet: tracked in dono mein se ek hai, doosra attach nahi hua.
        Assert.True(entry.TrackedProcess.Id == first.Id || entry.TrackedProcess.Id == second.Id);
    }

    // ================= Live reload (AppServices) =================

    [Fact]
    public async Task ReloadProfilesAsync_LoadsFromDisk_NoRestart_SameEngine()
    {
        var services = CreateServices();
        var engineBefore = services.Engine;
        int reloaded = 0;
        services.ProfilesReloaded += () => reloaded++;

        // Bahar se restore: disk par nayi profiles.json (doosri active profile).
        var restored = new ProfileFile
        {
            SchemaVersion = ProfileFile.CurrentSchemaVersion,
            Profiles = [WheelProfile.CreateDefault("Default"), WheelProfile.CreateDefault("Restored")],
            ActiveProfileName = "Restored",
        };
        string activeBefore = services.ActiveProfile.Name;
        await services.ProfileStore.SaveAsync(restored, TestContext.Current.CancellationToken);
        // Windows par AppServices %AppData% istemal karta hai (XDG_CONFIG_HOME
        // sirf Linux par) — pehle wale tests ki saved profiles (masalan
        // "Drift") is test ki fresh AppServices mein load hoti hain, is liye
        // hardcoded "Default" ghalat hai. Jo bhi active tha, SaveAsync ke
        // baad wahi rehna chahiye (in-memory tabdeel nahi hoti).
        Assert.Equal(activeBefore, services.ActiveProfile.Name); // abhi bhi purani in-memory

        var (ok, active, error) = await services.ReloadProfilesAsync(TestContext.Current.CancellationToken);
        Assert.True(ok, error);
        Assert.Equal("Restored", active);
        Assert.Equal("Restored", services.ActiveProfile.Name);
        Assert.Equal("Restored", services.Engine.Profile.Name); // engine ko nayi profile mili
        Assert.Same(engineBefore, services.Engine); // restart NAHI — wahi engine instance
        Assert.Equal(1, reloaded);
    }

    // ================= Pin-map board push =================

    [Fact]
    public void PinMapBoardPush_BuildBlob_DefaultMap_MatchesExpectedRecords()
    {
        var pins = PinMap.Defaults();
        byte[] blob = PinMapBoardPush.BuildBlob(pins, cfgVersion: 1);
        Assert.True(ConfigBlob.TryValidateStructure(blob, out string? error), error);
        // 11 records: 4 axes + battery + 3 buttons + 2 encoders + LED strip
        // (Clutch not assigned, Status LED ka blob type nahi hai).
        Assert.Equal(11, blob[5]);
        Assert.Equal(8 + 11 * 16 + 4, blob.Length);
        var records = ReadRecords(blob);
        Assert.Contains(records, r => r.Type == 1 && r.Slot == 0 && r.PinA == 34); // Steering
        Assert.Contains(records, r => r.Type == 1 && r.Slot == 1 && r.PinA == 35); // Accelerator
        Assert.Contains(records, r => r.Type == 1 && r.Slot == 4 && r.PinA == 32); // Handbrake — fixed slot 4 (Clutch 3 khaali hai)
        Assert.Contains(records, r => r.Type == 7 && r.Slot == 0 && r.PinA == 39); // Battery
        Assert.Contains(records, r => r.Type == 5 && r.Slot == 0 && r.PinA == 23 && r.PinB == 19); // Encoder 1
        Assert.Contains(records, r => r.Type == 6 && r.Slot == 0 && r.PinA == 21); // LED strip
        Assert.DoesNotContain(records, r => r.PinA == 2); // Status LED (GPIO 2) blob mein nahi
        Assert.DoesNotContain(records, r => r.Type == 1 && r.Slot == 3); // Clutch not assigned → koi record nahi
    }

    [Fact]
    public void PinMapBoardPush_BuildSetCfgChunks_LayoutMatchesProtocol35()
    {
        byte[] blob = PinMapBoardPush.BuildBlob(PinMap.Defaults(), cfgVersion: 7);
        var chunks = PinMapBoardPush.BuildSetCfgChunks(blob);
        Assert.Single(chunks);
        byte[] chunk = chunks[0];
        Assert.Equal(0, chunk[0] | (chunk[1] << 8)); // offset
        Assert.Equal(blob.Length, chunk[2] | (chunk[3] << 8)); // total
        Assert.Equal(blob.Length, chunk[4] | (chunk[5] << 8)); // chunkLen
        ushort crc = (ushort)(chunk[6] | (chunk[7] << 8));
        Assert.Equal(Crc16.Compute(blob), crc); // chunkCrc = CRC16-CCITT data par
        Assert.Equal(blob, chunk.AsSpan(8).ToArray());
        // Bara blob → kai chunks (2048 B blob → 2 chunks).
        var big = new byte[2048];
        Array.Fill<byte>(big, 0x5A);
        var bigChunks = PinMapBoardPush.BuildSetCfgChunks(big);
        Assert.Equal(2, bigChunks.Count);
        Assert.Equal(1024, bigChunks[0][4] | (bigChunks[0][5] << 8));
        Assert.Equal(1024, bigChunks[1][0] | (bigChunks[1][1] << 8)); // doosra chunk offset 1024
    }

    [Fact]
    public async Task PinMapBoardPush_PushAsync_HappyPath_CommitsAndConfirms()
    {
        var peer = new FakeBoardPeer();
        using var control = new ControlClient("127.0.0.1", TestKey, TestNonce, peer.Transport);
        var (ok, error, chunks) = await PinMapBoardPush.PushAsync(
            PinMap.Defaults(),
            (cmd, payload, ct) => control.SendCommandAsync(cmd, payload, ct),
            cfgVersion: 1,
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(ok, error);
        Assert.Equal(1, chunks);
        // SEC-E-14: HELLO par app background mein khud GET_CAPS bhejta hai
        // (Task.Run — async, is liye GetCaps push sequence ke pehle, beech
        // mein ya baad mein bhi aa sakta hai). Push ka sequence SetCfg ->
        // Commit -> Confirm hona chahiye — subsequence ke tor par assert
        // karo, exact list nahi.
        var seq = peer.ReceivedCommands;
        int iSet = seq.IndexOf(ControlCommand.SetCfg);
        int iCommit = seq.IndexOf(ControlCommand.Commit);
        int iConfirm = seq.IndexOf(ControlCommand.Confirm);
        Assert.True(iSet >= 0 && iCommit > iSet && iConfirm > iCommit,
            $"Push sequence ghalat: [{string.Join(", ", seq)}]");
        Assert.NotNull(peer.CommittedBlob);
        Assert.True(ConfigBlob.TryValidateStructure(peer.CommittedBlob!, out string? blobError), blobError);
        Assert.True(peer.Confirmed);
        // Board ne jo blob qabool kiya, us mein steering pin 34 hona chahiye.
        var records = ReadRecords(peer.CommittedBlob!);
        Assert.Contains(records, r => r.Type == 1 && r.Slot == 0 && r.PinA == 34);
    }

    [Fact]
    public async Task PinMapBoardPush_PushAsync_InvalidMap_SendsNothing()
    {
        var peer = new FakeBoardPeer();
        using var control = new ControlClient("127.0.0.1", TestKey, TestNonce, peer.Transport);
        var pins = PinMap.Defaults();
        pins["Brake"] = 34; // Steering se takrao — ValidateAll error
        var (ok, error, chunks) = await PinMapBoardPush.PushAsync(
            pins,
            (cmd, payload, ct) => control.SendCommandAsync(cmd, payload, ct),
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.False(ok);
        Assert.NotNull(error);
        Assert.Equal(0, chunks);
        Assert.Empty(peer.ReceivedCommands); // ghalat map board tak kabhi nahi jata
    }

    [Fact]
    public async Task PinMapBoardPush_PushAsync_CommitFails_RevertsStaged()
    {
        var peer = new FakeBoardPeer { FailCommit = true };
        using var control = new ControlClient("127.0.0.1", TestKey, TestNonce, peer.Transport);
        var (ok, error, _) = await PinMapBoardPush.PushAsync(
            PinMap.Defaults(),
            (cmd, payload, ct) => control.SendCommandAsync(cmd, payload, ct),
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.False(ok);
        Assert.Contains("COMMIT", error!);
        Assert.Equal(new[] { ControlCommand.SetCfg, ControlCommand.Commit, ControlCommand.Revert },
            peer.ReceivedCommands);
        Assert.Null(peer.CommittedBlob); // staged discard hua
    }

    // ================= SEC-E-11: VDF paths validate ho kar aage jayen =================

    [Fact]
    public void FindSteamLibraries_ValidatesVdfPaths_TraversalDropped()
    {
        string dir = Path.Combine(_xdgDir, "steam");
        Directory.CreateDirectory(Path.Combine(dir, "steamapps"));
        File.WriteAllText(Path.Combine(dir, "steamapps", "libraryfolders.vdf"),
            "\"libraryfolders\"\n{\n"
            + "\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Games\\\\SteamLibrary\"\n\t}\n"
            + "\t\"1\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Evil\\\\..\\\\Windows\"\n\t}\n"
            + "\t\"2\"\n\t{\n\t\t\"path\"\t\t\"relative\\\\path\"\n\t}\n"
            + "\t\"3\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Windows\\\\System32\"\n\t}\n"
            + "\t\"4\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Games\\\\SteamLibrary\"\n\t}\n"
            + "}\n");
        var libraries = GameLauncher.FindSteamLibraries(dir);
        Assert.Equal(2, libraries.Count);
        Assert.Equal(dir, libraries[0]); // steam root khud (VDF se nahi — registry/default rasta)
        Assert.Equal(@"C:\Games\SteamLibrary", libraries[1]); // sirf valid path guzra
    }

    // ================= SEC-E-14: (re)connect par khud GET_CAPS =================

    [Fact]
    public async Task RefreshBoardCapsAsync_WithoutSession_ReturnsErrorNoThrow()
    {
        var services = CreateServices(); // StartRuntime nahi — koi HELLO nahi, koi session nahi
        var (caps, error) = await services.RefreshBoardCapsAsync(TestContext.Current.CancellationToken);
        Assert.Null(caps);
        Assert.NotNull(error);
    }

    [Fact]
    public async Task AutoCapsFetch_OnNewSession_FetchesOnce_AndRefetchesOnReboot()
    {
        // In-memory gate test: HELLO socket ke bajaye ProcessDatagram se
        // aata hai (sandbox UDP sendto EPERM — dekho wheel.py), GET_CAPS
        // fake in-memory transport se. Asal socket rasta Windows-only
        // test (neeche) + rig par verify hota hai.
        var services = CreateServices();
        byte[] currentNonce = TestNonce;
        int getCapsCount = 0;
        services.ControlTransportOverride = new FakeControlTransport(frame =>
        {
            if (ControlFrameCodec.TryParse(frame, TestKey, currentNonce, out var req) != ControlParseResult.Ok)
                return null; // bad tag — firmware khamoshi se drop karta hai
            if ((ControlCommand)req.Cmd != ControlCommand.GetCaps)
                return ReplyControl(req, currentNonce, ControlStatus.Unsupported, []);
            Interlocked.Increment(ref getCapsCount);
            return ReplyControl(req, currentNonce, ControlStatus.Ok, CapsReplyBody());
        });
        int capsUpdated = 0;
        services.BoardCapsUpdated += () => capsUpdated++;

        // Pehla HELLO (naya nonce) → ek GET_CAPS, dashboard ko caps milein.
        services.UdpSource.ProcessDatagram(BuildHelloReply(currentNonce, TestKey));
        bool gotCaps = await WaitUntilAsync(() => services.LastBoardCaps is not null, timeoutMs: 5000);
        Assert.True(gotCaps, "Connect par GET_CAPS khud hona chahiye tha (SEC-E-14).");
        Assert.Equal(new Version(4, 0, 0), services.LastBoardCaps!.FirmwareVersion);
        Assert.True(capsUpdated >= 1);
        Assert.Equal(1, getCapsCount);

        // Wahi nonce dobara aaye (1 Hz HELO) → dobara GET_CAPS nahi (spam nahi).
        services.UdpSource.ProcessDatagram(BuildHelloReply(currentNonce, TestKey));
        await Task.Delay(1000, TestContext.Current.CancellationToken);
        Assert.Equal(1, getCapsCount);

        // Board reboot: naya nonce → dobara GET_CAPS (dashboard dobara bhar jaye).
        currentNonce = OtherNonce;
        services.UdpSource.ProcessDatagram(BuildHelloReply(currentNonce, TestKey));
        bool refetched = await WaitUntilAsync(() => getCapsCount == 2, timeoutMs: 5000);
        Assert.True(refetched, "Board reboot (naya nonce) par GET_CAPS dobara hona chahiye tha.");
        Assert.True(capsUpdated >= 2);
    }

    [Fact]
    public async Task AutoCapsFetch_LoopbackUdp_WindowsOnly()
    {
        // Asal socket rasta: sandbox (Linux) mein unconnected UDP sendto
        // EPERM se block hota hai (wheel.py) — sirf Windows-PC gate par.
        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
            Assert.Skip("Loopback UDP e2e sirf Windows-PC gate par (sandbox UDP sendto blocked).");
        var services = CreateServices();
        services.Settings.EspIp = "127.0.0.1"; // loopback board (fake peer)
        int capsUpdated = 0;
        services.BoardCapsUpdated += () => capsUpdated++;
        using var hello = new HelloResponder(TestNonce);
        using var capsPeer = new FakeCapsPeer(() => hello.Nonce);
        hello.Start();
        capsPeer.Start();
        services.StartRuntime();

        // Pehla HELLO (naya nonce) → ek GET_CAPS, dashboard ko caps milein.
        bool gotCaps = false;
        for (int i = 0; i < 150 && !gotCaps; i++)
        {
            await Task.Delay(100, TestContext.Current.CancellationToken);
            gotCaps = services.LastBoardCaps is not null;
        }
        Assert.True(gotCaps, "Connect par GET_CAPS khud hona chahiye tha (SEC-E-14).");
        Assert.Equal(new Version(4, 0, 0), services.LastBoardCaps!.FirmwareVersion);
        Assert.True(capsUpdated >= 1);
        Assert.Equal(1, capsPeer.GetCapsCount);

        // Wahi nonce dobara aaye (1 Hz HELO) → dobara GET_CAPS nahi (spam nahi).
        await Task.Delay(3000, TestContext.Current.CancellationToken);
        Assert.Equal(1, capsPeer.GetCapsCount);

        // Board reboot: naya nonce → dobara GET_CAPS (dashboard dobara bhar jaye).
        hello.Nonce = OtherNonce;
        bool refetched = false;
        for (int i = 0; i < 150 && !refetched; i++)
        {
            await Task.Delay(100, TestContext.Current.CancellationToken);
            refetched = capsPeer.GetCapsCount == 2;
        }
        Assert.True(refetched, "Board reboot (naya nonce) par GET_CAPS dobara hona chahiye tha.");
        Assert.True(capsUpdated >= 2);
    }

    // ================= W3 follow-up wiring (Phase F) =================

    [Fact]
    public async Task BoardSessionSendControlAsync_WithoutSession_ReturnsErrorNoFrames()
    {
        using var session = new BoardSession(); // koi HELLO nahi — key/nonce nahi
        Assert.False(session.CanSendControl);
        var result = await session.SendControlAsync("127.0.0.1", ControlCommand.SetCfg,
            ReadOnlyMemory<byte>.Empty, cancellationToken: TestContext.Current.CancellationToken);
        Assert.Null(result.ReplyPayload);
        Assert.NotNull(result.Error);
        Assert.Contains("HELLO", result.Error!, StringComparison.Ordinal);
    }

    [Fact]
    public async Task PinMapBoardPush_ViaBoardSessionSendControlAsync_HappyPath()
    {
        // Poori production wiring: PinMapBoardPush → BoardSession.SendControlAsync
        // (key/nonce/counter session se) → fake board peer (SET_CFG/COMMIT/CONFIRM).
        var services = CreateServices();
        var peer = new FakeBoardPeer();
        services.ControlTransportOverride = peer.Transport;
        services.UdpSource.ProcessDatagram(BuildHelloReply(TestNonce, TestKey));
        bool sessionReady = await WaitUntilAsync(() => services.BoardSession.CanSendControl, timeoutMs: 5000);
        Assert.True(sessionReady, "HELLO ke baad BoardSession ko key/nonce milna chahiye tha.");

        var (ok, error, chunks) = await PinMapBoardPush.PushAsync(
            PinMap.Defaults(),
            (cmd, payload, ct) => services.BoardSession.SendControlAsync(
                services.Settings.EspIp, cmd, payload, peer.Transport, ct),
            cfgVersion: 1,
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(ok, error);
        Assert.Equal(1, chunks);
        // SEC-E-14: HELLO par app background mein khud GET_CAPS bhejta hai
        // (Task.Run — async, is liye GetCaps push sequence ke pehle, beech
        // mein ya baad mein bhi aa sakta hai). Push ka sequence SetCfg ->
        // Commit -> Confirm hona chahiye — subsequence ke tor par assert
        // karo, exact list nahi.
        var seq = peer.ReceivedCommands;
        int iSet = seq.IndexOf(ControlCommand.SetCfg);
        int iCommit = seq.IndexOf(ControlCommand.Commit);
        int iConfirm = seq.IndexOf(ControlCommand.Confirm);
        Assert.True(iSet >= 0 && iCommit > iSet && iConfirm > iCommit,
            $"Push sequence ghalat: [{string.Join(", ", seq)}]");
        Assert.NotNull(peer.CommittedBlob);
        var records = ReadRecords(peer.CommittedBlob!);
        Assert.Contains(records, r => r.Type == 1 && r.Slot == 0 && r.PinA == 34);
    }

    [Fact]
    public async Task SetupViewModelBridge_BoardCapsUpdated_MirrorsLastBoardCaps()
    {
        // SEC-E-14 bridge (W2): BoardCapsUpdated event → SetupViewModel.LastBoardCaps.
        var services = CreateServices();
        byte[] currentNonce = TestNonce;
        services.ControlTransportOverride = new FakeControlTransport(frame =>
        {
            if (ControlFrameCodec.TryParse(frame, TestKey, currentNonce, out var req) != ControlParseResult.Ok)
                return null;
            if ((ControlCommand)req.Cmd != ControlCommand.GetCaps)
                return ReplyControl(req, currentNonce, ControlStatus.Unsupported, []);
            return ReplyControl(req, currentNonce, ControlStatus.Ok, CapsReplyBody());
        });
        // Instance ki zaroorat nahi — bridge subscription AppServices ke event
        // par rehti hai (lambda sirf `services` capture karta hai, `this` nahi).
        _ = new SetupViewModel(services);
        services.UdpSource.ProcessDatagram(BuildHelloReply(currentNonce, TestKey));
        bool gotCaps = await WaitUntilAsync(() => services.LastBoardCaps is not null, timeoutMs: 5000);
        Assert.True(gotCaps, "Connect par GET_CAPS khud hona chahiye tha (SEC-E-14).");
        Assert.Same(services.LastBoardCaps, SetupViewModel.LastBoardCaps);
    }

    // ================= Helpers =================

    /// <summary>Condition poori hone tak poll karo (timeout par false).</summary>
    private static async Task<bool> WaitUntilAsync(Func<bool> condition, int timeoutMs, int pollMs = 100)
    {
        var deadline = DateTimeOffset.UtcNow + TimeSpan.FromMilliseconds(timeoutMs);
        while (DateTimeOffset.UtcNow < deadline)
        {
            if (condition()) return true;
            await Task.Delay(pollMs, TestContext.Current.CancellationToken);
        }
        return condition();
    }

    /// <summary>Control reply frame banao (cmd|0x80, status + body payload, HMAC-signed).</summary>
    private static byte[] ReplyControl(ControlFrame req, byte[] nonce, ControlStatus status, byte[] body)
    {
        var payload = new byte[1 + body.Length];
        payload[0] = (byte)status;
        body.CopyTo(payload, 1);
        return ControlFrameCodec.Build((byte)(req.Cmd | 0x80), req.ReqId, req.Counter, payload, TestKey, nonce);
    }

    /// <summary>HELLO reply bytes banao (92 B, §2.2) — key nonzero = unpaired.</summary>
    private static byte[] BuildHelloReply(byte[] nonce, byte[] key)
    {
        var bytes = new byte[92];
        bytes[0] = (byte)'R'; bytes[1] = (byte)'S'; bytes[2] = (byte)'H'; bytes[3] = (byte)'R';
        bytes[4] = 2; bytes[5] = 1; bytes[6] = 0x01; bytes[7] = 0;
        new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF }.CopyTo(bytes, 8);
        bytes[14] = 4; bytes[15] = 0; bytes[16] = 0; bytes[17] = 0; // fw 4.0.0
        nonce.CopyTo(bytes, 18);
        key.CopyTo(bytes, 34);
        ushort crc = Crc16.Compute(bytes.AsSpan(0, 90));
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(90, 2), crc);
        return bytes;
    }

    /// <summary>GET_CAPS reply body (status ke baad ke 17 B: MAC + fw + proto + limits + channel + flags + pinCount 0).</summary>
    private static byte[] CapsReplyBody() => [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        4, 0, 0, 2, 1, 8, 64, 4, 6, 0, 0];

    /// <summary>Fake board: UDP 4210 par HELO ka HELLO reply deta hai (nonce badla ja sakta hai = reboot).</summary>
    private sealed class HelloResponder : IDisposable
    {
        private readonly UdpClient _udp = new(4210);
        private readonly CancellationTokenSource _cts = new();
        private Task? _loop;
        private bool _disposed;

        public HelloResponder(byte[] nonce) => Nonce = nonce;

        /// <summary>Is waqt ka nonce — test isay badal kar board reboot dikhata hai.</summary>
        public byte[] Nonce { get; set; }

        public void Start() => _loop = Task.Run(LoopAsync);

        private async Task LoopAsync()
        {
            while (!_cts.IsCancellationRequested)
            {
                UdpReceiveResult received;
                try { received = await _udp.ReceiveAsync(_cts.Token).ConfigureAwait(false); }
                catch (Exception) when (_cts.IsCancellationRequested || _disposed) { break; }
                catch (Exception) { continue; }
                if (received.Buffer.Length < 5 ||
                    received.Buffer[0] != (byte)'H' || received.Buffer[1] != (byte)'E' ||
                    received.Buffer[2] != (byte)'L' || received.Buffer[3] != (byte)'O')
                    continue;
                byte[] reply = BuildHelloReply(Nonce, TestKey);
                try { await _udp.SendAsync(reply, received.RemoteEndPoint, _cts.Token).ConfigureAwait(false); }
                catch (Exception) { /* band ho raha hai */ }
            }
        }

        public void Dispose()
        {
            if (_disposed) return;
            _disposed = true;
            _cts.Cancel();
            _udp.Dispose();
            try { _loop?.Wait(TimeSpan.FromSeconds(2)); } catch (Exception) { }
            _cts.Dispose();
        }
    }

    /// <summary>Fake board: UDP 4212 par GET_CAPS ka jawab deta hai (HMAC verify ke saath).</summary>
    private sealed class FakeCapsPeer : IDisposable
    {
        private readonly Func<byte[]> _nonceProvider;
        private readonly UdpClient _udp = new(4212);
        private readonly CancellationTokenSource _cts = new();
        private Task? _loop;
        private int _getCapsCount;
        private bool _disposed;

        public FakeCapsPeer(Func<byte[]> nonceProvider) => _nonceProvider = nonceProvider;

        public int GetCapsCount => Volatile.Read(ref _getCapsCount);

        public void Start() => _loop = Task.Run(LoopAsync);

        private async Task LoopAsync()
        {
            while (!_cts.IsCancellationRequested)
            {
                UdpReceiveResult received;
                try { received = await _udp.ReceiveAsync(_cts.Token).ConfigureAwait(false); }
                catch (Exception) when (_cts.IsCancellationRequested || _disposed) { break; }
                catch (Exception) { continue; }
                byte[]? reply = Handle(received.Buffer);
                if (reply is null) continue;
                try { await _udp.SendAsync(reply, received.RemoteEndPoint, _cts.Token).ConfigureAwait(false); }
                catch (Exception) { /* band ho raha hai */ }
            }
        }

        private byte[]? Handle(byte[] frame)
        {
            byte[] nonce = _nonceProvider();
            if (ControlFrameCodec.TryParse(frame, TestKey, nonce, out var req) != ControlParseResult.Ok)
                return null; // bad tag / bad frame — firmware khamoshi se drop karta hai
            if ((ControlCommand)req.Cmd != ControlCommand.GetCaps)
                return Reply(req, nonce, ControlStatus.Unsupported, []);
            Interlocked.Increment(ref _getCapsCount);
            return Reply(req, nonce, ControlStatus.Ok, CapsReplyBody());
        }

        private byte[] Reply(ControlFrame req, byte[] nonce, ControlStatus status, byte[] body) =>
            ReplyControl(req, nonce, status, body);

        public void Dispose()
        {
            if (_disposed) return;
            _disposed = true;
            _cts.Cancel();
            _udp.Dispose();
            try { _loop?.Wait(TimeSpan.FromSeconds(2)); } catch (Exception) { }
            _cts.Dispose();
        }
    }

    private static List<(byte Type, byte Slot, byte PinA, byte PinB)> ReadRecords(byte[] blob)
    {
        var list = new List<(byte, byte, byte, byte)>();
        for (int i = 0; i < blob[5]; i++)
        {
            int off = 8 + i * 16;
            list.Add((blob[off], blob[off + 1], blob[off + 3], blob[off + 4]));
        }
        return list;
    }

    /// <summary>Fake board peer: firmware config transaction (SET_CFG/COMMIT/CONFIRM/REVERT) ki naqal.</summary>
    private sealed class FakeBoardPeer
    {
        public bool FailCommit { get; set; }
        public List<ControlCommand> ReceivedCommands { get; } = [];
        public byte[]? CommittedBlob { get; private set; }
        public bool Confirmed { get; private set; }

        private byte[]? _staged;
        private int _stagedBytes;
        private int _total;

        public FakeControlTransport Transport { get; }

        public FakeBoardPeer() => Transport = new FakeControlTransport(Handle);

        private byte[]? Handle(byte[] frame)
        {
            if (ControlFrameCodec.TryParse(frame, TestKey, TestNonce, out var req) != ControlParseResult.Ok)
                return null; // bad tag / bad frame — firmware khamoshi se drop karta hai
            var cmd = (ControlCommand)req.Cmd;
            ReceivedCommands.Add(cmd);
            return cmd switch
            {
                ControlCommand.SetCfg => HandleSetCfg(req),
                ControlCommand.Commit => HandleCommit(req),
                ControlCommand.Confirm => HandleConfirm(req),
                ControlCommand.Revert => HandleRevert(req),
                _ => Reply(req, ControlStatus.Unsupported, []),
            };
        }

        private byte[] HandleSetCfg(ControlFrame req)
        {
            if (req.Payload.Length < 8) return Reply(req, ControlStatus.BadCrc, []);
            int offset = req.Payload[0] | (req.Payload[1] << 8);
            int total = req.Payload[2] | (req.Payload[3] << 8);
            int chunkLen = req.Payload[4] | (req.Payload[5] << 8);
            ushort chunkCrc = (ushort)(req.Payload[6] | (req.Payload[7] << 8));
            if (req.Payload.Length != 8 + chunkLen || chunkLen == 0 || chunkLen > 1024 || total > 2048 || offset + chunkLen > total)
                return Reply(req, ControlStatus.BadCrc, []);
            var data = req.Payload.AsSpan(8, chunkLen);
            if (Crc16.Compute(data) != chunkCrc)
                return Reply(req, ControlStatus.BadCrc, []);
            if (offset == 0 || _staged is null || _total != total)
            {
                _staged = new byte[total];
                _stagedBytes = 0;
                _total = total;
            }
            data.CopyTo(_staged.AsSpan(offset));
            _stagedBytes = Math.Max(_stagedBytes, offset + chunkLen);
            byte[] body = new byte[4];
            BinaryPrimitives.WriteUInt16LittleEndian(body.AsSpan(0, 2), (ushort)_stagedBytes);
            BinaryPrimitives.WriteUInt16LittleEndian(body.AsSpan(2, 2), (ushort)_total);
            return Reply(req, ControlStatus.Ok, body);
        }

        private byte[] HandleCommit(ControlFrame req)
        {
            if (FailCommit) return Reply(req, ControlStatus.PinConflict, [1]);
            if (_staged is null || _stagedBytes != _total)
                return Reply(req, ControlStatus.Unsupported, []);
            if (!ConfigBlob.TryValidateStructure(_staged, out _))
                return Reply(req, ControlStatus.BadCrc, [4]);
            CommittedBlob = _staged.ToArray();
            Confirmed = false;
            return Reply(req, ControlStatus.Ok, []);
        }

        private byte[] HandleConfirm(ControlFrame req)
        {
            if (CommittedBlob is null) return Reply(req, ControlStatus.Unsupported, []);
            Confirmed = true;
            return Reply(req, ControlStatus.Ok, []);
        }

        private byte[] HandleRevert(ControlFrame req)
        {
            _staged = null;
            _stagedBytes = 0;
            _total = 0;
            CommittedBlob = null;
            Confirmed = false;
            return Reply(req, ControlStatus.Ok, []);
        }

        private byte[] Reply(ControlFrame req, ControlStatus status, byte[] body)
        {
            var payload = new byte[1 + body.Length];
            payload[0] = (byte)status;
            body.CopyTo(payload, 1);
            return ControlFrameCodec.Build((byte)(req.Cmd | 0x80), req.ReqId, req.Counter, payload, TestKey, TestNonce);
        }
    }

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
}
