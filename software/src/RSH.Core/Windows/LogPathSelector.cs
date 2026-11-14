namespace RSH.Core.Windows;

/// <summary>
/// v2.0.4 — log directory ka faisla: primary (<c>%AppData%\RSH\logs</c>)
/// likhne-layak ho to wahi, warna fallback (<c>%TEMP%\RSH\logs</c>).
///
/// Background: Serilog ka file sink likhne mein fail ho to events
/// KHAMOSHI se drop kar deta hai (SelfLog off ho to koi error kahin
/// nahi aata) — is liye logger banane se PEHLE directory ki writability
/// probe karna lazmi hai. Probe: directory banao + aik chhoti probe file
/// likho + delete karo. Kuch bhi fail ho to fallback — kabhi throw nahi.
/// </summary>
public static class LogPathSelector
{
    private const string ProbeFileName = ".rsh-write-probe";

    /// <summary>
    /// Writable primary ho to <paramref name="primaryDir"/>, warna
    /// <paramref name="fallbackDir"/>. Dono null/empty hon to fallback
    /// (jo bhi diya gaya ho) wapas — caller phir bhi try karega.
    /// </summary>
    public static string ChooseLogDirectory(string? primaryDir, string? fallbackDir)
    {
        if (!string.IsNullOrWhiteSpace(primaryDir) && IsWritableDirectory(primaryDir))
            return primaryDir;
        return fallbackDir ?? primaryDir ?? string.Empty;
    }

    /// <summary>
    /// Directory banao (agar na ho) aur probe file likh kar delete karo.
    /// Koi bhi failure = not writable. Kabhi throw nahi karta.
    /// </summary>
    public static bool IsWritableDirectory(string? dir)
    {
        if (string.IsNullOrWhiteSpace(dir))
            return false;
        try
        {
            Directory.CreateDirectory(dir);
            string probe = Path.Combine(dir, ProbeFileName);
            File.WriteAllText(probe, "probe");
            File.Delete(probe);
            return true;
        }
        catch
        {
            return false;
        }
    }
}
