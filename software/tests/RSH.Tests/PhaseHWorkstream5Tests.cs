// Phase H — Workstream 5: transport-switch UI wiring + FakeBoardResponder threading fix (C1).
// (1) TransportSwitchController (RSH.App/Services, WPF-free, is test
// assembly mein link hai): toggle → asal switch, state reflect, fail →
// saaf error + wapas Wi-Fi. Fake ITransportSwitchTarget se.
// (2) FakeBoardResponder (PhaseGWorkstream1Tests): dedicated thread +
// cancellable blocking wait — Dispose thread join karta hai; ye tests
// us guarantee ko pakarte hain (flaky #5/#9 ka gate ×3 runs hai).
using System.Diagnostics;
using RSH.App.Services;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Tests;

public class PhaseHWorkstream5Tests
{
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();

    private static (PhaseGWorkstream1Tests.LoopbackSerialPort Pc, PhaseGWorkstream1Tests.LoopbackSerialPort Board) LinkedPair()
    {
        var pc = new PhaseGWorkstream1Tests.LoopbackSerialPort("COM-PC");
        var board = new PhaseGWorkstream1Tests.LoopbackSerialPort("COM-BOARD");
        pc.Link(board);
        board.Link(pc);
        return (pc, board);
    }

    private static async Task WaitUntilAsync(Func<bool> condition, int timeoutMs = 3000)
    {
        var deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
        while (!condition())
        {
            if (DateTime.UtcNow > deadline) break;
            await Task.Delay(10, TestContext.Current.CancellationToken).ConfigureAwait(false);
        }
        Assert.True(condition());
    }

    // ================= Fake transport target =================

    /// <summary>AppServices ka fake: switch ka natija test control karta hai.</summary>
    private sealed class FakeTransportTarget : ITransportSwitchTarget
    {
        public TransportKind ActiveTransport { get; private set; } = TransportKind.Wifi;
        public string? SerialPortName { get; private set; }
        public int SwitchToSerialCalls { get; private set; }
        public string? LastSerialPort { get; private set; }
        public int SwitchToWifiCalls { get; private set; }
        public int DetectCalls { get; private set; }

        /// <summary>null = kamyabi; warna (Ok, Error) — port naam milta hai.</summary>
        public Func<string, (bool Ok, string? Error)>? SwitchToSerialResult { get; set; }
        /// <summary>null = kamyabi (port COM-DETECTED); warna (Ok, Error).</summary>
        public Func<(bool Ok, string? Error)>? DetectResult { get; set; }
        /// <summary>Set ho to switch is ke complete hone tak ruka rehta hai (busy test).</summary>
        public TaskCompletionSource? HoldSwitch { get; set; }

        public async Task<(bool Ok, string? Error)> SwitchToSerialAsync(string portName, CancellationToken ct = default)
        {
            SwitchToSerialCalls++;
            LastSerialPort = portName;
            if (HoldSwitch is not null) await HoldSwitch.Task.WaitAsync(ct).ConfigureAwait(false);
            var result = SwitchToSerialResult?.Invoke(portName) ?? (true, null);
            if (result.Ok) { ActiveTransport = TransportKind.UsbSerial; SerialPortName = portName; }
            return result;
        }

        public void SwitchToWifi()
        {
            SwitchToWifiCalls++;
            ActiveTransport = TransportKind.Wifi;
            SerialPortName = null;
        }

        public async Task<(bool Ok, string? Error)> DetectAndSwitchToSerialAsync(CancellationToken ct = default)
        {
            DetectCalls++;
            if (HoldSwitch is not null) await HoldSwitch.Task.WaitAsync(ct).ConfigureAwait(false);
            var result = DetectResult?.Invoke() ?? (true, null);
            if (result.Ok) { ActiveTransport = TransportKind.UsbSerial; SerialPortName = "COM-DETECTED"; }
            return result;
        }
    }

    private static TransportSwitchController Controller(
        FakeTransportTarget target, Func<string[]>? listPorts = null) =>
        new(target, listPorts ?? (() => ["COM3", "COM7"]));

    // ================= Transport switch =================

