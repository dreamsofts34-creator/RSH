using System.Diagnostics;
using ApexLink.Core.Output;
using ApexLink.Core.Processing;
using ApexLink.Core.Profiles;
using ApexLink.Core.Transport;

namespace ApexLink.Core.Pipeline;

/// <summary>
/// Poora nizam chalane wala engine: 200 Hz high-resolution loop mein
/// source se latest packet → pipeline → output. Teen mehfooz usool:
/// 1) Data 250 ms se purana ho to output NEUTRAL (watchdog).
/// 2) Output band ho ya device na mile to bhi loop chalta rehta hai —
///    UI hamesha zinda state dikhati hai.
/// 3) Output ki kharabi (exception) loop ko kabhi nahi torti; dobara
///    connect ki koshish har 2 s par hoti rehti hai.
/// UI is se snapshot padhti hai (30 Hz par) — engine UI ko nahi bulata.
/// </summary>
public sealed class WheelEngine : IDisposable
{
    public const long WatchdogMs = 250;
    private const long TickMs = 5;

    private readonly ISampleSource _source;
    private readonly IJoystickOutput _output;
    private readonly ProcessingPipeline _pipeline;

    private Thread? _thread;
    private volatile bool _running;
    private volatile bool _outputEnabled = true;
    private volatile bool _disposed;
    private bool _neutralSentWhileDisabled;
    private bool _wasFresh;
    private long _lastReconnectAttemptMs = -2000;   // pehla tick hi connect kare — 2 s intezar nahi
    private string? _lastOutputError;

    // AUDIT FIX: ye halat engine thread likhta hai aur UI thread padhta
    // hai — ProcessedState struct hai, is liye volatile ki jagah chhota
    // sa lock (200 Hz par bhi sasta) taaza aur poori qeemat ki zamanat
    // deta hai.
    private readonly object _stateLock = new();
    private ProcessedState _currentState = ProcessedState.Neutral;
    private volatile bool _watchdogActive = true;

    public WheelEngine(ISampleSource source, IJoystickOutput output, WheelProfile profile)
    {
        _source = source;
        _output = output;
        _pipeline = new ProcessingPipeline(profile);
    }

    public ProcessedState CurrentState { get { lock (_stateLock) return _currentState; } }
    public bool IsRunning => _running;
    public bool OutputEnabled { get => _outputEnabled; set => _outputEnabled = value; }
    public bool OutputConnected => _output.IsConnected;
    public string? LastOutputError => _lastOutputError;

    /// <summary>Wheel se data aa raha hai (2 s ke andar koi valid packet).</summary>
    public bool WheelConnected => _source.LastValidAgeMs is < 2000;

    /// <summary>Is lamhe output neutral watchdog ki wajah se hai?</summary>
    public bool WatchdogActive => _watchdogActive;

    // Timing sachai (benchmarks inhi par chalte hain — dawa nahi, number hai)
    public long TickCount { get; private set; }
    public long LateTickCount { get; private set; }
    public long MaxTickDurationUs { get; private set; }
    public long TotalTickDurationUs { get; private set; }

    public LinkStatistics Statistics => _source.Statistics;
    public WheelProfile Profile => _pipeline.Profile;

    public void SetProfile(WheelProfile profile) => _pipeline.SetProfile(profile);

    public void Start()
    {
        if (_running || _disposed) return;   // AUDIT FIX: Dispose ke baad restart disposed output ko zinda kar deta tha
        _running = true;
        _thread = new Thread(Loop) { IsBackground = true, Name = "ApexLink-Engine", Priority = ThreadPriority.AboveNormal };
        _thread.Start();
    }

    public void Stop()
    {
        _running = false;
        _thread?.Join(1000);
        _thread = null;
        try { if (_output.IsConnected) _output.SendNeutral(); } catch { /* band hote waqt khamoshi behtareen */ }
        _output.Disconnect();
    }

    private void Loop()
    {
        var clock = Stopwatch.StartNew();
        long nextTick = clock.ElapsedMilliseconds + TickMs;
        long lastTickMs = clock.ElapsedMilliseconds;

        while (_running)
        {
            long now = clock.ElapsedMilliseconds;
            long wait = nextTick - now;
            if (wait > 1)
            {
                Thread.Sleep((int)Math.Min(wait - 1, 10));
                continue;
            }
            // AUDIT FIX: "late" ka matlab hai tick apni jagah (slot) se
            // chuka — kaam ke doraniye se nahi. 6 ms ka kaam karke bhi
            // tick waqt par ho sakta hai (resync ke baad), aur halka kaam
            // bhi der se shuru ho to late hai.
            bool missedSlot = wait < -TickMs;
            if (wait < -25) nextTick = now;   // bohat peeche — resync, spiral nahi

            double dt = Math.Clamp(now - lastTickMs, 1, 50) / 1000.0;
            lastTickMs = now;
            nextTick += TickMs;

            long tickStart = Stopwatch.GetTimestamp();
            TickOnce(dt, now);
            long tickUs = (long)((Stopwatch.GetTimestamp() - tickStart) * 1_000_000.0 / Stopwatch.Frequency);
            TickCount++;
            TotalTickDurationUs += tickUs;
            if (tickUs > MaxTickDurationUs) MaxTickDurationUs = tickUs;
            if (missedSlot) LateTickCount++;

            _source.Statistics.RefreshRate(now);
        }
    }

    /// <summary>Ek tick ka kaam — tests yahin se chalate hain (baghair thread ke).</summary>
    internal void TickOnce(double dtSeconds, long nowMs)
    {
        bool fresh = _source.TryGetLatest(out var packet, out _, WatchdogMs);
        _watchdogActive = !fresh;
        // AUDIT FIX: data wapas aane par filters ki purani yaad (watchdog
        // se pehle ki _xPrev) naye sample ke sath galat milti thi — pehle
        // fresh tick par pipeline saaf halat se shuru hoti hai.
        if (fresh && !_wasFresh) _pipeline.Reset();
        _wasFresh = fresh;
        var newState = fresh ? _pipeline.Process(packet, dtSeconds) : ProcessedState.Neutral;
        lock (_stateLock) _currentState = newState;

        if (!_outputEnabled)
        {
            // AUDIT FIX (safety): output band karne par aakhri input vJoy
            // mein ATKA rehta tha (masalan ghuma hua steering game mein
            // waisa hi kharra rehta). Band karte hi EK dafa neutral bhejo —
            // "pause" ka matlab freeze nahi, mehfooz rukna hai.
            if (_output.IsConnected && !_neutralSentWhileDisabled)
            {
                _neutralSentWhileDisabled = true;
                try { _output.SendNeutral(); }
                catch { _output.Disconnect(); }
            }
            return;
        }
        _neutralSentWhileDisabled = false;

        if (!_output.IsConnected)
        {
            if (nowMs - _lastReconnectAttemptMs >= 2000)
            {
                _lastReconnectAttemptMs = nowMs;
                if (!_output.TryConnect(out var error))
                    _lastOutputError = error;
                else
                    _lastOutputError = null;
            }
            return;
        }

        try
        {
            _output.Update(CurrentState);
        }
        catch (Exception ex)
        {
            _lastOutputError = ex.Message;
            _output.Disconnect();   // agla tick dobara connect karega
        }
    }

    public void Dispose()
    {
        _disposed = true;
        Stop();
        _output.Dispose();
    }
}
