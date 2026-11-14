using System.Buffers.Binary;
using System.Net;
using System.Text;
using RSH.App.Services;
using RSH.App.ViewModels;
using RSH.Core.Firmware;
using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Security;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>
/// Round 2 W2 (Backend / Logic) — naye findings ke regression tests:
/// B2-01 pairing-window gate, B2-02 HELLO Wi-Fi password, B2-03 KEY_RESET
/// sender, B2-04 control-send serialization, B2-05 engine plan-aware
/// connect, B2-06 UDP sender filter, B2-08 Horn module gating.
/// Fixtures deterministic hain (koi asal secret nahi).
/// </summary>
public class Round2BackendTests
{
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] OtherKey = Enumerable.Range(32, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();
    private static readonly byte[] OtherNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xB0 + i)).ToArray();
    private static readonly byte[] TestMac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    private const string TestBoardId = "aabbccddeeff";
    private const string TestPassword = "Ab1!Cd2@Ef3#Gh4$Ij5%Kl6^"; // exactly 24 printable ASCII

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

    private sealed class DelayedFakeControlTransport : IControlTransport
    {
        private readonly Func<byte[], byte[]?> _responder;
        private readonly object _lock = new();
        public List<byte[]> CapturedFrames { get; } = new();
        public DelayedFakeControlTransport(Func<byte[], byte[]?> responder) => _responder = responder;
        public async Task<byte[]?> ExchangeAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken)
        {
            lock (_lock) CapturedFrames.Add(frame.ToArray());
            await Task.Delay(15, cancellationToken).ConfigureAwait(false);
            return _responder(frame);
        }
        public void Dispose() { }
    }

    private static byte[] ReplyFor(byte[] request, byte[] payload, byte[] key, byte[] nonce)
    {
        ushort reqId = BinaryPrimitives.ReadUInt16LittleEndian(request.AsSpan(5, 2));
        uint counter = BinaryPrimitives.ReadUInt32LittleEndian(request.AsSpan(7, 4));
        return ControlFrameCodec.Build((byte)(request[4] | 0x80), reqId, counter, payload, key, nonce);
    }

    /// <summary>HELLO reply bytes (92 B, §2.2). password: 24-char string ya null (= zero field).</summary>
    private static byte[] BuildHelloReply(byte flags, byte[] nonce, byte[] key, string? password = null)
    {
        var bytes = new byte[92];
        bytes[0] = (byte)'R'; bytes[1] = (byte)'S'; bytes[2] = (byte)'H'; bytes[3] = (byte)'R';
        bytes[4] = 2; bytes[5] = 1; bytes[6] = flags; bytes[7] = 0;
        TestMac.CopyTo(bytes, 8);
        bytes[14] = 4; bytes[15] = 0; bytes[16] = 1; bytes[17] = 0;
        nonce.CopyTo(bytes, 18);
        key.CopyTo(bytes, 34);
        if (password is not null)
            Encoding.ASCII.GetBytes(password).CopyTo(bytes, 66);
        ushort crc = Crc16.Compute(bytes.AsSpan(0, 90));
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(90, 2), crc);
        return bytes;
    }

    private static HelloReply HelloReplyFrom(byte[] bytes)
    {
        Assert.Equal(HelloReplyParseResult.Ok, HelloReply.TryParse(bytes, out var reply));
        return reply;
    }

    private sealed class EmptySource : ISampleSource
    {
        public LinkStatistics Statistics { get; } = new();
        public bool TryGetLatest(out WheelPacket packet, out long ageMs, long maxAgeMs = long.MaxValue)
        {
            packet = default;
            ageMs = long.MaxValue;
            return false;
        }
        public long? LastValidAgeMs => null;
    }

    private sealed class PlanRecordingOutput : IJoystickOutput
    {
        public string Name => "PlanRecorder";
        public bool IsConnected { get; private set; }
        public PipelinePlan? SeenPlan { get; private set; }
        public bool TryConnect(out string error)
        {
            IsConnected = true;
            error = "";
            return true;
        }
        public bool TryConnect(out string error, PipelinePlan? plan)
        {
            SeenPlan = plan;
            return TryConnect(out error);
        }
        public void Disconnect() => IsConnected = false;
        public void Update(in ProcessedState state) { }
        public void SendNeutral() { }
        public void Dispose() { }
    }

    // ================= B2-01: pairing-window gate =================

    [Fact]
    public void UpdateFromHello_WindowClosed_NonzeroKey_Ignored_StoredKeyUntouched()
    {
        using var store = new KeyStore();
        using var session1 = new BoardSession(store);
        session1.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey)));
        Assert.True(store.TryLoadKey(TestBoardId, out byte[]? stored) && stored!.AsSpan().SequenceEqual(TestKey));

        // Spoof/ghalat HELLO: unpaired (bit0) lekin window BAND (bit3=0) + nonzero key.
        using var session2 = new BoardSession(store);
        session2.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x01, OtherNonce, OtherKey)));
        Assert.False(session2.HasKey);
        Assert.True(store.TryLoadKey(TestBoardId, out byte[]? stored2) && stored2!.AsSpan().SequenceEqual(TestKey));
    }

    [Fact]
    public void UpdateFromHello_WindowOpenButPaired_NonzeroKey_Ignored()
    {
        using var store = new KeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x08, TestNonce, TestKey)));
        Assert.False(session.HasKey);
        Assert.False(store.TryLoadKey(TestBoardId, out _));
    }

    [Fact]
    public void UpdateFromHello_WindowOpen_NonzeroKey_Accepted()
    {
        using var store = new KeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey)));
        Assert.True(session.HasKey);
        Assert.True(store.TryLoadKey(TestBoardId, out byte[]? stored) && stored!.AsSpan().SequenceEqual(TestKey));
    }

    // ================= B2-02: HELLO Wi-Fi password =================

    [Fact]
    public void UpdateFromHello_WindowOpen_WifiPassword_SavedToStore()
    {
        using var store = new KeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey, TestPassword)));
        Assert.True(store.TryLoadPassword(TestBoardId, out string? pw));
        Assert.Equal(TestPassword, pw);
    }

    [Fact]
    public void UpdateFromHello_ZeroPasswordField_DoesNotOverwriteStoredPassword()
    {
        using var store = new KeyStore();
        using var session = new BoardSession(store);
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey, TestPassword)));
        // Dobara HELLO, password field zero (user-set password case) — stored password waisa hi.
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey, null)));
        Assert.True(store.TryLoadPassword(TestBoardId, out string? pw));
        Assert.Equal(TestPassword, pw);
    }

    // ================= B2-03: KEY_RESET sender =================

    [Fact]
    public async Task SendKeyReset_NotArmed_FailsWithoutFrame()
    {
        using var session = new BoardSession();
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey)));
        Assert.False(session.LastHelloKeyResetArmed);
        var transport = new FakeControlTransport(req => ReplyFor(req, [0x00], TestKey, TestNonce));
        var (sent, error) = await session.SendKeyResetAsync("127.0.0.1", transport, TestContext.Current.CancellationToken);
        Assert.False(sent);
        Assert.Contains("BOOT", error!);
        Assert.Empty(transport.CapturedFrames);
    }

    [Fact]
    public async Task SendKeyReset_Armed_SucceedsAndClearsCredentials()
    {
        using var store = new KeyStore();
        using var session = new BoardSession(store);
        // flags 0x0B = unpaired + armed + window open; password bhi HELLO se.
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x0B, TestNonce, TestKey, TestPassword)));
        Assert.True(session.LastHelloKeyResetArmed);
        Assert.True(session.HasKey);
        var transport = new FakeControlTransport(req => ReplyFor(req, [0x00], TestKey, TestNonce));
        var (sent, error) = await session.SendKeyResetAsync("127.0.0.1", transport, TestContext.Current.CancellationToken);
        Assert.True(sent, error);
        Assert.Single(transport.CapturedFrames);
        Assert.Equal((byte)ControlCommand.KeyReset, transport.CapturedFrames[0][4]);
        Assert.Equal(29, transport.CapturedFrames[0].Length); // 13 header + 0 payload + 16 tag
        // Kamyabi ke baad purani credentials dead hain (board rotate + restart).
        Assert.False(session.HasKey);
        Assert.False(session.CanSendControl);
        Assert.False(session.LastHelloKeyResetArmed);
        Assert.False(store.TryLoadKey(TestBoardId, out _));
        Assert.False(store.TryLoadPassword(TestBoardId, out _));
    }

    [Fact]
    public async Task SendKeyReset_BoardRejects_ReturnsStatusError()
    {
        using var session = new BoardSession();
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x0B, TestNonce, TestKey)));
        var transport = new FakeControlTransport(req => ReplyFor(req, [(byte)ControlStatus.Unsupported], TestKey, TestNonce));
        var (sent, error) = await session.SendKeyResetAsync("127.0.0.1", transport, TestContext.Current.CancellationToken);
        Assert.False(sent);
        Assert.Contains("Unsupported", error!);
        Assert.True(session.HasKey); // reject par credentials nahi mit te
    }

    // ================= B2-04: control-send serialization =================

    [Fact]
    public async Task ConcurrentControlSends_Serialized_UniqueCounters()
    {
        using var session = new BoardSession();
        session.UpdateFromHello(HelloReplyFrom(BuildHelloReply(0x09, TestNonce, TestKey)));
        var transport = new DelayedFakeControlTransport(req => ReplyFor(req, [0x00], TestKey, TestNonce));
        var tasks = Enumerable.Range(0, 8).Select(_ =>
            session.SendControlAsync("127.0.0.1", ControlCommand.GetCaps, ReadOnlyMemory<byte>.Empty, transport, TestContext.Current.CancellationToken)).ToArray();
        ControlResult[] results = await Task.WhenAll(tasks).ConfigureAwait(false);
        Assert.All(results, r => Assert.True(r.Succeeded, r.Error));
        Assert.Equal(8, transport.CapturedFrames.Count);
        uint[] counters = transport.CapturedFrames
            .Select(f => BinaryPrimitives.ReadUInt32LittleEndian(f.AsSpan(7, 4)))
            .OrderBy(c => c).ToArray();
        Assert.Equal([0u, 1u, 2u, 3u, 4u, 5u, 6u, 7u], counters);
    }

    // ================= B2-05: engine plan-aware connect =================

    [Fact]
    public void WheelEngine_Reconnect_PassesPlanToOutput()
    {
        var source = new EmptySource();
        var output = new PlanRecordingOutput();
        using var engine = new WheelEngine(source, output, WheelProfile.CreateDefault());
        engine.Start();
        try
        {
            var deadline = DateTime.UtcNow.AddSeconds(2);
            while (output.SeenPlan is null && DateTime.UtcNow < deadline)
                Thread.Sleep(20);
            Assert.NotNull(output.SeenPlan);
            Assert.Same(engine.Plan, output.SeenPlan);
        }
        finally
        {
            engine.Stop();
        }
    }

    // ================= B2-06: UDP sender filter =================

    [Fact]
    public void UdpWheelSource_SenderFilter_DropsUnexpectedSender()
    {
        using var source = new UdpWheelSource(0);
        source.ExpectedRemoteAddress = IPAddress.Parse("127.0.0.1");
        byte[] v2 = WheelPacketV2.Build(1, 1000, 0, 0x01, 0, [2050], ReadOnlySpan<short>.Empty);
        source.ProcessDatagram(v2, new IPEndPoint(IPAddress.Parse("127.0.0.2"), 4211));
        Assert.Equal(0, source.V2ValidPackets);
        Assert.Equal(1, source.UnknownDatagrams);
        source.ProcessDatagram(v2, new IPEndPoint(IPAddress.Parse("127.0.0.1"), 4211));
        Assert.Equal(1, source.V2ValidPackets);
    }

    [Fact]
    public void UdpWheelSource_NoFilter_AcceptsAnySender()
    {
        using var source = new UdpWheelSource(0);
        byte[] v2 = WheelPacketV2.Build(1, 1000, 0, 0x01, 0, [2050], ReadOnlySpan<short>.Empty);
        source.ProcessDatagram(v2, new IPEndPoint(IPAddress.Parse("127.0.0.2"), 4211));
        Assert.Equal(1, source.V2ValidPackets);
    }

    // ================= B2-08: Horn module gating =================

    private static WheelPacketV2 V2PacketWithButtons(uint buttons)
    {
        byte[] bytes = WheelPacketV2.Build(1, 1000, 0, 0x01, buttons, [2050], ReadOnlySpan<short>.Empty);
        Assert.Equal(WheelPacketV2ParseResult.Ok, WheelPacketV2.TryParse(bytes, out var p));
        return p;
    }

    private static WheelProfile PlainProfile() => WheelProfile.CreateDefault() with
    {
        Smoothing = 0, Deadzone = 0, CurveStrength = 0,
        SteeringCalibration = new SteeringCalibration
        { RawMin = 900, RawCenter = 2050, RawMax = 3200, IsSet = true, IsMillivolts = true },
        AccelCalibration = new PedalCalibration
        { RawMin = 300, RawMax = 3300, IsSet = true, IsMillivolts = true },
        BrakeCalibration = new PedalCalibration
        { RawMin = 300, RawMax = 3300, IsSet = true, IsMillivolts = true },
    };

    [Fact]
    public void ProcessV2_HornModuleOff_HornBitMaskedOut()
    {
        var pipeline = new ProcessingPipeline(PlainProfile());
        pipeline.SetPlan(new PipelinePlanBuilder().WithModule(ModuleKind.Horn, false).Build());
        var state = pipeline.ProcessV2(V2PacketWithButtons(0x10u), 0.005);
        Assert.Equal(0u, state.ButtonsMask & ProcessingPipeline.HornButtonBit);
    }

    [Fact]
    public void ProcessV2_HornModuleOn_HornBitPasses()
    {
        var pipeline = new ProcessingPipeline(PlainProfile());
        var state = pipeline.ProcessV2(V2PacketWithButtons(0x10u), 0.005);
        Assert.NotEqual(0u, state.ButtonsMask & ProcessingPipeline.HornButtonBit);
    }

    // ================= O-1: ChangePasswordDialog picker (classic variant) =================

    [Fact]
    public void ChangePasswordPicker_ClassicMode_SelectsClassic()
    {
        Assert.Equal(ChangePasswordDialogKind.Classic, ChangePasswordDialogPicker.Pick(classicWindowRequested: true));
    }

    [Fact]
    public void ChangePasswordPicker_Win11_SelectsFluent()
    {
        Assert.Equal(ChangePasswordDialogKind.Fluent, ChangePasswordDialogPicker.Pick(classicWindowRequested: false));
    }

    /// <summary>
    /// Classic dialog XAML mein FluentWindow chrome ka koi nishan nahi —
    /// plain Window hi hai (O-1 ka asal guarantee; Window kholna Linux
    /// par mumkin nahi, is liye XAML text hi authority hai).
    /// </summary>
    [Fact]
    public void ChangePasswordClassicXaml_IsPlainWindow_NoFluentChrome()
    {
        string xaml = ReadViewsXaml("ChangePasswordDialogClassic.xaml");
        Assert.Contains("<Window ", xaml);
        Assert.DoesNotContain("FluentWindow", xaml);
        Assert.DoesNotContain("WindowBackdropType", xaml);
        Assert.DoesNotContain("ExtendsContentIntoTitleBar", xaml);
        Assert.DoesNotContain("xmlns:ui=", xaml);
        // Wahi fields/buttons/bindings (Fluent variant jaisi hi).
        Assert.Contains("NewPasswordBox", xaml);
        Assert.Contains("ConfirmPasswordBox", xaml);
        Assert.Contains("{Binding ValidationText}", xaml);
        Assert.Contains("{Binding ChangeCommand}", xaml);
        Assert.Contains("{Binding CancelCommand}", xaml);
    }

    [Fact]
    public void ChangePasswordFluentXaml_Unchanged_StillFluentWindow()
    {
        string xaml = ReadViewsXaml("ChangePasswordDialog.xaml");
        Assert.Contains("ui:FluentWindow", xaml);
    }

    private static string ReadViewsXaml(string fileName)
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null)
        {
            string candidate = Path.Combine(dir.FullName, "software", "src", "RSH.App", "Views", fileName);
            if (File.Exists(candidate))
                return File.ReadAllText(candidate);
            dir = dir.Parent;
        }
        throw new FileNotFoundException($"Views/{fileName} repo mein nahi mila (test bin dir se walk-up fail).");
    }

    // ================= W3-2: 25/26 aliasing — UI warning (PinMap.ValidatePin) =================
    // GPIO 25/26 fixed pedal button-mode pins hain (PIN_ACCEL_BTN 25,
    // PIN_BRAKE_BTN 26) — horn/gear/encoder in par assign ho to board
    // pedal button suppress karta hai (W3 FIX-2, sahi behaviour). UI mein
    // warning honi chahiye; error nahi (Apply allowed rehta hai).

    [Fact]
    public void ValidatePin_HornOn25_WarnsAcceleratorSuppressed()
    {
        var (error, warning) = PinMap.ValidatePin(new PinFunction("Horn", PinFunctionKind.Digital, 4), 25);
        Assert.Null(error);
        Assert.NotNull(warning);
        Assert.Contains("25", warning);
        Assert.Contains("accelerator", warning);
    }

    [Fact]
    public void ValidatePin_GearOn26_WarnsBrakeSuppressed()
    {
        var (error, warning) = PinMap.ValidatePin(new PinFunction("Shifter up", PinFunctionKind.Digital, 27), 26);
        Assert.Null(error);
        Assert.NotNull(warning);
        Assert.Contains("26", warning);
        Assert.Contains("brake", warning);
    }

    [Fact]
    public void ValidatePin_EncoderOn25_WarnsToo()
    {
        var (error, warning) = PinMap.ValidatePin(new PinFunction("Encoder 1 A", PinFunctionKind.Digital, 23), 25);
        Assert.Null(error);
        Assert.NotNull(warning);
    }

    [Fact]
    public void ValidatePin_HornOnDefaultPin_NoWarning()
    {
        var (error, warning) = PinMap.ValidatePin(new PinFunction("Horn", PinFunctionKind.Digital, 4), 4);
        Assert.Null(error);
        Assert.Null(warning);
    }

    [Fact]
    public void ValidateAll_HornOn25_WarningSurfaces_NoError()
    {
        var pins = PinMap.Defaults();
        pins["Horn"] = 25;
        var (rows, errorCount) = PinMap.ValidateAll(pins);
        Assert.Equal(0, errorCount);
        Assert.NotNull(rows["Horn"].Warning);
        Assert.Contains("accelerator", rows["Horn"].Warning);
    }

    // ================= W4: CurrentFirmwareVersion invariant (Benchmarks header) =================

    [Fact]
    public void CurrentFirmwareVersion_AtLeastMinimumFloor()
    {
        Assert.True(FirmwareImageValidator.CurrentFirmwareVersion >= FirmwareImageValidator.MinimumFirmwareVersion);
    }

    [Fact]
    public void CurrentFirmwareVersion_MatchesShippingFirmware()
    {
        // Tripwire, jaan boojh kar: firmware bump par ye test fail hoga
        // jab tak FirmwareImageValidator.CurrentFirmwareVersion aur
        // firmware/rsh/config.h FIRMWARE_VERSION dono na badlein.
        Assert.Equal(new Version(4, 0, 1), FirmwareImageValidator.CurrentFirmwareVersion);
    }
}
