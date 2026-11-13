using System.Diagnostics;
using RSH.Core.Output;
using RSH.Core.Processing;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Transport;

namespace RSH.Core.Pipeline;

/// <summary>
/// Poora nizam chalane wala engine: 200 Hz high-resolution loop mein
/// source se latest packet → pipeline → output. Teen mehfooz usool:
/// 1) Data 250 ms se purana ho to output NEUTRAL (watchdog).
/// 2) Output band ho ya device na mile to bhi loop chalta rehta hai —
///    UI hamesha zinda state dikhati hai.
/// 3) Output ki kharabi (exception) loop ko kabhi nahi torti; dobara
///    connect ki koshish har 2 s par hoti rehti hai.
/// 4) Process/source ki exception (misal P2 reject throw) bhi loop ko nahi
///    torti — gini jati hai (LoopErrorCount), output neutral kiya jata hai.
/// UI is se snapshot padhti hai (30 Hz par) — engine UI ko nahi bulata.
/// </summary>
public sealed class WheelEngine : IDisposable
{
    public const long WatchdogMs = 250;
    private const long TickMs = 5;
    // P4 (bounded): Stop() engine thread ka zyada se zyada itni der intezar
    // karta hai (5 x Join(1000)). Thread aam tor par ~15 ms mein khatam
    // hota hai (har tick _running check + zyada se zyada ~10 ms sleep).
    private const int StopJoinAttempts = 5;

    private readonly ISampleSource _source;
    private readonly IJoystickOutput _output;
    private readonly ProcessingPipeline _pipeline;

    private Thread? _thread;
    private volatile bool _running;
    private volatile bool _outputEnabled = true;
    private volatile bool _disposed;
    private volatile uint _latestButtonsEx;
    private bool _neutralSentWhileDisabled;
    private bool _wasFresh;
    private long _lastReconnectAttemptMs = -2000;   // pehla tick hi connect kare — 2 s intezar nahi
    private string? _lastOutputError;

    // AUDIT FIX: ye halat engine thread likhta hai aur UI thread padhta
    // hai — ProcessedState struct hai, is liye volatile ki jagah chhota
    // sa lock (200 Hz par bhi sasta) taaza aur poori qeemat ki zamanat
    // deta hai.
    private readonly object _stateLock = new();
    // P4: Start/Stop/Dispose ka lifecycle lock — _thread field aur output
    // ki zindagi isi ke neeche badalti hai. Loop thread ye lock kabhi nahi
    // leta, is liye Stop() is ke andar Join kar sakta hai (deadlock nahi).
    private readonly object _lifecycleLock = new();
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

    /// <summary>Aakhri tick ka asal waqfa, seconds mein (P5: high-res Stopwatch se).</summary>
    public double LastDtSeconds { get; private set; }

    /// <summary>Loop mein pakdi gayi exceptions (misal Process() ka throw) — engine thread ko maarne ke bajaye gini jati hain.</summary>
    public long LoopErrorCount { get; private set; }

    /// <summary>Aakhri loop exception ka message (LastOutputError jaisa pattern — UI/diagnostics ke liye).</summary>
    public string? LastLoopError { get; private set; }

    /// <summary>Stop() timeout ki wajah — thread waqt par khatam na hua ho to set hota hai, kamyab Stop() par saaf.</summary>
    public string? LastStopError { get; private set; }

    public LinkStatistics Statistics => _source.Statistics;
    public WheelProfile Profile => _pipeline.Profile;

    /// <summary>
    /// T11/Ex: taaza StatusPacketV2.BtnsEx word (0-based buttons 32..63)
    /// — 1 Hz status packet se aata hai. TickOnce har tick source
    /// (IStatusSource) se taaza value leta hai; haath se set karna
    /// sirf us waqt asar karta hai jab source IStatusSource na ho
    /// (tests / fake source). TickOnce har tick yehi value pipeline ko deta hai.
    /// </summary>
    public uint LatestButtonsEx { get => _latestButtonsEx; set => _latestButtonsEx = value; }

    /// <summary>Maujooda PipelinePlan — profile se bana ho ya SetPlan se aaya ho.</summary>
    public PipelinePlan Plan => _pipeline.Plan;

    public void SetProfile(WheelProfile profile) => _pipeline.SetProfile(profile);

