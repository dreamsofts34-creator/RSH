// RSH wired USB-serial transport — serial link (Phase G, D26 / OPEN-20).
// Ek SerialPort, kayi logical channels: stream/status/probe frames
// events se jate hain (SerialWheelSource), control/ping/HELO replies
// waiter se (SerialControlTransport / SerialPingTransport / SendHelloAsync).
// Frame format: docs/PROTOCOL-V2.1.md §10; codec SerialFrame.cs.
//
// Ek waqt mein ek hi control exchange / HELO / ping ho sakta hai —
// doosra caller InvalidOperationException pata hai (ControlClient
// sequential hai; BoardSession har call par naya client banata hai
// lekin ek waqt mein ek hi command chalti hai).
using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>
/// Serial link: ek port par "RSHS" frames parhna/bhejna + kind ke
/// mutabiq demux. Stream/status/probe → events; control/ping/HELLO →
/// waiter (Exchange*/SendHelloAsync). Read loop background thread par
/// hai; send lock-protected hai (do threads ek saath frame na likhein).
/// </summary>
public sealed class SerialLink : IDisposable
{
    private readonly ISerialPort _port;
    private readonly bool _ownsPort;
    private readonly SerialFrameDecoder _decoder = new();
    private readonly object _sendLock = new();
    private readonly object _waiterLock = new();
    private readonly object _lifecycleLock = new();

    private TaskCompletionSource<byte[]>? _controlWaiter;
    private TaskCompletionSource<HelloReply>? _helloWaiter;
    private TaskCompletionSource<byte[]>? _pingWaiter;

    private CancellationTokenSource? _cts;
    private Thread? _readThread;
    private bool _disposed;
    private long _portErrorCount;
    private string? _lastPortError;
    private long _helloBadCount;
    private long _unsolicitedControl;
    private long _unsolicitedPing;
    private long _droppedInbound;

    /// <summary>Stream frame aaya (payload = v2.1 "RSH2" ya v1 packet bytes — wahi jo UDP par aate).</summary>
    public event Action<byte[]>? StreamFrameReceived;
    /// <summary>Status frame aaya (payload = v2.1 status packet bytes).</summary>
    public event Action<byte[]>? StatusFrameReceived;
    /// <summary>Probe frame aaya (payload = "RSHP" frame bytes).</summary>
    public event Action<byte[]>? ProbeFrameReceived;
    /// <summary>Valid HELLO reply aaya (parse ho gaya — BoardSession isi se nonce/key leta hai).</summary>
    public event Action<HelloReply>? HelloReplyReceived;

    public SerialLink(ISerialPort port, bool ownsPort = true)
    {
        ArgumentNullException.ThrowIfNull(port);
        _port = port;
        _ownsPort = ownsPort;
        // Gira hua HELLO (bad CRC / bad length) decoder se deliver nahi
        // hota — phir bhi HelloBadCount mein gina jata hai (§10: frame
        // CRC kharab ho to bhi HELLO kharab hai; magic kharab ho to kind
        // maloom hi nahi hota, wo decoder ke BadMagic mein rehta hai).
        _decoder.FrameDropped += OnFrameDropped;
    }

    private void OnFrameDropped(SerialFrameKind kind, SerialFrameDropReason reason)
    {
        _ = reason;
        if (kind == SerialFrameKind.Hello)
            Interlocked.Increment(ref _helloBadCount);
    }

    /// <summary>Port ka naam (misal COM3 ya /dev/ttyUSB0).</summary>
    public string PortName => _port.PortName;
    /// <summary>Port khula hai?</summary>
    public bool IsOpen => _port.IsOpen;
    /// <summary>Read loop chal raha hai?</summary>
    public bool IsRunning
    {
        get { lock (_lifecycleLock) return _readThread is { IsAlive: true }; }
    }

