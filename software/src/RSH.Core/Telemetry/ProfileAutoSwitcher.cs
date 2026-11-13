using RSH.Core.Profiles;

namespace RSH.Core.Telemetry;

/// <summary>
/// Auto-switch ka faisla (TELEMETRY-READER.md §3 rules 1/4/5):
/// switch sirf tab jab snapshot <see cref="GameTelemetrySnapshot.IsSwitchable"/>
/// ho (OnTrack + car ka naam), resolver ko profile mile, aur woh
/// profile abhi active na ho. <b>Manual override jeetta hai</b> — user
/// ne haath se profile chuni ho to <see cref="ManualOverride"/> on rehta
/// hai aur koi auto-switch nahi hota jab tak user dobara "Auto" na kare.
/// <para>
/// Ye class sirf faisla karti hai (pure — koi I/O nahi): asal switch
/// (active naam badalna + save + engine apply) AppServices ka kaam hai.
/// Is se <c>ManualOverrideBlocksAutoSwitch</c> jaisi tests Core mein
/// chalti hain, WPF ke baghair.
/// </para>
/// </summary>
public sealed class ProfileAutoSwitcher
{
    /// <summary>User ne haath se profile chuni hai — auto-switch band hai jab tak ye false na ho.</summary>
    public bool ManualOverride { get; set; }

    /// <summary>
    /// Snapshot par switch hona chahiye? Haan to woh profile (caller
    /// active kare), warna null — wajah <paramref name="reason"/> mein.
    /// </summary>
    public WheelProfile? TrySwitch(
        GameTelemetrySnapshot? snap,
        IReadOnlyList<WheelProfile> profiles,
        string currentProfileName,
        out string reason)
    {
        reason = "";
        if (ManualOverride)
        {
            reason = "Manual override is on — auto-switch is paused until you resume Auto.";
            return null;
        }
        if (snap is null || !snap.IsSwitchable)
        {
            reason = "Not on track with a known car — no switch.";
            return null;
        }
        var resolved = CarProfileResolver.Resolve(snap, profiles);
        if (resolved is null)
        {
            reason = $"No profile for '{snap.CarModel}' — keeping the current profile.";
            return null;
        }
        if (string.Equals(resolved.Name, currentProfileName, StringComparison.OrdinalIgnoreCase))
        {
            reason = $"Profile '{resolved.Name}' is already active.";
            return null;
        }
        reason = $"Profile: {resolved.Name} (auto)";
        return resolved;
    }
}
