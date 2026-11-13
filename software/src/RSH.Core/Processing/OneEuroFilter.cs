namespace RSH.Core.Processing;

/// <summary>
/// One Euro filter (Casiez et al., CHI 2012) — reference implementation
/// ke usool par. Susti par jitter khatam, tezi par lag nahi, kyunke
/// cutoff signal ki raftaar (derivative) ke sath khud barhta hai.
///
/// Parameters: freq = sample rate (yahan 200 Hz pipeline),
/// mincutoff = bunyadi cutoff Hz (kam = zyada smooth, zyada lag),
/// beta = raftaar par cutoff kitna barhe (author ki misaal 0.1),
/// dcutoff = derivative ki smoothing (aam tor par 1.0).
/// Pehla sample seedha guzarta hai.
/// </summary>
public sealed class OneEuroFilter
{
    private readonly double _freq;
    private readonly double _minCutoff;
    private readonly double _beta;
    private readonly double _dCutoff;

    private bool _initialized;
    private double _xPrev;
    private double _dxPrev;

    public OneEuroFilter(double freq, double minCutoff, double beta, double dCutoff = 1.0)
    {
        // T14 guard: freq=0 C# mein crash NAHI karta — 1/0 = +Infinity deta
        // hai aur filter degenerate ho jata hai (alpha NaN/Infinity, output
        // beqarar). Ghalat parameters ab banne se pehle hi reject hote hain
        // taake aisi halat kabhi chalti hui pipeline mein na pahunche.
        if (!double.IsFinite(freq) || freq <= 0)
            throw new ArgumentOutOfRangeException(nameof(freq), freq, "Sample rate (Hz) lazmi finite aur > 0 hai.");
        if (!double.IsFinite(minCutoff) || minCutoff <= 0)
            throw new ArgumentOutOfRangeException(nameof(minCutoff), minCutoff, "mincutoff lazmi finite aur > 0 hai.");
        if (!double.IsFinite(beta) || beta < 0)
            throw new ArgumentOutOfRangeException(nameof(beta), beta, "beta lazmi finite aur >= 0 hai.");
        if (!double.IsFinite(dCutoff) || dCutoff <= 0)
            throw new ArgumentOutOfRangeException(nameof(dCutoff), dCutoff, "dcutoff lazmi finite aur > 0 hai.");
        _freq = freq;
        _minCutoff = minCutoff;
        _beta = beta;
        _dCutoff = dCutoff;
    }

    private static double Alpha(double cutoff, double freq)
    {
        double tau = 1.0 / (2.0 * Math.PI * cutoff);
        double te = 1.0 / freq;
        return 1.0 / (1.0 + tau / te);
    }

    public double Filter(double x) => Filter(x, 1.0 / _freq);

    /// <summary>
    /// Asal waqfe (dt) ke sath wala rasta — AUDIT FIX: pehle filter hamesha
    /// 200 Hz farz karta tha, lekin engine ka asal dt load ke sath badalta
    /// hai (1–50 ms clamp), to smoothing ki sakhti system load ke sath
    /// badal jati thi. Ab alpha har dafa asal dt se banta hai.
    /// </summary>
    public double Filter(double x, double dtSeconds)
    {
        // T14 guard: NaN/Infinity dt clamp se guzar jata tha (NaN comparisons
        // false hote hain) aur filter ko NaN kar deta tha. Na-mumkin dt ko
        // nominal 5 ms mana jata hai — crash nahi, degenerate bhi nahi.
        double dt = double.IsFinite(dtSeconds) ? Math.Clamp(dtSeconds, 0.0005, 0.1) : 0.005;
        double freq = 1.0 / dt;
        if (!_initialized)
        {
            _initialized = true;
            _xPrev = x;
            _dxPrev = 0;
            return x;
        }

        double dx = (x - _xPrev) / dt;
        double aD = Alpha(_dCutoff, freq);
        double dxHat = aD * dx + (1 - aD) * _dxPrev;

        double cutoff = _minCutoff + _beta * Math.Abs(dxHat);
        double a = Alpha(cutoff, freq);
        double xHat = a * x + (1 - a) * _xPrev;

        _xPrev = xHat;
        _dxPrev = dxHat;
        return xHat;
    }

    public void Reset()
    {
        _initialized = false;
        _xPrev = 0;
        _dxPrev = 0;
    }
}