    /// <summary>Decoder counters (diagnostics).</summary>
    public long FramesOk => _decoder.FramesOk;
    /// <summary>Magic scan mein gire hue bytes.</summary>
    public long BadMagic => _decoder.BadMagic;
    /// <summary>CRC mismatch par gire hue frames.</summary>
    public long BadCrc => _decoder.BadCrc;
    /// <summary>Len &gt; 1053 ya kind unknown par gire hue frames.</summary>
    public long BadLength => _decoder.BadLength;
    /// <summary>HELLO frames jo parse nahi hue — frame CRC/length kharab ho (decoder drop), ya payload parse na ho (ghalat magic/version/length/CRC). Magic kharab ho to kind maloom hi nahi hota — wo <see cref="BadMagic"/> mein gina jata hai, yahan nahi.</summary>
    public long HelloBadCount => Interlocked.Read(ref _helloBadCount);
    /// <summary>Control replies jo kisi waiter ke baghair aain (koi exchange nahi chal raha tha).</summary>
    public long UnsolicitedControlFrames => Interlocked.Read(ref _unsolicitedControl);
    /// <summary>Ping replies jo kisi waiter ke baghair aain.</summary>
    public long UnsolicitedPingFrames => Interlocked.Read(ref _unsolicitedPing);
    /// <summary>Board ko na chahiye wale inbound frames (HELO kind PC se — board ko jata hai, PC ko nahi aana chahiye).</summary>
    public long DroppedInboundFrames => Interlocked.Read(ref _droppedInbound);
    /// <summary>Ab tak kitni port errors aain (read loop mein).</summary>
    public long PortErrorCount => Interlocked.Read(ref _portErrorCount);
    /// <summary>Aakhri port error ka message (koi error na ho to null).</summary>
    public string? LastPortError => _lastPortError;

    /// <summary>Port kholo + read loop shuru karo. Pehle se chal raha ho to kuch nahi.</summary>
    public void Start()
    {
        lock (_lifecycleLock)
        {
            if (_disposed) return;
            if (_readThread is { IsAlive: true }) return;
            if (!_port.IsOpen)
            {
                _port.ReadTimeout = 250;
                _port.Open();
            }
            try { _port.DiscardInBuffer(); } catch { /* stale bytes na nikle to decoder resync karta hai */ }
            _decoder.Reset();
            var cts = new CancellationTokenSource();
            _cts = cts;
            // Read loop DEDICATED thread par hai (Task.Run nahi): ye loop
            // mustaqil blocking Read karta hai — thread-pool thread par
            // aisa loop pool ko starve karta hai (parallel tests / loaded
            // PC par Task.Run schedule hone mein seconds lag sakte hain aur
            // 500 ms probe / 2 s exchange timeouts khaali expire ho jate
            // hain). Dedicated thread foran start hota hai, pool se azaad.
            var thread = new Thread(() => ReadLoop(cts.Token))
            {
                IsBackground = true,
                Name = $"RSH-SerialLink-{PortName}",
            };
            _readThread = thread;
            thread.Start();
        }
    }

    /// <summary>Read loop roko + port band karo. Idempotent; dobara Start() chal sakta hai.</summary>
    public void Stop()
    {
        Thread? thread;
        CancellationTokenSource? cts;
        lock (_lifecycleLock)
        {
            (thread, cts) = TakeLocked();
        }
        CancelWaiters();
        if (thread is null)
        {
            try { cts?.Dispose(); } catch (ObjectDisposedException) { }
            ClosePortQuietly();
            return;
        }
        try { cts?.Cancel(); } catch (ObjectDisposedException) { }
        try
        {
            if (Thread.CurrentThread != thread) thread.Join(500);
        }
        catch { /* band hote waqt khamoshi behtareen */ }
        try { cts?.Dispose(); } catch (ObjectDisposedException) { }
        ClosePortQuietly();
    }

    private void ClosePortQuietly()
    {
        try { if (_port.IsOpen) _port.Close(); } catch { /* band hote waqt khamoshi */ }
    }

    private (Thread? Thread, CancellationTokenSource? Cts) TakeLocked()
    {
        var thread = _readThread;
        var cts = _cts;
        _readThread = null;
        _cts = null;
        return (thread, cts);
    }

