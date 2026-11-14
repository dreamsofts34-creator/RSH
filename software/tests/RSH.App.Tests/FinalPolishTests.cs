// Final polish sweep (2026-11-13) — Backend ki 3 Frontend findings:
// 1. Wizard calibration unit mismatch (V-C2 class): wizard intake raw
//    ADC values ko mV-domain calibration mein daal deta tha.
// 2. PinMapBoardPush CONFIRM-retry edge: pehle se confirmed config par
//    UNSUPPORTED ko "fail" + REVERT samjha jata tha.
// 3. DiagnosticsSettings "Windows 11" wording (gate Win10 19045 par
//    pass karta hai) — RuntimeCheck.RequirementsText istemal hona chahiye.
using RSH.App.ViewModels;
using RSH.App.Views;
using RSH.Core.Processing;
using RSH.Core.Protocol;
using RSH.Core.Transport;
using RSH.Core.Utilities;

namespace RSH.App.Tests;

/// <summary>Final polish sweep ke tests — wizard intake, CONFIRM edge, wording.</summary>
public sealed class FinalPolishTests
{
    // ================= Fix 1: wizard calibration intake (mV-domain) =================

    [Fact]
    public void WizardIntake_RawSteering_ConvertsToMillivolts()
    {
        // Raw fallback: flag false, center raw 2048 → 1650 mV (na ke 2048).
        var packet = new WheelPacket(1, 0, 2048, 0, 0, 0, SteeringInMillivolts: false);
        Assert.Equal(1650, WizardCalibrationIntake.SteeringToMillivolts(packet));

        var cal = new AutoCalibrator();
        for (int i = 0; i < 20; i++) cal.AddCenterSample(WizardCalibrationIntake.SteeringToMillivolts(packet));
        Assert.True(cal.TryGetCenter(out ushort center));
        Assert.Equal(1650, center);

        // Full lock-to-lock raw (0..4095) → mV (0..3300), span gate mV mein pass.
        for (int i = 0; i < 30; i++) cal.AddSteeringRangeSample(WizardCalibrationIntake.SteeringToMillivolts(new WheelPacket(1, 0, 0, 0, 0, 0, false)));
        for (int i = 0; i < 30; i++) cal.AddSteeringRangeSample(WizardCalibrationIntake.SteeringToMillivolts(new WheelPacket(1, 0, 4095, 0, 0, 0, false)));
        Assert.True(cal.TryGetSteeringRange(out ushort min, out ushort max));
        Assert.Equal(0, min);
        Assert.Equal(3300, max);
    }

    [Fact]
    public void WizardIntake_RawSteering_NarrowSpan_FailsMillivoltGate()
    {
        // Raw span 700 (1700..2400) purane code mein gate pass kar deta
        // (700 >= 660) aur ghalat calibration save hoti; mV span sirf
        // 564 hai (1370..1934) — gate ko FAIL hona chahiye.
        var cal = new AutoCalibrator();
        for (int i = 0; i < 30; i++) cal.AddSteeringRangeSample(WizardCalibrationIntake.SteeringToMillivolts(new WheelPacket(1, 0, 1700, 0, 0, 0, false)));
        for (int i = 0; i < 30; i++) cal.AddSteeringRangeSample(WizardCalibrationIntake.SteeringToMillivolts(new WheelPacket(1, 0, 2400, 0, 0, 0, false)));
        Assert.Equal(1370, WizardCalibrationIntake.SteeringToMillivolts(new WheelPacket(1, 0, 1700, 0, 0, 0, false)));
        Assert.Equal(1934, WizardCalibrationIntake.SteeringToMillivolts(new WheelPacket(1, 0, 2400, 0, 0, 0, false)));
        Assert.False(cal.TryGetSteeringRange(out _, out _));
    }

