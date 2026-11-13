using RSH.Core.Profiles;

namespace RSH.Core.Telemetry;

/// <summary>
/// Car → profile resolve (TELEMETRY-READER.md §3 rule 3 / §4):
/// pehle <b>car-specific</b> profile (Game + CarModel), phir <b>game</b>
/// profile (aaj ka <c>GameProcessName</c> mechanism), phir <c>null</c>
/// (caller maujooda profile rakhta hai — switch nahi hota).
/// <para>
/// Match normalized strings par hota hai (separator-insensitive key —
/// <see cref="TelemetryNormalization.NormalizeCarKey"/>). Ek car ke
/// liye do profiles hon to naam ke hisaab se pehli milti hai (aur
/// <see cref="FindDuplicateCarProfiles"/> UI warning ke liye baqi deta hai).
/// Calibration kabhi resolve nahi hoti — woh board-bound hai (D21) aur
/// profile switch sirf tuning badalta hai.
/// </para>
/// </summary>
public static class CarProfileResolver
{
    /// <summary>Game → GameLauncher catalogue wale process naam (Core mein App ka catalogue nahi aa sakta — yehi mapping authority hai).</summary>
    public static IReadOnlyList<string> ProcessNamesFor(GameId game) => game switch
    {
        GameId.AssettoCorsa => ["AssettoCorsa", "acs"],
        GameId.Acc => ["acc"],
        GameId.IRacing => ["iRacingSim64DX11"],
        _ => [],
    };

    /// <summary>
    /// Snapshot → profile: car-specific → game → null. Snapshot khaali
    /// car ya Unknown game ho to null. Kabhi throw nahi karta.
    /// </summary>
    public static WheelProfile? Resolve(GameTelemetrySnapshot? snap, IReadOnlyList<WheelProfile> profiles)
    {
        try
        {
            if (snap is null || snap.CarModel.Length == 0 || snap.Game == GameId.Unknown) return null;
            if (profiles.Count == 0) return null;
            string carKey = TelemetryNormalization.NormalizeCarKey(snap.CarModel);
            var ordered = profiles.OrderBy(p => p.Name, StringComparer.OrdinalIgnoreCase).ToList();

            // 1) Car-specific: CarModel mile + Game mile (ya profile kisi bhi game ki ho).
            foreach (var p in ordered)
            {
                if (p.CarModel.Length == 0) continue;
                if (!string.Equals(TelemetryNormalization.NormalizeCarKey(p.CarModel), carKey, StringComparison.Ordinal)) continue;
                if (p.Game != GameId.Unknown && p.Game != snap.Game) continue;
                return p;
            }

            // 2) Game profile: CarModel khaali ho (kisi bhi car par) + Game ya GameProcessName mile.
            var processNames = ProcessNamesFor(snap.Game);
            foreach (var p in ordered)
            {
                if (p.CarModel.Length > 0) continue; // car-specific profile kisi aur car ki hai — game fallback nahi
                bool gameMatch = (p.Game != GameId.Unknown && p.Game == snap.Game) ||
                    (p.GameProcessName.Length > 0 &&
                     processNames.Any(n => string.Equals(n, p.GameProcessName, StringComparison.OrdinalIgnoreCase)));
                if (gameMatch) return p;
            }
            return null;
        }
        catch (Exception)
        {
            return null;
        }
    }

    /// <summary>
    /// Ek hi (Game, CarModel) ke liye ek se zyada profiles — pehli ke
    /// ilawa baqi (naam ke hisaab se) — UI in par warning dikhata hai.
    /// </summary>
    public static IReadOnlyList<WheelProfile> FindDuplicateCarProfiles(IReadOnlyList<WheelProfile> profiles)
    {
        var extras = new List<WheelProfile>();
        try
        {
            var groups = profiles
                .Where(p => p.CarModel.Length > 0)
                .GroupBy(p => (p.Game, Key: TelemetryNormalization.NormalizeCarKey(p.CarModel)));
            foreach (var g in groups)
            {
                var ordered = g.OrderBy(p => p.Name, StringComparer.OrdinalIgnoreCase).ToList();
                if (ordered.Count > 1) extras.AddRange(ordered.Skip(1));
            }
        }
        catch (Exception) { /* duplicate check kabhi caller ko nahi torta */ }
        return extras;
    }
}
