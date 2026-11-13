namespace RSH.Core.Utilities;

/// <summary>
/// 400 ms debounce (U-P4 / A8): har keystroke par save banned hai.
/// <para>
/// Har <see cref="Debounce"/> call pending action ko badal deti hai aur
/// timer dobara shuru hota hai — action sirf tab chalta hai jab
/// <see cref="Delay"/> tak koi nayi call na aaye. Callback ThreadPool par
/// chalta hai (UI thread par kabhi I/O nahi — U-P1); caller ko UI update
/// khud dispatcher par marshall karna hai.
/// </para>
/// </summary>
public sealed class Debouncer : IDisposable
{
    private readonly TimeSpan _delay;
    private readonly object _gate = new();
    private readonly Timer _timer;
    private Action? _pending;
    private bool _disposed;

    public Debouncer(TimeSpan delay)
    {
        _delay = delay;
        _timer = new Timer(OnTimer, null, Timeout.InfiniteTimeSpan, Timeout.InfiniteTimeSpan);
    }

    /// <summary>Action schedule karo — pehle wali pending action (agar ho) replace ho jati hai.</summary>
    public void Debounce(Action action)
    {
        ArgumentNullException.ThrowIfNull(action);
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            _pending = action;
            _timer.Change(_delay, Timeout.InfiniteTimeSpan);
        }
    }

    /// <summary>Pending action (agar ho) foran chalao — timer ka intezar nahi.</summary>
    public void Flush()
    {
        Action? action;
        lock (_gate)
        {
            action = _pending;
            _pending = null;
            _timer.Change(Timeout.InfiniteTimeSpan, Timeout.InfiniteTimeSpan);
        }
        action?.Invoke();
    }

    /// <summary>Koi action pending hai?</summary>
    public bool HasPending
    {
        get { lock (_gate) return _pending is not null; }
    }

    private void OnTimer(object? _)
    {
        Action? action;
        lock (_gate)
        {
            action = _pending;
            _pending = null;
        }
        // Timer callback ke andar lock nahi — action dobara Debounce kar sakta hai.
        try { action?.Invoke(); }
        catch { /* debounce action ka exception timer thread ko nahi maarna chahiye; caller log kare */ }
    }

    public void Dispose()
    {
        lock (_gate)
        {
            if (_disposed) return;
            _disposed = true;
            _pending = null;
        }
        _timer.Dispose();
    }
}
