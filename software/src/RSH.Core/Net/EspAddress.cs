using System.Diagnostics.CodeAnalysis;
using System.Net;
using System.Net.Sockets;

namespace RSH.Core.Net;

/// <summary>
/// Wheel ka address (EspIp) validate karo — SEC-C-03 (URL injection).
///
/// `settings.json` disk se load hote waqt koi check nahi hota tha: koi bhi
/// string (hostname, path, <c>user@host</c>, port) seedha URL interpolation
/// mein ja kar firmware + Basic-auth password attacker host par POST kar
/// sakti thi. Is liye:
/// <list type="bullet">
/// <item>Sirf IPv4 literal qabool hai — hostname/DNS kabhi nahi.</item>
/// <item>Sirf private/link-local unicast range (wheel hamesha local
/// network — AP ya home LAN — par hota hai): 10/8, 172.16/12,
/// 192.168/16, 169.254/16. Public, loopback, multicast, reserved reject.</item>
/// <item>URL kabhi string interpolation se nahi banta — caller validated
/// IP ko <c>UriBuilder</c> mein daale (host sirf ye IP hota hai).</item>
/// </list>
///
/// D33 (Phase F, SW-14): mDNS <c>rsh.local</c> PEHLE try hota hai, fallback
/// default IP (192.168.4.1, configurable — yahan <see cref="DefaultEspIp"/>).
/// <see cref="ResolveAsync"/> wahi karta hai. SEC-C-03 yahan bhi laagu hai:
/// mDNS/DNS se mila address bhi private-range check se guzarta hai — koi
/// bhi naam (ya sandbox jaisa wildcard DNS) public/sinkhole IP par nahi
/// le ja sakta; aisa ho to fallback milta hai, attacker IP kabhi nahi.
/// </summary>
public static class EspAddress
{
    public const string DefaultEspIp = "192.168.4.1";

    /// <summary>mDNS hostname (D33): board isi naam se advertise karta hai — pehle ye try hota hai.</summary>
    public const string MdnsHostname = "rsh.local";

    /// <summary><paramref name="value"/> mDNS naam hai? (case-insensitive, aakhri dot ke saath/bagair).</summary>
    public static bool IsMdnsName(string? value)
    {
        if (string.IsNullOrWhiteSpace(value)) return false;
        string text = value.Trim();
        return string.Equals(text, MdnsHostname, StringComparison.OrdinalIgnoreCase)
            || string.Equals(text, MdnsHostname + ".", StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>
    /// D33 resolve (mDNS pehle, fallback default IP):
    /// <list type="bullet">
    /// <item>Private IPv4 literal → seedha wahi (koi DNS nahi).</item>
    /// <item><c>rsh.local</c> ya khaali → mDNS/DNS lookup, pehla IPv4 jo
    /// private-range check pass kare; na mile / ghalat range ho / lookup
    /// fail ho → fallback <see cref="DefaultEspIp"/> (UsedFallback = true).</item>
    /// <item>Koi aur hostname, public IP ya ghalat text → Ok = false, koi
    /// fallback nahi (SEC-C-03: ghalat config khamoshi se default par nahi
    /// girni chahiye — caller SanitizeOrDefault ya saaf error dikhaye).</item>
    /// </list>
    /// <paramref name="dnsLookup"/> sirf tests ke liye (fake mDNS jawab);
    /// null ho to <see cref="Dns.GetHostEntryAsync(string)"/> istemal hota hai.
    /// </summary>
    public static async Task<(bool Ok, IPAddress? Address, bool UsedFallback, string? Error)> ResolveAsync(
        string? value,
        Func<string, CancellationToken, Task<IPAddress[]?>>? dnsLookup = null,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(value) || IsMdnsName(value))
        {
            var (resolved, _) = await TryResolveMdnsAsync(dnsLookup, cancellationToken).ConfigureAwait(false);
            if (resolved is not null)
                return (true, resolved, false, null);
            _ = IPAddress.TryParse(DefaultEspIp, out var fallback);
            return (true, fallback, true, null);
        }
        if (TryValidate(value, out var literal, out _))
            return (true, literal, false, null);
        return (false, null, false,
            $"'{value?.Trim()}' valid private IPv4 ya '{MdnsHostname}' nahi hai — wheel ka address private IP ya rsh.local hona chahiye.");
    }

    /// <summary>mDNS/DNS se <c>rsh.local</c> resolve karo — pehla private IPv4, warna null.</summary>
    public static async Task<(IPAddress? Address, string? Error)> TryResolveMdnsAsync(
        Func<string, CancellationToken, Task<IPAddress[]?>>? dnsLookup = null,
        CancellationToken cancellationToken = default)
    {
        IPAddress[]? answers;
        try
        {
            if (dnsLookup is not null)
            {
                answers = await dnsLookup(MdnsHostname, cancellationToken).ConfigureAwait(false);
            }
            else
            {
                var entry = await Dns.GetHostEntryAsync(MdnsHostname, cancellationToken).ConfigureAwait(false);
                answers = entry.AddressList;
            }
        }
        catch (Exception ex) when (ex is SocketException or OperationCanceledException)
        {
            return (null, $"mDNS lookup fail hua: {ex.Message}");
        }
        if (answers is null || answers.Length == 0)
            return (null, "mDNS se koi jawab nahi mila.");
        foreach (var candidate in answers)
        {
            if (candidate.AddressFamily != AddressFamily.InterNetwork) continue;
            if (TryValidate(candidate.ToString(), out var address, out _))
                return (address, null);
        }
        return (null, "mDNS ne private local address nahi diya (jawab public/ghalat range ka tha) — fallback istemal hoga.");
    }

    public static bool TryValidate(string? value, [NotNullWhen(true)] out IPAddress? address, out string? error)
    {
        address = null;
        error = null;
        if (string.IsNullOrWhiteSpace(value))
        {
            error = "Wheel address khaali hai.";
            return false;
        }
        string text = value.Trim();
        if (!IPAddress.TryParse(text, out var parsed) || parsed.AddressFamily != AddressFamily.InterNetwork)
        {
            error = $"'{text}' valid IPv4 address nahi hai — hostname ya URL yahan nahi chalega.";
            return false;
        }
        byte[] b = parsed.GetAddressBytes();
        bool isPrivate =
            b[0] == 10 ||
            (b[0] == 172 && b[1] >= 16 && b[1] <= 31) ||
            (b[0] == 192 && b[1] == 168) ||
            (b[0] == 169 && b[1] == 254);
        if (!isPrivate)
        {
            error = $"'{text}' local network ka address nahi hai — wheel sirf local network (192.168.x / 10.x / 172.16-31.x) par hota hai.";
            return false;
        }
        address = parsed;
        return true;
    }

    /// <summary>Settings load par sanitize: ghalat EspIp ho to default par girao (SEC-C-03).</summary>
    public static string SanitizeOrDefault(string? value) =>
        TryValidate(value, out var address, out _) ? address!.ToString() : DefaultEspIp;
}