    /// <summary>
    /// P11: naya plan lagao — pipeline dobara wire hoti hai (SetPlan,
    /// ProcessingPipeline). Ghalat plan throw karta hai, purana plan rehta hai.
    /// </summary>
    public void SetPlan(PipelinePlan plan) => _pipeline.SetPlan(plan);

    /// <summary>
    /// P11: Profile v2 se plan load karo — active profile lo, engine ko
    /// do (profile + us se bana plan, ModuleTunings samet).
    /// </summary>
    public void LoadPlanFromActiveProfile(ProfileFile data) =>
        SetProfile(ProfileStore.GetActive(data));

    /// <summary>
    /// P11: maujooda plan ko Profile v2 mein save karo — active profile
    /// ki ModuleTunings (tuning + on/off, har module) + legacy v1 fields
    /// (steering tuning, taake purane readers bhi sahi parhein) update
    /// karke ProfileStore.Save chalao. Koi profile kho nahi jati —
    /// sirf active profile ki tuning badalti hai.
    /// </summary>
    public void SavePlanToProfile(ProfileStore store, ProfileFile data)
    {
        ArgumentNullException.ThrowIfNull(store);
        ArgumentNullException.ThrowIfNull(data);
        var plan = _pipeline.Plan;
        var active = ProfileStore.GetActive(data);
        var steer = plan[ModuleKind.Steering].Tuning;
        var updated = active with
        {
            Deadzone = steer.Deadzone,
            CurveStrength = steer.CurveStrength,
            Smoothing = steer.Smoothing,
            InvertSteering = steer.Invert,
            SteeringScale = steer.Scale,
            InvertAccel = plan[ModuleKind.Accelerator].Tuning.Invert,
            InvertBrake = plan[ModuleKind.Brake].Tuning.Invert,
            ModuleTunings = PipelinePlanBuilder.ToModuleTunings(plan),
        };
        int idx = data.Profiles.FindIndex(p =>
            string.Equals(p.Name, data.ActiveProfileName, StringComparison.OrdinalIgnoreCase));
        if (idx >= 0) data.Profiles[idx] = updated;
        else data.Profiles.Add(updated);
        store.Save(data);
    }

    public void Start()
    {
        lock (_lifecycleLock)
        {
            if (_running || _disposed || _thread is not null) return;   // AUDIT FIX: Dispose ke baad restart disposed output ko zinda kar deta tha
            _running = true;
            _thread = new Thread(Loop) { IsBackground = true, Name = "RSH-Engine", Priority = ThreadPriority.AboveNormal };
            _thread.Start();
        }
    }

    public void Stop()
    {
        lock (_lifecycleLock)
        {
            _running = false;
            Thread? thread = _thread;
            if (thread is not null)
            {
                // P4: Join(1000) ka return ignore nahi karo — thread zinda
                // ho to output ko haath mat lagao. Thread har tick _running
                // check karta hai aur zyada se zyada ~10 ms sota hai, is
                // liye aam tor par pehla Join hi kaafi hai.
                // BOUNDED (researchers review): retry kul 5 s tak hai —
                // unbounded wait Stop() ko hamesha hang kar sakta tha.
                // Timeout par kya hota hai: Stop() wapas aa jata hai,
                // LastStopError set hota hai, _thread rakha rehta hai
                // (taake Start() naya thread na banaye aur agli Stop()
                // dobara join kar sake), aur output ko HAATH NAHI lagaya
                // jata — zinda thread ke neeche Disconnect/Dispose hi asal
                // race thi. Output dispose nahi hota (leak, crash nahi);
                // thread apni blocking call se wapas aate hi khud khatam
                // ho jayega (_running=false hai), phir agli Stop() safai
                // kar degi. Dispose() bhi timeout par output dispose nahi
                // karta (neeche dekho).
                bool exited = false;
                for (int attempt = 0; attempt < StopJoinAttempts && !exited; attempt++)
                    exited = thread.Join(1000);
                if (!exited)
                {
                    LastStopError = "Engine thread 5 s mein khatam nahi hua; output ko haath nahi lagaya gaya — dobara Stop() karo.";
                    return;
                }
                _thread = null;
            }
            LastStopError = null;
            try { if (_output.IsConnected) _output.SendNeutral(); } catch { /* band hote waqt khamoshi behtareen */ }
            _output.Disconnect();
        }
    }