    [Fact]
    public async Task SwitchToSerial_SelectedPort_Succeeds_AndReflectsState()
    {
        var target = new FakeTransportTarget();
        var controller = Controller(target);
        controller.SelectedPortName = "COM7";

        bool ok = await controller.SetUseUsbSerialAsync(true, TestContext.Current.CancellationToken);

        Assert.True(ok);
        Assert.Equal(1, target.SwitchToSerialCalls);
        Assert.Equal("COM7", target.LastSerialPort);
        Assert.Equal(0, target.DetectCalls); // port chuna tha — auto-detect nahi
        Assert.True(controller.IsUsbSerial);
        Assert.Equal("COM7", controller.ActivePortName);
        Assert.Equal(TransportKind.UsbSerial, target.ActiveTransport);
        Assert.Contains("USB-serial", controller.StatusText);
        Assert.Contains("COM7", controller.StatusText);
    }

    [Fact]
    public async Task SwitchToSerial_Fails_RevertsToWifi_WithError()
    {
        var target = new FakeTransportTarget
        {
            SwitchToSerialResult = _ => (false, "port busy"),
        };
        var controller = Controller(target);
        controller.SelectedPortName = "COM7";

        bool ok = await controller.SetUseUsbSerialAsync(true, TestContext.Current.CancellationToken);

        Assert.False(ok);
        Assert.False(controller.IsUsbSerial);
        Assert.Null(controller.ActivePortName);
        Assert.Equal(TransportKind.Wifi, target.ActiveTransport);
        Assert.Equal(0, target.SwitchToWifiCalls); // target pehle se Wi-Fi par — fail-closed, koi wapsi call nahi
        Assert.Contains("port busy", controller.StatusText);
    }

    [Fact]
    public async Task SwitchToSerial_NoPortSelected_AutoDetects()
    {
        var target = new FakeTransportTarget();
        var controller = Controller(target);
        Assert.Null(controller.SelectedPortName); // khaali = auto-detect

        bool ok = await controller.SetUseUsbSerialAsync(true, TestContext.Current.CancellationToken);

        Assert.True(ok);
        Assert.Equal(1, target.DetectCalls);
        Assert.Equal(0, target.SwitchToSerialCalls);
        Assert.True(controller.IsUsbSerial);
        Assert.Equal("COM-DETECTED", controller.ActivePortName);
    }

    [Fact]
    public async Task SwitchToSerial_AutoDetectFails_StaysOnWifi()
    {
        var target = new FakeTransportTarget
        {
            DetectResult = () => (false, "no board found"),
        };
        var controller = Controller(target);

        bool ok = await controller.SetUseUsbSerialAsync(true, TestContext.Current.CancellationToken);

        Assert.False(ok);
        Assert.False(controller.IsUsbSerial);
        Assert.Equal(TransportKind.Wifi, target.ActiveTransport);
        Assert.Contains("no board found", controller.StatusText);
    }

    [Fact]
    public async Task SwitchBackToWifi_CallsSwitchToWifi()
    {
        var target = new FakeTransportTarget();
        var controller = Controller(target);
        controller.SelectedPortName = "COM7";
        Assert.True(await controller.SetUseUsbSerialAsync(true, TestContext.Current.CancellationToken));
        Assert.True(controller.IsUsbSerial);

        bool ok = await controller.SetUseUsbSerialAsync(false, TestContext.Current.CancellationToken);

        Assert.True(ok);
        Assert.Equal(1, target.SwitchToWifiCalls);
        Assert.False(controller.IsUsbSerial);
        Assert.Null(controller.ActivePortName);
        Assert.Equal(TransportKind.Wifi, target.ActiveTransport);
        Assert.Contains("Wi-Fi", controller.StatusText);
    }

    [Fact]
    public async Task SecondToggleWhileBusy_IsIgnored()
    {
        var target = new FakeTransportTarget();
        var hold = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        target.HoldSwitch = hold;
        var controller = Controller(target);
        controller.SelectedPortName = "COM7";

        Task<bool> first = controller.SetUseUsbSerialAsync(true, TestContext.Current.CancellationToken);
        await WaitUntilAsync(() => controller.IsBusy);
        Task<bool> second = controller.SetUseUsbSerialAsync(true, TestContext.Current.CancellationToken);

        Assert.False(await second); // busy-ignored
        Assert.Equal(1, target.SwitchToSerialCalls);
        hold.SetResult();
        Assert.True(await first);
        Assert.True(controller.IsUsbSerial);
        Assert.False(controller.IsBusy);
    }

