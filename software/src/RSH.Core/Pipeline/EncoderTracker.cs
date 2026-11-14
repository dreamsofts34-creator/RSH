namespace RSH.Core.Pipeline;

/// <summary>
/// Cumulative i16 encoder counts → per-tick delta (Phase F WS5, D1/D6).
/// <para>
/// Firmware cumulative counts bhejta hai (i16, wrap allowed); PC delta
/// wrap-aware nikalta hai (<c>(short)(now - prev)</c>). Do kaam:
/// </para>
/// <list type="bullet">
/// <item><b>Wrap:</b> 32767 → -32768 ka jump +1 delta hai, -32768 ka jump nahi.</item>
/// <item><b>Glitch filter:</b> ek tick (5 ms) mein |delta| &gt;
/// <see cref="MaxPlausibleDeltaPerTick"/> namumkin hai — haath se ghumaya
/// gaya knob 80 counts/rev par bhi 80 rev/s se zyada nahi ghumta
/// (32/tick = 6400/s). Aisa jump wire glitch hai (virtual wheel ka
/// EncoderGlitch ±50..200 jump jaisa): output delta 0 hoti hai,
/// <see cref="GlitchesFiltered"/> barhta hai, aur baseline nayi value
/// par aage barh jati hai (warna wahi jump har tick dobara dikhta aur
/// encoder hamesha ke liye freeze ho jata). ±1 spurious steps is filter
/// se neeche hain — asal single detent step se farq nahi kiya ja sakta,
/// wo bounded (±1) rehte hain; A/B bounce firmware taraf
/// <c>rsh_encoder_step</c> ki reversal-cancellation se filter hota hai.</item>
/// </list>
/// Pehla packet baseline banata hai (delta 0). <see cref="Reset"/>
/// (watchdog resume / plan change) baseline dobara banata hai — purani
/// value se naya packet jorne par jhoota bara delta nahi banta.
/// Allocation-free hai (koi array allocate nahi hoti — 0 B/tick SLO).
/// </summary>
public sealed class EncoderTracker
{
    /// <summary>Ek tick mein mumkin zyada se zyada asal delta (counts).</summary>
    public const int MaxPlausibleDeltaPerTick = 32;

    public const int MaxEncoders = 4;

    private readonly short[] _last = new short[MaxEncoders];
    private bool _hasBaseline;

    /// <summary>Ab tak filter hue glitch jumps (cumulative).</summary>
    public long GlitchesFiltered { get; private set; }

    /// <summary>
    /// Cumulative counts (nEnc tak) se per-tick deltas nikalo.
    /// <paramref name="deltas"/> caller ka buffer hai (kam az kam 4 short) —
    /// allocate nahi hota. EncoderCount 0 ho to deltas 0 aur baseline
    /// reset (dobara enable par jump nahi).
    /// </summary>
    /// <summary>
    /// DATA-AI FIX (Round 2 W7-3): cumulative span chhoti ho (caller ne
    /// encoderCount se kam values di hon) to IndexOutOfRange ke bajaye
    /// maujooda values tak clamp karo — pipeline tick kabhi crash nahi
    /// karta. Packet path par ye kabhi nahi hota (WheelPacketV2 parse
    /// Encoders ki lambai = EncoderCount pakka karta hai); ye sirf
    /// defense in depth hai. n == 0 (khaali span) par baseline nahi
    /// banti — agla poora packet baseline banayega.
    /// </summary>
    public void Update(ReadOnlySpan<short> cumulative, int encoderCount, Span<short> deltas)
    {
        if (deltas.Length < MaxEncoders)
            throw new ArgumentException("deltas buffer kam az kam 4 short ka ho.", nameof(deltas));
        deltas.Clear();
        if (encoderCount <= 0)
        {
            _hasBaseline = false;
            return;
        }
        int n = Math.Min(encoderCount, MaxEncoders);
        n = Math.Min(n, cumulative.Length);
        if (n <= 0) return;
        if (!_hasBaseline)
        {
            cumulative[..n].CopyTo(_last.AsSpan(0, n));
            _hasBaseline = true;
            return;
        }
        for (int i = 0; i < n; i++)
        {
            short delta = unchecked((short)(cumulative[i] - _last[i]));
            _last[i] = cumulative[i];
            if (delta > MaxPlausibleDeltaPerTick || delta < -MaxPlausibleDeltaPerTick)
            {
                GlitchesFiltered++;
                delta = 0;
            }
            deltas[i] = delta;
        }
    }

    public void Reset()
    {
        _hasBaseline = false;
        Array.Clear(_last);
    }
}