    private void Loop()
    {
        var clock = Stopwatch.StartNew();
        long nextTick = clock.ElapsedMilliseconds + TickMs;
        long lastTickTimestamp = Stopwatch.GetTimestamp();

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

            long tickStart = Stopwatch.GetTimestamp();
            // P5: dt high-res Stopwatch se — ElapsedMilliseconds 1 ms
            // quantised tha (dt hamesha poora ms), jab ke filters (OneEuro)
            // ko asal waqfa chahiye. Had wahi [1 ms, 50 ms], ab seconds mein.
            double dt = Math.Clamp((tickStart - lastTickTimestamp) / (double)Stopwatch.Frequency, 0.001, 0.05);
            lastTickTimestamp = tickStart;
            LastDtSeconds = dt;
            nextTick += TickMs;

            try
            {
                TickOnce(dt, now);
                long tickUs = (long)((Stopwatch.GetTimestamp() - tickStart) * 1_000_000.0 / Stopwatch.Frequency);
                TickCount++;
                TotalTickDurationUs += tickUs;
                if (tickUs > MaxTickDurationUs) MaxTickDurationUs = tickUs;
                if (missedSlot) LateTickCount++;

                _source.Statistics.RefreshRate(now);
            }
            catch (Exception ex)
            {
                // Cybersecurity review finding (P4 hardening): Process()
                // se exception (misal P2 reject throw) engine thread ko
                // khamoshi se maar deti thi — output aakhri state par atka
                // rehta (neutral nahi), na log, na restart. Ab: gino,
                // state + output neutral karo, aur loop jari rakho.
                // TickCount sirf mukammal ticks ginta hai.
                LoopErrorCount++;
                LastLoopError = ex.Message;
                lock (_stateLock) _currentState = ProcessedState.Neutral;
                try { if (_output.IsConnected) _output.SendNeutral(); } catch { /* neutral na ja sake to bhi loop jari */ }
            }
        }
    }

    /// <summary>Ek tick ka kaam — tests yahin se chalate hain (baghair thread ke).</summary>
    internal void TickOnce(double dtSeconds, long nowMs)
    {
        // Ex listener (Phase D): status packet se BtnsEx har tick source
        // se lo — 2 s se purana status ho to buttons saaf (stale Ex bits
        // kabhi atke nahi rehte). Source IStatusSource na ho (fake source)
        // to haath se set ki hui LatestButtonsEx rehti hai (tests).
        if (_source is IStatusSource statusSource)
        {
            _latestButtonsEx = statusSource.LatestButtonsExAgeMs is < 2000
                ? statusSource.LatestButtonsEx
                : 0;
        }
        // v2.1 stream (D6: sab axes mV) taaza ho to wahi rasta — warna v1.
        var v2packet = default(WheelPacketV2);
        var packet = default(WheelPacket);
        bool v2Fresh = _source is IV2SampleSource v2Source
            && v2Source.TryGetLatestV2(out v2packet, out _, WatchdogMs);
        bool fresh = v2Fresh || _source.TryGetLatest(out packet, out _, WatchdogMs);
        _watchdogActive = !fresh;
        // AUDIT FIX: data wapas aane par filters ki purani yaad (watchdog
        // se pehle ki _xPrev) naye sample ke sath galat milti thi — pehle
        // fresh tick par pipeline saaf halat se shuru hoti hai.
        if (fresh && !_wasFresh) _pipeline.Reset();
        _wasFresh = fresh;
        var newState = !fresh ? ProcessedState.Neutral
            : v2Fresh ? _pipeline.ProcessV2(v2packet, dtSeconds, _latestButtonsEx)
            : _pipeline.Process(packet, dtSeconds, _latestButtonsEx);
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
        lock (_lifecycleLock)
        {
            if (_disposed) return;   // dobara Dispose: output dobara dispose nahi hota
            _disposed = true;
            Stop();   // lock re-entrant hai — thread reape hone ke baad hi output dispose hota hai (P4)
            // P4 (bounded Stop): Stop() timeout par _thread rakha rehta
            // hai aur output ko haath nahi lagata — yahan bhi dispose
            // sirf tab jab thread waqai khatam ho, warna zinda thread ke
            // neeche dispose race hoti. (Timeout wala output leak hota
            // hai, crash nahi — agli Stop() thread marne ke baad
            // Disconnect kar degi.)
            if (_thread is null) _output.Dispose();
        }
    }
}