    /// <summary>Pending waiters cancel karo (Stop/Dispose par — exchange calls latke nahi).</summary>
    private void CancelWaiters()
    {
        TaskCompletionSource<byte[]>? control;
        TaskCompletionSource<HelloReply>? hello;
        TaskCompletionSource<byte[]>? ping;
        lock (_waiterLock)
        {
            (control, hello, ping) = (_controlWaiter, _helloWaiter, _pingWaiter);
            _controlWaiter = null;
            _helloWaiter = null;
            _pingWaiter = null;
        }
        control?.TrySetCanceled();
        hello?.TrySetCanceled();
        ping?.TrySetCanceled();
    }

    /// <summary>Ek frame bhejo (kind + payload). Port band ho ya frame na bane to false.</summary>
    public bool SendFrame(SerialFrameKind kind, ReadOnlySpan<byte> payload)
    {
        if (!_port.IsOpen) return false;
        byte[] frame;
        try
        {
            frame = SerialFrameCodec.Build(kind, payload);
        }
        catch (ArgumentException)
        {
            return false;
        }
        lock (_sendLock)
        {
            try
            {
                _port.Write(frame, 0, frame.Length);
                return true;
            }
            catch (IOException) { return false; }
            catch (InvalidOperationException) { return false; }
        }
    }

