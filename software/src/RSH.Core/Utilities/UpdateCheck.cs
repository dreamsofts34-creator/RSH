// RSH — A13 update mechanism + runtime check (Phase H, SW-16 / SW-11).
// Plan v2.3 §5 Phase H + §6: A13 sirf Phase H mein hai — in-app update
// check, .NET runtime check, aur USER-GUIDE version = release version.
//
// Design:
//  - UpdateChecker: GitHub Releases "latest" endpoint se tag parhta hai
//    (https://api.github.com/repos/dreamsofts34-creator/RSH/releases/latest)
//    aur current assembly version se compare karta hai. Koi auto-download
//    ya auto-install NAHI — user ko sirf bataya jata hai ke naya version
//    hai; install hamesha user khud release page / MSIX se karta hai.
//    Offline ho, API fail ho, ya tag parse na ho to result "unknown" hai
//    (Error field mein wajah) — app kabhi crash nahi karti, update check
//    kabhi startup nahi rokti (caller background par chalata hai).
//  - RuntimeCheck: .NET runtime >= 10 aur Windows 11 (build >= 22000)
//    — plan §6 floor: Windows 11. Sandbox/Linux par IsSupportedOs()
//    false deta hai (koi exception nahi) — tests isi liye platform-aware
//    hain, "Windows 11 chahiye" ka daawa Linux par nahi karte.
using System.Net.Http.Headers;
using System.Text.Json;

namespace RSH.Core.Utilities;

/// <summary>.NET runtime + OS floor check (A13, plan §6: Windows 11 floor, .NET 10 LTS).</summary>
public static class RuntimeCheck
{
    /// <summary>Minimum .NET runtime major version (plan: .NET 10 LTS).</summary>
    public const int MinimumDotNetMajor = 10;

    /// <summary>Minimum Windows 11 build (21H2 = 22000).</summary>
    public const int MinimumWindows11Build = 22000;

    /// <summary>Chal raha runtime supported hai? (.NET major >= 10.)</summary>
    public static bool IsSupportedRuntime() => Environment.Version.Major >= MinimumDotNetMajor;

    /// <summary>OS Windows 11 ya naya hai? (Non-Windows par false — koi exception nahi.)</summary>
    public static bool IsSupportedOs()
    {
        if (!OperatingSystem.IsWindows()) return false;
        return OperatingSystem.IsWindowsVersionAtLeast(10, 0, MinimumWindows11Build);
    }

    /// <summary>Dono checks ek saath — app start par About/diagnostics mein dikhane ke liye.</summary>
    public static (bool RuntimeOk, bool OsOk) CheckAll() => (IsSupportedRuntime(), IsSupportedOs());

    /// <summary>Insani-readable maujooda environment (logs/About ke liye).</summary>
    public static string DescribeCurrent() =>
        $".NET {Environment.Version} · OS {Environment.OSVersion} · {(OperatingSystem.IsWindows() ? "Windows" : "non-Windows")}";
}

/// <summary>Update check ka nateeja — Available sirf tab true jab latest version pakka naya ho.</summary>
public sealed record UpdateCheckResult(
    bool Available,
    Version? LatestVersion,
    Version CurrentVersion,
    string? Error,
    DateTimeOffset CheckedAt)
{
    /// <summary>Check hi nahi ho saka (offline / API error / tag parse fail) — Available false, Error mein wajah.</summary>
    public bool Unknown => LatestVersion is null && Error is not null;
}

/// <summary>
/// In-app update check (A13). GitHub Releases "latest" se tag parhta hai.
/// Pure parts (ParseLatestTag / IsNewer) unit-testable hain; network wala
/// hissa HttpClient leta hai taake tests fake handler de sakein.
/// </summary>
public static class UpdateChecker
{
    /// <summary>GitHub API endpoint — RSH repo ki latest release.</summary>
    public const string LatestReleaseUrl =
        "https://api.github.com/repos/dreamsofts34-creator/RSH/releases/latest";

    /// <summary>Release page (user ko yahin bhejo — download/install yahin se).</summary>
    public const string ReleasesPageUrl =
        "https://github.com/dreamsofts34-creator/RSH/releases";

    /// <summary>
    /// GitHub "latest release" JSON se tag version nikalo.
    /// tag_name "v2.0.0" (ya "2.0.0") hota hai; "v2.0.0-pre" jaisi
    /// pre-release tag ko ignore karo (null) — stable releases hi offer
    /// hote hain. Na mile / parse na ho to null.
    /// </summary>
    public static Version? ParseLatestTag(string json)
    {
        if (string.IsNullOrWhiteSpace(json)) return null;
        try
        {
            using JsonDocument doc = JsonDocument.Parse(json);
            if (!doc.RootElement.TryGetProperty("tag_name", out JsonElement tagEl)) return null;
            string? tag = tagEl.GetString();
            if (string.IsNullOrWhiteSpace(tag)) return null;
            tag = tag.Trim();
            if (tag.StartsWith("v", StringComparison.OrdinalIgnoreCase)) tag = tag[1..];
            // Pre-release (masalan "2.1.0-beta.1") — stable check mein ignore.
            if (tag.Contains('-')) return null;
            return Version.TryParse(tag, out Version? v) ? v : null;
        }
        catch (JsonException)
        {
            return null;
        }
    }

    /// <summary>Latest current se naya hai? (Barabar ya purana = false.)</summary>
    public static bool IsNewer(Version current, Version? latest)
    {
        ArgumentNullException.ThrowIfNull(current);
        return latest is not null && latest > current;
    }

    /// <summary>
    /// Live check: latest release tag lao aur current se compare karo.
    /// Koi bhi network/parse ghalti "unknown" result hai (exception nahi).
    /// Caller: background thread par chalao; UI thread par kabhi nahi.
    /// </summary>
    public static async Task<UpdateCheckResult> CheckForUpdateAsync(
        HttpClient http, Version current, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(http);
        ArgumentNullException.ThrowIfNull(current);
        try
        {
            using var request = new HttpRequestMessage(HttpMethod.Get, LatestReleaseUrl);
            request.Headers.UserAgent.Add(new ProductInfoHeaderValue("RSH", current.ToString()));
            request.Headers.Accept.Add(new MediaTypeWithQualityHeaderValue("application/vnd.github+json"));
            using HttpResponseMessage response = await http.SendAsync(request, ct).ConfigureAwait(false);
            if (!response.IsSuccessStatusCode)
            {
                return new UpdateCheckResult(false, null, current,
                    $"Update check fail hua (HTTP {(int)response.StatusCode}). Internet check karo ya {ReleasesPageUrl} kholo.",
                    DateTimeOffset.UtcNow);
            }
            string json = await response.Content.ReadAsStringAsync(ct).ConfigureAwait(false);
            Version? latest = ParseLatestTag(json);
            if (latest is null)
            {
                return new UpdateCheckResult(false, null, current,
                    "Update check ka jawab samajh nahi aaya (release tag nahi mila).",
                    DateTimeOffset.UtcNow);
            }
            return new UpdateCheckResult(IsNewer(current, latest), latest, current, null, DateTimeOffset.UtcNow);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException or JsonException)
        {
            return new UpdateCheckResult(false, null, current,
                $"Update check nahi ho saka ({ex.Message}). Internet check karo ya {ReleasesPageUrl} kholo.",
                DateTimeOffset.UtcNow);
        }
    }
}
