namespace RSH.Core.Telemetry;

/// <summary>Kaunsa game telemetry de raha hai (TELEMETRY-READER.md §1 — Phase H v1 scope).</summary>
public enum GameId
{
    Unknown = 0,
    AssettoCorsa = 1,
    Acc = 2,
    IRacing = 3,
}

/// <summary>Game session ki halat — switch sirf <see cref="OnTrack"/> par hota hai (§3 rule 1).</summary>
public enum SessionState
{
    Unknown = 0,
    Menu = 1,
    Loading = 2,
    OnTrack = 3,
    Replay = 4,
    Paused = 5,
}

/// <summary>
/// Reader ki aakhri koshish ka nateeja (diagnostics / UI — `TryRead`
/// ka bool sirf kamyabi batata hai, ye wajah batata hai).
/// </summary>
public enum ReaderStatus
{
    /// <summary>Abhi koi koshish nahi hui.</summary>
    NeverRead = 0,
    /// <summary>Snapshot kamyab parha gaya.</summary>
    Ok = 1,
    /// <summary>Game ka process nahi chal raha (map dhoondne se pehle hi gate).</summary>
    GameNotRunning = 2,
    /// <summary>Process chal raha hai lekin map nahi mila (menu / session load nahi hua).</summary>
    MapNotFound = 3,
    /// <summary>Map mila lekin size/version documented layout se nahi milta — ghalat offset se kabhi nahi parha gaya.</summary>
    UnsupportedVersion = 4,
    /// <summary>Map khula lekin bytes na-qabil-e-parh (khaali smVersion / offset bahir / aadhi YAML).</summary>
    Corrupt = 5,
}

/// <summary>
/// Car-model strings ki normalization (TELEMETRY-READER.md §3/§5):
/// snapshot mein sirf trim + lowercase (plan ki exact shakal); resolver
/// key mein `_`/space/`-` bhi ek jaise mane jate hain (AC mein
/// `ferrari_488_gt3`, ACC mein `Ferrari 488 GT3 Evo` jaisi shaklein).
/// </summary>
public static class TelemetryNormalization
{
    /// <summary>Snapshot/profile storage ke liye: trim + lowercase (kuch aur nahi).</summary>
    public static string NormalizeCarModel(string? value) =>
        string.IsNullOrWhiteSpace(value) ? "" : value.Trim().ToLowerInvariant();

    /// <summary>
    /// Resolver match key: lowercase, `_`/`-` → space, musalsal spaces ek.
    /// Is se `ferrari_488_gt3`, `Ferrari 488 GT3` aur `ferrari-488-gt3` ek hi key hain.
    /// </summary>
    public static string NormalizeCarKey(string? value)
    {
        if (string.IsNullOrWhiteSpace(value)) return "";
        Span<char> buf = stackalloc char[value.Length];
        int n = 0;
        bool lastSpace = true; // shuru ki space bhi nigal jao (trim jaisa)
        foreach (char ch in value)
        {
            char c = ch is '_' or '-' ? ' ' : char.ToLowerInvariant(ch);
            if (c == ' ')
            {
                if (!lastSpace) { buf[n++] = ' '; lastSpace = true; }
            }
            else
            {
                buf[n++] = c;
                lastSpace = false;
            }
        }
        if (n > 0 && buf[n - 1] == ' ') n--; // aakhri space
        return new string(buf[..n]);
    }
}

/// <summary>
/// Ek poll ka nateeja: kaunsi game, kaunsi car, kis halat mein.
/// <see cref="CarModel"/> normalized hota hai (trim + lowercase) — yehi
/// profile switch ki key hai. Physics fields v1 mein NAHI parhe jate
/// (§2 rule 2 — sirf identity, 2 Hz).
/// </summary>
public sealed record GameTelemetrySnapshot
{
    public GameTelemetrySnapshot(GameId game, string carModel, string trackName, SessionState state, DateTimeOffset seenAt)
    {
        Game = game;
        CarModel = TelemetryNormalization.NormalizeCarModel(carModel);
        TrackName = (trackName ?? "").Trim();
        State = state;
        SeenAt = seenAt;
    }

    public GameId Game { get; init; }
    public string CarModel { get; init; }
    public string TrackName { get; init; }
    public SessionState State { get; init; }
    public DateTimeOffset SeenAt { get; init; }

    /// <summary>Switch ke qabil snapshot? Car ka naam ho aur gaari track par ho (§3 rule 1).</summary>
    public bool IsSwitchable => State == SessionState.OnTrack && CarModel.Length > 0;
}