    [Fact]
    public async Task ToggleSetter_FailedSwitch_RevertsToWifi_WithError()
    {
        // XAML two-way path: setter fire-and-forget hai (async void jaisa) —
        // fail par toggle khud Wi-Fi par wapas aana chahiye.
        var target = new FakeTransportTarget
        {
            SwitchToSerialResult = _ => (false, "port busy"),
        };
        var controller = Controller(target);
        controller.SelectedPortName = "COM7";

        controller.IsUsbSerial = true;
        await WaitUntilAsync(() => !controller.IsBusy && !controller.IsUsbSerial);

        Assert.False(controller.IsUsbSerial);
        Assert.Equal(TransportKind.Wifi, target.ActiveTransport);
        Assert.Contains("port busy", controller.StatusText);
    }

    [Fact]
    public void RefreshPorts_ListsPorts_KeepsSelection()
    {
        string[] ports = ["COM3", "COM7"];
        var target = new FakeTransportTarget();
        var controller = new TransportSwitchController(target, () => ports);
        Assert.Equal(["COM3", "COM7"], controller.AvailablePorts);

        controller.SelectedPortName = "COM7";
        ports = ["COM3"]; // COM7 gayab — selection phir bhi barqarar
        controller.RefreshPorts();

        Assert.Equal(["COM3"], controller.AvailablePorts);
        Assert.Equal("COM7", controller.SelectedPortName);
    }

    [Fact]
    public void ListPortsThrows_ControllerStillWorks_EmptyList()
    {
        var target = new FakeTransportTarget();
        var controller = new TransportSwitchController(target, () => throw new InvalidOperationException("no serial"));
        Assert.Empty(controller.AvailablePorts);
        // Auto-detect rasta phir bhi khula hai (list par inhisar nahi).
        Assert.Null(controller.SelectedPortName);
    }

    // ================= FakeBoardResponder threading (C1) =================

    [Fact]
    public async Task Responder_RunsOnDedicatedThread_NotThreadPool()
    {
        var (_, board) = LinkedPair();
        using var responder = new PhaseGWorkstream1Tests.FakeBoardResponder(board);
        await WaitUntilAsync(() => responder.Started);
        Assert.False(responder.IsThreadPoolThread);
        Assert.True(responder.ThreadId > 0);
    }

    [Fact]
    public async Task Responder_Dispose_JoinsThread_NoLinger()
    {
        var (_, board) = LinkedPair();
        var responder = new PhaseGWorkstream1Tests.FakeBoardResponder(board);
        await WaitUntilAsync(() => responder.Started);
        Assert.True(responder.IsAlive);

        responder.Dispose();

        // Dispose thread join ho kar hi wapas aata hai — baad mein koi
        // lingering thread disposed port par nahi likhta.
        Assert.False(responder.IsAlive);
    }

    [Fact]
    public async Task Responder_DisposeDuringControlDelay_JoinsPromptly()
    {
        // Timing-robust: control delay 5 s hai, Dispose ~100 ms par hota
        // hai — loaded VM par Task.Delay oversleep bhi kare to Dispose
        // phir bhi delay ke andar rehta hai (400 ms delay par ye test
        // full-suite load mein flaky tha: delay poora ho kar reply aa
        // jata tha, is liye Assert.Null fail hota tha).
        var (pc, board) = LinkedPair();
        using var link = new SerialLink(pc);
        var responder = new PhaseGWorkstream1Tests.FakeBoardResponder(board, controlDelayMs: 5000);
        link.Start();
        byte[] request = ControlFrameCodec.Build((byte)ControlCommand.GetCaps, 1, 1, Array.Empty<byte>(), TestKey, TestNonce);
        Task<byte[]?> exchange = link.ExchangeControlAsync(request, TimeSpan.FromSeconds(2), TestContext.Current.CancellationToken);
        await Task.Delay(100, TestContext.Current.CancellationToken); // responder delay mein hai

        var sw = Stopwatch.StartNew();
        responder.Dispose();
        sw.Stop();

        Assert.False(responder.IsAlive);
        Assert.True(sw.Elapsed < TimeSpan.FromSeconds(5));
        link.Stop();
        // Jawab kabhi nahi aaya (responder delay mein band hua) — timeout par null.
        Assert.Null(await exchange);
    }
}
