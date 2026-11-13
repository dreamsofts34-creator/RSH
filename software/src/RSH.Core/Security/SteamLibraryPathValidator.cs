namespace RSH.Core.Security;

/// <summary>
/// SEC-E-11: Steam <c>libraryfolders.vdf</c> se nikle paths ki validation.
///
/// VDF file Steam ki hai, lekin us ka content user ke control mein nahi
/// rehta (shared PC, haath se badli file, ya malicious library path):
/// <c>"path" "..\\..\\Windows"</c> jaisi value se nikla library root
/// phir <c>steamapps\common\&lt;game&gt;\&lt;exe&gt;</c> ke saath jor kar
/// <c>File.Exists</c> + <c>Process.Start</c> tak jata hai — traversal
/// wala path game exe ki jagah kisi aur jagah ki exe chala sakta hai.
/// Is liye VDF se nikla har path istemal se PEHLE yahan se guzarta hai:
///
/// <list type="bullet">
/// <item>absolute Windows path hona lazmi (drive letter + <c>\</c>) — relative / UNC kabhi nahi;</item>
/// <item><c>..</c> segment kabhi nahi — upar jane ka rasta band;</item>
/// <item>drive root akela nahi, aur <c>Windows</c> system directory ke andar nahi;</item>
/// <item>resolved exe candidate sirf validated library ke
/// <c>steamapps\common</c> subtree ke andar ho (<see cref="IsExeUnderLibrary"/>).</item>
/// </list>
///
/// Windows-path logic yahan jaan-boojh kar OS-independent hai (drive
/// letter + backslash khud parse hote hain, <c>Path.GetFullPath</c>
/// nahi — Linux host tests par bhi wahi nateeja aata hai jo Windows
/// runtime par aayega). Caller (GameLauncher) ko sirf validated
/// normalized path milta hai; ghalat path khamoshi se drop hota hai
/// (wajah sirf diagnostics mein — VDF parse kabhi crash nahi karta).
/// </summary>
public static class SteamLibraryPathValidator
{
    /// <summary>
    /// VDF se nikli raw path value validate + normalize karo.
    /// True = istemal ke qaabil (normalizedPath mein), false + error = drop karo.
    /// </summary>
    public static bool TryValidateLibraryPath(string? rawPath, out string? normalizedPath, out string? error)
    {
        normalizedPath = null;
        error = null;
        if (string.IsNullOrWhiteSpace(rawPath))
        {
            error = "Steam library path khaali hai.";
            return false;
        }
        // VDF mein backslash escape hota hai ("C:\\\\Games") — pehle unescape.
        string path = rawPath.Trim().Replace(@"\\", @"\");
        if (path.IndexOf('\0') >= 0 || path.Any(char.IsControl))
        {
            error = "Steam library path mein control characters hain.";
            return false;
        }
        // Separators ek jaisi karo, phir segments mein toro.
        string unified = path.Replace('/', '\\');
        string[] segments = unified.Split('\\', StringSplitOptions.RemoveEmptyEntries);
        // ".." segment kabhi nahi — traversal ka rasta band (resolve
        // karne se pehle reject, taake "C:\Steam\..\..\Windows" bhi na guzre).
        if (segments.Any(s => s == ".."))
        {
            error = $"Steam library path '{rawPath}' mein '..' hai — path traversal reject hota hai.";
            return false;
        }
        // Absolute Windows path: drive letter + ':' + '\' (UNC "\\server\share" nahi).
        if (unified.StartsWith(@"\\", StringComparison.Ordinal) ||
            segments.Length < 2 ||
            segments[0].Length != 2 || !char.IsLetter(segments[0][0]) || segments[0][1] != ':' ||
            !unified.StartsWith(segments[0] + "\\", StringComparison.Ordinal))
        {
            error = $"Steam library path '{rawPath}' absolute Windows path nahi hai (drive letter se shuru hona chahiye, UNC/relative nahi).";
            return false;
        }
        // "." segments hatao (resolve, reject nahi — "C:\Steam\." theek hai).
        var clean = new List<string> { segments[0] };
        foreach (string s in segments.Skip(1))
            if (s != ".")
                clean.Add(s);
        if (clean.Count < 2)
        {
            error = $"Steam library path '{rawPath}' drive root akela hai — library root nahi ho sakta.";
            return false;
        }
        // Windows system directory ke andar kabhi nahi.
        if (string.Equals(clean[1], "Windows", StringComparison.OrdinalIgnoreCase))
        {
            error = $"Steam library path '{rawPath}' Windows system directory ke andar hai.";
            return false;
        }
        normalizedPath = string.Join("\\", clean);
        return true;
    }

    /// <summary>
    /// Resolved game-exe candidate validated library roots ke andar hai?
    /// Exe sirf tab chalane ke qaabil jab wo kisi validated library ke
    /// <c>steamapps\common\...</c> subtree mein ho. <paramref name="validatedLibraries"/>
    /// mein sirf <see cref="TryValidateLibraryPath"/> se guzri hui paths hon.
    /// </summary>
    public static bool IsExeUnderLibrary(string? exePath, IReadOnlyList<string> validatedLibraries)
    {
        if (string.IsNullOrWhiteSpace(exePath) || validatedLibraries.Count == 0) return false;
        string unified = exePath.Trim().Replace('/', '\\');
        string[] segments = unified.Split('\\', StringSplitOptions.RemoveEmptyEntries);
        if (segments.Any(s => s == "..")) return false;
        foreach (string library in validatedLibraries)
        {
            string root = library.TrimEnd('\\') + @"\steamapps\common\";
            if (unified.StartsWith(root, StringComparison.OrdinalIgnoreCase) &&
                unified.Length > root.Length)
                return true;
        }
        return false;
    }

    /// <summary>
    /// Launch-time gate (GameLauncherViewModel): game-exe candidate
    /// plausible Steam path hai? Absolute Windows path + koi <c>..</c>
    /// nahi + <c>steamapps\common\</c> subtree ke andar + <c>.exe</c>.
    /// Ye VDF-parse-time validation ka doosra darwaza hai (defense in
    /// depth) — scan se nikli koi bhi ajeeb path yahan rukti hai, chahay
    /// parse-time validator kisi wajah se na laga ho.
    /// </summary>
    public static bool IsPlausibleGameExePath(string? exePath)
    {
        if (string.IsNullOrWhiteSpace(exePath)) return false;
        string unified = exePath.Trim().Replace('/', '\\');
        string[] segments = unified.Split('\\', StringSplitOptions.RemoveEmptyEntries);
        if (segments.Any(s => s == "..")) return false;
        if (segments.Length < 2 || segments[0].Length != 2 ||
            !char.IsLetter(segments[0][0]) || segments[0][1] != ':')
            return false;
        if (!unified.StartsWith(segments[0] + "\\", StringComparison.Ordinal)) return false;
        if (!unified.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)) return false;
        return unified.Contains(@"\steamapps\common\", StringComparison.OrdinalIgnoreCase);
    }
}