    [Fact]
    public void WizardIntake_MillivoltSteering_PassesThroughUnchanged()
    {
        // Default mV mode: flag true ho to value jaisi hai waisi hi rahe.
        var cal = new AutoCalibrator();
        for (int i = 0; i < 20; i++) cal.AddCenterSample(WizardCalibrationIntake.SteeringToMillivolts(new WheelPacket(1, 0, 2050, 0, 0, 0, true)));
        Assert.True(cal.TryGetCenter(out ushort center));
        Assert.Equal(2050, center);
        for (int i = 0; i < 30; i++) cal.AddSteeringRangeSample(WizardCalibrationIntake.SteeringToMillivolts(new WheelPacket(1, 0, 900, 0, 0, 0, true)));
        for (int i = 0; i < 30; i++) cal.AddSteeringRangeSample(WizardCalibrationIntake.SteeringToMillivolts(new WheelPacket(1, 0, 3200, 0, 0, 0, true)));
        Assert.True(cal.TryGetSteeringRange(out ushort min, out ushort max));
        Assert.Equal(900, min);
        Assert.Equal(3200, max);
    }

    [Fact]
    public void WizardIntake_RawPedals_ConvertToMillivolts()
    {
        // v1 pedals hamesha raw ADC hote hain — 0..4095 → 0..3300 mV.
        Assert.Equal(0, WizardCalibrationIntake.PedalToMillivolts(0));
        Assert.Equal(1650, WizardCalibrationIntake.PedalToMillivolts(2048));
        Assert.Equal(3300, WizardCalibrationIntake.PedalToMillivolts(4095));

        var cal = new AutoCalibrator();
        for (int i = 0; i < 30; i++) cal.AddAccelSample(WizardCalibrationIntake.PedalToMillivolts(0));
        for (int i = 0; i < 30; i++) cal.AddAccelSample(WizardCalibrationIntake.PedalToMillivolts(4095));
        Assert.True(cal.TryGetAccelRange(out ushort aMin, out ushort aMax));
        Assert.Equal(0, aMin);
        Assert.Equal(3300, aMax);
    }

    // ================= Fix 2: PinMapBoardPush CONFIRM-retry edge =================

    private sealed class ScriptedBoard
    {
        public List<ControlCommand> Received { get; } = [];
        public Queue<ControlResult> ConfirmScript { get; } = new();
        private readonly int _total;

        public ScriptedBoard() => _total = PinMapBoardPush.BuildBlob(PinMap.Defaults(), cfgVersion: 1).Length;

        public Task<ControlResult> Send(ControlCommand cmd, ReadOnlyMemory<byte> payload, CancellationToken ct)
        {
            Received.Add(cmd);
            return Task.FromResult(cmd switch
            {
                ControlCommand.SetCfg => SetCfgOk(payload.Span),
                ControlCommand.Confirm => ConfirmScript.Count > 0 ? ConfirmScript.Dequeue() : OkResult(),
                _ => OkResult(),
            });
        }

        private ControlResult SetCfgOk(ReadOnlySpan<byte> chunk)
        {
            int offset = chunk[0] | (chunk[1] << 8);
            int chunkLen = chunk[4] | (chunk[5] << 8);
            int staged = offset + chunkLen;
            return new ControlResult(
                new byte[] { 0, (byte)(staged & 0xFF), (byte)(staged >> 8), (byte)(_total & 0xFF), (byte)(_total >> 8) },
                ControlStatus.Ok, null, 1);
        }

        internal static ControlResult OkResult() =>
            new(new byte[] { 0 }, ControlStatus.Ok, null, 1);

        internal static ControlResult TimeoutResult() =>
            new(null, null, "Board se 250 ms mein jawab nahi aaya (koshish 3/3).", 3);

        internal static ControlResult UnsupportedResult() =>
            new(new byte[] { (byte)ControlStatus.Unsupported }, ControlStatus.Unsupported, "Board ne Unsupported (6) diya.", 1);

        internal static ControlResult BusyResult() =>
            new(new byte[] { (byte)ControlStatus.Busy }, ControlStatus.Busy, "Board ne Busy (7) diya.", 1);
    }