    /// <summary>
    /// HELO bhejo (5 B, §2.1) aur HELLO reply ka intezar karo. Pehli
    /// dafa <paramref name="newSession"/> true do (naya session — naya
    /// nonce, counter 0 se); baad mein false (resume). Timeout ya port
    /// band ho to null.
    /// </summary>
    public async Task<HelloReply?> SendHelloAsync(bool newSession, TimeSpan timeout, CancellationToken cancellationToken = default)
    {
        var waiter = new TaskCompletionSource<HelloReply>(TaskCreationOptions.RunContinuationsAsynchronously);
        lock (_waiterLock)
        {
            if (_helloWaiter is not null)
                throw new InvalidOperationException("Pehle se ek HELO exchange chal raha hai — ek waqt mein ek hi HELO ho sakta hai.");
            _helloWaiter = waiter;
        }
        try
        {
            if (!SendFrame(SerialFrameKind.Helo, HelloClient.BuildHello(newSession)))
                return null;
            using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            timeoutCts.CancelAfter(timeout);
            try
            {
                return await waiter.Task.WaitAsync(timeoutCts.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                return null;
            }
        }
        finally
        {
            lock (_waiterLock)
            {
                if (_helloWaiter == waiter) _helloWaiter = null;
            }
        }
    }

    /// <summary>Ek control frame bhejo, reply frame ka intezar karo. Timeout / cancel / send fail par null.</summary>
    public async Task<byte[]?> ExchangeControlAsync(byte[] frame, TimeSpan timeout, CancellationToken cancellationToken = default)
    {
        var waiter = new TaskCompletionSource<byte[]>(TaskCreationOptions.RunContinuationsAsynchronously);
        lock (_waiterLock)
        {
            if (_controlWaiter is not null)
                throw new InvalidOperationException("Pehle se ek control exchange chal raha hai — ek waqt mein ek hi control exchange ho sakta hai.");
            _controlWaiter = waiter;
        }
        try
        {
            if (!SendFrame(SerialFrameKind.Control, frame))
                return null;
            using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            timeoutCts.CancelAfter(timeout);
            try
            {
                return await waiter.Task.WaitAsync(timeoutCts.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                return null;
            }
        }
        finally
        {
            lock (_waiterLock)
            {
                if (_controlWaiter == waiter) _controlWaiter = null;
            }
        }
    }

    /// <summary>Ek ping frame bhejo, reply frame ka intezar karo. Timeout / cancel / send fail par null.</summary>
    public async Task<byte[]?> ExchangePingAsync(byte[] request, TimeSpan timeout, CancellationToken cancellationToken = default)
    {
        var waiter = new TaskCompletionSource<byte[]>(TaskCreationOptions.RunContinuationsAsynchronously);
        lock (_waiterLock)
        {
            if (_pingWaiter is not null)
                throw new InvalidOperationException("Pehle se ek ping exchange chal raha hai — ek waqt mein ek hi ping ho sakta hai.");
            _pingWaiter = waiter;
        }
        try
        {
            if (!SendFrame(SerialFrameKind.Ping, request))
                return null;
            using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            timeoutCts.CancelAfter(timeout);
            try
            {
                return await waiter.Task.WaitAsync(timeoutCts.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                return null;
            }
        }
        finally
        {
            lock (_waiterLock)
            {
                if (_pingWaiter == waiter) _pingWaiter = null;
            }
        }
    }

    private void ReadLoop(CancellationToken token)
    {
        byte[] buf = new byte[256];
        while (!token.IsCancellationRequested)
        {
            int n;
            try
            {
                n = _port.Read(buf, 0, buf.Length);
            }
            catch (OperationCanceledException) { break; }
            catch (ObjectDisposedException) { break; }
            catch (IOException ex)
            {
                Interlocked.Increment(ref _portErrorCount);
                _lastPortError = ex.Message;
                break; // cable unplugged / port gaya — loop band, dobara Start() se khulega
            }
            catch (InvalidOperationException ex)
            {
                Interlocked.Increment(ref _portErrorCount);
                _lastPortError = ex.Message;
                break;
            }
            if (n <= 0) continue; // timeout — dobara parho
            for (int i = 0; i < n; i++)
            {
                if (_decoder.Feed(buf[i]) && _decoder.LastFrame is { } frame)
                    DispatchFrame(frame);
            }
        }
    }

    private void DispatchFrame(SerialFrame frame)
    {
        switch (frame.Kind)
        {
            case SerialFrameKind.Stream:
                InvokeSafely(StreamFrameReceived, frame.Payload);
                return;
            case SerialFrameKind.Status:
                InvokeSafely(StatusFrameReceived, frame.Payload);
                return;
            case SerialFrameKind.Probe:
                InvokeSafely(ProbeFrameReceived, frame.Payload);
                return;
            case SerialFrameKind.Hello:
            {
                var parse = HelloReply.TryParse(frame.Payload, out var reply);
                if (parse != HelloReplyParseResult.Ok)
                {
                    Interlocked.Increment(ref _helloBadCount);
                    return;
                }
                InvokeSafely(HelloReplyReceived, reply);
                TaskCompletionSource<HelloReply>? waiter;
                lock (_waiterLock) { waiter = _helloWaiter; }
                waiter?.TrySetResult(reply);
                return;
            }
            case SerialFrameKind.Control:
            {
                TaskCompletionSource<byte[]>? waiter;
                lock (_waiterLock) { waiter = _controlWaiter; }
                if (waiter is null) Interlocked.Increment(ref _unsolicitedControl);
                else waiter.TrySetResult(frame.Payload);
                return;
            }
            case SerialFrameKind.Ping:
            {
                TaskCompletionSource<byte[]>? waiter;
                lock (_waiterLock) { waiter = _pingWaiter; }
                if (waiter is null) Interlocked.Increment(ref _unsolicitedPing);
                else waiter.TrySetResult(frame.Payload);
                return;
            }
            default:
                Interlocked.Increment(ref _droppedInbound);
                return;
        }
    }

    private static void InvokeSafely(Action<byte[]>? handler, byte[] payload)
    {
        if (handler is null) return;
        try { handler(payload); }
        catch { /* listener ki exception read loop ko nahi torti */ }
    }

    private static void InvokeSafely(Action<HelloReply>? handler, HelloReply reply)
    {
        if (handler is null) return;
        try { handler(reply); }
        catch { /* listener ki exception read loop ko nahi torti */ }
    }

    public void Dispose()
    {
        Thread? thread;
        CancellationTokenSource? cts;
        lock (_lifecycleLock)
        {
            if (_disposed) return;
            _disposed = true;
            (thread, cts) = TakeLocked();
        }
        CancelWaiters();
        try { cts?.Cancel(); } catch (ObjectDisposedException) { }
        try
        {
            if (thread is not null && Thread.CurrentThread != thread) thread.Join(500);
        }
        catch { /* band hote waqt khamoshi */ }
        try { cts?.Dispose(); } catch (ObjectDisposedException) { }
        ClosePortQuietly();
        if (_ownsPort) _port.Dispose();
    }
}
