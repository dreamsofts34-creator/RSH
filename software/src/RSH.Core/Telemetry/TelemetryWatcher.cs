namespace RSH.Core.Telemetry;

/// <summary>
/// Poll loop (TELEMETRY-READER.md §4): 2 Hz par readers se identity
/// parho, <see cref="SnapshotChanged"/> sirf tab jab
/// (Game, CarModel, State) badle — aur sirf tab jab wahi snapshot
/// <b>2 s tak stable</b> rahe (4 musalsal polls @ 2 Hz), taake loading
/// ke dauraan aadhi identity par switch na ho.
/// <para>
/// Game band ho ya map ghayab ho to koi event nahi (candidate reset —
/// dobara aane par debounce dobara shuru hota hai). Khaali CarModel wala
/// snapshot bhi candidate nahi banta (identity aadhi hai).
/// <see cref="PollOnce"/> public hai taake tests timer ke baghair
/// deterministic tareeqe se debounce check kar sakein
/// (<see cref="TimeProvider"/> inject hota hai).
/// </para>
/// </summary>
public sealed class TelemetryWatcher : IDisposable
{
    public static readonly TimeSpan DefaultPollInterval = TimeSpan.FromMilliseconds(500);
    public static readonly TimeSpan DefaultStableDuration = TimeSpan.FromSeconds(2);

    private readonly IReadOnlyList<ITelemetryReader> _readers;
    private readonly TimeProvider _time;
    private readonly TimeSpan _stableDuration;
    private readonly object _gate = new();
    private Timer? _timer;
    private bool _disposed;

    private (GameId Game, string CarModel, SessionState State) _candidateKey;
    private GameTelemetrySnapshot? _candidate;
    private DateTimeOffset _candidateSince;
    private (GameId Game, string CarModel, SessionState State) _lastEmittedKey;
    private bool _hasEmitted;

    public TelemetryWatcher(
        IEnumerable<ITelemetryReader> readers,
        TimeProvider? timeProvider = null,
        TimeSpan? pollInterval = null,
        TimeSpan? stableDuration = null)
    {
        ArgumentNullException.ThrowIfNull(readers);
        _readers = readers.ToList();
        _time = timeProvider ?? TimeProvider.System;
        PollInterval = pollInterval ?? DefaultPollInterval;
        _stableDuration = stableDuration ?? DefaultStableDuration;
    }

    public TimeSpan PollInterval { get; }

    /// <summary>Stable snapshot badla — switcher/resolver yahin sunta hai.</summary>
    public event Action<GameTelemetrySnapshot>? SnapshotChanged;

    /// <summary>Aakhri raw snapshot (kisi bhi reader se — "current car" button ke liye).</summary>
    public GameTelemetrySnapshot? LatestRawSnapshot { get; private set; }

    /// <summary>Aakhri stable (emit hua) snapshot — null = abhi koi stable identity nahi.</summary>
    public GameTelemetrySnapshot? LatestStableSnapshot { get; private set; }

    public bool IsRunning { get; private set; }

    public void Start()
    {
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            if (IsRunning) return;
            IsRunning = true;
            _timer = new Timer(static state => ((TelemetryWatcher)state!).PollOnce(),
                this, PollInterval, PollInterval);
        }
    }

    public void Stop()
    {
        lock (_gate)
        {
            IsRunning = false;
            _timer?.Dispose();
            _timer = null;
        }
    }

    /// <summary>
    /// Ek poll: pehla reader jo snapshot de, usay debounce se guzaro.
    /// Timer bhi yahi bulata hai; tests seedha bulate hain.
    /// Kabhi throw nahi karta (reader contract + yahan ka try/catch).
    /// </summary>
    public void PollOnce()
    {
        GameTelemetrySnapshot? snap = null;
        try
        {
            foreach (var reader in _readers)
            {
                try
                {
                    if (reader.TryRead(out var s))
                    {
                        snap = s;
                        break;
                    }
                }
                catch (Exception) { /* reader contract ke khilaf throw — agla reader */ }
            }
        }
        catch (Exception) { /* poll kabhi timer thread ko nahi maarta */ }

        DateTimeOffset now = _time.GetUtcNow();
        GameTelemetrySnapshot? emit = null;
        lock (_gate)
        {
            if (snap is not null) LatestRawSnapshot = snap;
            if (snap is null || snap.CarModel.Length == 0)
            {
                // Game band / identity aadhi — candidate reset.
                _candidate = null;
                return;
            }
            var key = (snap.Game, snap.CarModel, snap.State);
            if (_candidate is null || _candidateKey != key)
            {
                _candidate = snap;
                _candidateKey = key;
                _candidateSince = now;
                return;
            }
            if (now - _candidateSince < _stableDuration) return;
            if (_hasEmitted && _lastEmittedKey == key) return; // pehle hi emit ho chuka
            _lastEmittedKey = key;
            _hasEmitted = true;
            LatestStableSnapshot = snap;
            emit = snap;
        }
        if (emit is not null)
        {
            try { SnapshotChanged?.Invoke(emit); }
            catch (Exception) { /* subscriber ka exception watcher ko nahi maarta */ }
        }
    }

    public void Dispose()
    {
        lock (_gate)
        {
            if (_disposed) return;
            _disposed = true;
            IsRunning = false;
            _timer?.Dispose();
            _timer = null;
        }
    }
}