    [Fact]
    public async Task PushAsync_ConfirmTimeoutThenUnsupported_ReportsSuccess_NoRevert()
    {
        // Task ka case: CONFIRM timeout, phir retry par UNSUPPORTED —
        // pehli koshish confirm kar chuki thi, reply kho gaya tha.
        var board = new ScriptedBoard();
        board.ConfirmScript.Enqueue(ScriptedBoard.TimeoutResult());
        board.ConfirmScript.Enqueue(ScriptedBoard.UnsupportedResult());
        var (ok, error, chunks) = await PinMapBoardPush.PushAsync(
            PinMap.Defaults(), board.Send, cfgVersion: 1,
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(ok, error);
        Assert.Null(error);
        Assert.Equal(1, chunks);
        Assert.Equal(new[] { ControlCommand.SetCfg, ControlCommand.Commit, ControlCommand.Confirm, ControlCommand.Confirm },
            board.Received);
        Assert.DoesNotContain(ControlCommand.Revert, board.Received);
    }

    [Fact]
    public async Task PushAsync_ConfirmUnsupportedDirectly_ReportsSuccess_NoRevert()
    {
        // ControlClient ke andar ki retry (wahi reqId, naya counter) se
        // aaya UNSUPPORTED — seedha already-confirmed success hai.
        var board = new ScriptedBoard();
        board.ConfirmScript.Enqueue(ScriptedBoard.UnsupportedResult());
        var (ok, error, chunks) = await PinMapBoardPush.PushAsync(
            PinMap.Defaults(), board.Send, cfgVersion: 1,
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.True(ok, error);
        Assert.Equal(1, chunks);
        Assert.Equal(new[] { ControlCommand.SetCfg, ControlCommand.Commit, ControlCommand.Confirm },
            board.Received);
        Assert.DoesNotContain(ControlCommand.Revert, board.Received);
    }

    [Fact]
    public async Task PushAsync_ConfirmBusy_FailsAndReverts()
    {
        // Regression: asal reject (Busy) abhi bhi fail + REVERT hai —
        // sirf UNSUPPORTED ko already-confirmed samjha jata hai.
        var board = new ScriptedBoard();
        board.ConfirmScript.Enqueue(ScriptedBoard.BusyResult());
        var (ok, error, _) = await PinMapBoardPush.PushAsync(
            PinMap.Defaults(), board.Send, cfgVersion: 1,
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.False(ok);
        Assert.Contains("CONFIRM", error!);
        Assert.Equal(new[] { ControlCommand.SetCfg, ControlCommand.Commit, ControlCommand.Confirm, ControlCommand.Revert },
            board.Received);
    }

    [Fact]
    public async Task PushAsync_ConfirmTimeoutTwice_FailsAndReverts()
    {
        // Dobara bhi no-reply — confirm hua ya nahi, maloom nahi:
        // fail + REVERT (config 30 s deadline par rollback hoti).
        var board = new ScriptedBoard();
        board.ConfirmScript.Enqueue(ScriptedBoard.TimeoutResult());
        board.ConfirmScript.Enqueue(ScriptedBoard.TimeoutResult());
        var (ok, error, _) = await PinMapBoardPush.PushAsync(
            PinMap.Defaults(), board.Send, cfgVersion: 1,
            cancellationToken: TestContext.Current.CancellationToken);
        Assert.False(ok);
        Assert.Contains("CONFIRM", error!);
        Assert.Equal(new[] { ControlCommand.SetCfg, ControlCommand.Commit, ControlCommand.Confirm, ControlCommand.Confirm, ControlCommand.Revert },
            board.Received);
    }

    // ================= Fix 3: "Windows 11" wording → RequirementsText =================

    [Fact]
    public void RequirementsText_MentionsWindows10_22H2()
    {
        Assert.Equal(
            "RSH needs .NET 10+ on Windows 10 22H2 (build 19045) or newer.",
            RuntimeCheck.RequirementsText);
    }

    [Fact]
    public void DiagnosticsSettings_UpdateGate_UsesRequirementsText()
    {
        string src = ReadRepoFileOrSkip(Path.Combine("software", "src", "RSH.App", "ViewModels", "DiagnosticsSettings.cs"));
        Assert.Contains("RuntimeCheck.RequirementsText", src, StringComparison.Ordinal);
        Assert.DoesNotContain("Windows 11", src, StringComparison.Ordinal);
    }

    private static string ReadRepoFileOrSkip(string relative)
    {
        string? path = FindRepoFile(relative);
        if (path is null) Assert.Skip($"Repo file nahi mili (test run ki layout alag hai): {relative}");
        return File.ReadAllText(path).Replace("\r\n", "\n");
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
}
