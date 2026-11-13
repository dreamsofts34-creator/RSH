// Ping rate limiter (SEC-C-21, High) — board-side rule ka reference implementation.
//
// Board (firmware, Phase D) har source ke liye token bucket rakhega:
//   max 10 replies/s, burst 5; limit se zyada ping KHAMOSHI se drop
//   (koi error reply nahi — error reply bhi amplification hai).
// Ye C# class usi rule ki host-side copy hai (wahi numbers, wahi faisle),
// taake rule unit test ho sake aur virtual wheel + future firmware-mirror
// is se match karein. Firmware mein ye fixed-size table hogi (koi heap
// allocation nahi, koi per-ping state store nahi); yahan dictionary hai
// lekin tracked sources ki tadad bounded hai (MaxTrackedSources) taake
// attacker sources ki list ko bhi flood na kar sake.
//
// Key = source IP address (port nahi — ek client ke port badal sakte hain,
// aur reflector rokne ke liye IP hi asal pehchan hai).
// Reply hamesha ≤ request size hai (dono 34 B, fixed) — SEC-C-22.
using System.Diagnostics;

namespace RSH.Core.Transport;

/// <summary>Per-source token bucket (SEC-C-21): max 10 replies/s, burst 5, silent drop.</summary>
public sealed class PingRateLimiter
{
    /// <summary>Har source ko kitne replies/s mil sakte hain (sustained).</summary>
    public const double MaxRepliesPerSecond = 10;
    /// <summary>Ek saath kitne replies (burst) — bucket ki capacity.</summary>
    public const double BurstCapacity = 5;
    /// <summary>Ek waqt mein kitne sources track hote hain (bounded memory — is se zyada hon to sab se purana source nikaal kar naya rakha jata hai).</summary>
    public const int MaxTrackedSources = 64;

    private sealed class Bucket
    {
        public double Tokens = BurstCapacity;
        public long LastRefillTicks;
        public long LastSeenTicks;
    }

    private readonly object _lock = new();
    private readonly Dictionary<string, Bucket> _buckets = new(StringComparer.Ordinal);
    private readonly Func<long> _nowTicks;

    /// <summary>Asal waqt (Stopwatch ticks) ke saath limiter banao.</summary>
    public PingRateLimiter() : this(() => Stopwatch.GetTimestamp())
    {
    }

    /// <summary>Tests ke liye: waqt inject karo (ticks — Stopwatch.Frequency ke mutabiq).</summary>
    internal PingRateLimiter(Func<long> nowTicks)
    {
        _nowTicks = nowTicks;
    }

    /// <summary>Is source ko abhi reply dena hai? True = token mila (reply bhejo); false = limit — KHAMOSHI se drop karo, koi error reply nahi.</summary>
    public bool TryAcquire(string sourceKey)
    {
        if (string.IsNullOrEmpty(sourceKey)) return false;
        long now = _nowTicks();
        lock (_lock)
        {
            if (!_buckets.TryGetValue(sourceKey, out var bucket))
            {
                if (_buckets.Count >= MaxTrackedSources)
                {
                    // Bounded: sab se purana (sab se der se dekha gaya) source nikaalo.
                    string? oldest = null;
                    long oldestSeen = long.MaxValue;
                    foreach (var kv in _buckets)
                    {
                        if (kv.Value.LastSeenTicks < oldestSeen)
                        {
                            oldestSeen = kv.Value.LastSeenTicks;
                            oldest = kv.Key;
                        }
                    }
                    if (oldest is not null) _buckets.Remove(oldest);
                }
                bucket = new Bucket { LastRefillTicks = now, LastSeenTicks = now };
                _buckets[sourceKey] = bucket;
            }

            double elapsedSec = (now - bucket.LastRefillTicks) / (double)Stopwatch.Frequency;
            if (elapsedSec > 0)
            {
                bucket.Tokens = Math.Min(BurstCapacity, bucket.Tokens + elapsedSec * MaxRepliesPerSecond);
                bucket.LastRefillTicks = now;
            }
            bucket.LastSeenTicks = now;

            if (bucket.Tokens < 1) return false;
            bucket.Tokens -= 1;
            return true;
        }
    }

    /// <summary>Abhi kitne sources track ho rahe hain (≤ MaxTrackedSources hamesha).</summary>
    public int SourcesTracked { get { lock (_lock) return _buckets.Count; } }

    /// <summary>Sab bhool jao (tests / restart).</summary>
    public void Reset()
    {
        lock (_lock) _buckets.Clear();
    }
}
