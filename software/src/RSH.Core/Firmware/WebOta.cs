namespace RSH.Core.Firmware;

/// <summary>
/// WebOTA CSRF token (SEC-D-12): board ka WebOTA page per-boot random
/// token deta hai (<c>&lt;meta name="csrf-token" content="…"&gt;</c>);
/// <c>/update</c> POST ko wahi token <c>X-RSH-CSRF</c> header mein
/// chahiye, warna board 403 deta hai. Token na mile to legacy board
/// samjho (us par token check nahi hai) — header ke baghair bhejo.
/// </summary>
public static class WebOta
{
    /// <summary>CSRF token header ka naam (board + app dono yahi istemal karte hain).</summary>
    public const string CsrfHeaderName = "X-RSH-CSRF";

    /// <summary>WebOTA page ke &lt;meta&gt; tag ka naam.</summary>
    public const string CsrfMetaName = "csrf-token";

    /// <summary>
    /// WebOTA page HTML se CSRF token nikalo. Token 32 hex chars hota hai
    /// (16 random bytes). Na mile ya format ghalat ho to null — caller
    /// legacy board samajh kar header ke baghair aage barhe.
    /// </summary>
    public static string? TryExtractCsrfToken(string html)
    {
        if (string.IsNullOrEmpty(html)) return null;
        int meta = html.IndexOf(CsrfMetaName, StringComparison.Ordinal);
        if (meta < 0) return null;
        const string marker = "content=\"";
        int start = html.IndexOf(marker, meta, StringComparison.Ordinal);
        if (start < 0) return null;
        start += marker.Length;
        if (start + 32 > html.Length) return null;
        string token = html.Substring(start, 32);
        if (html[start + 32] != '"') return null;
        foreach (char c in token)
        {
            bool hex = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
            if (!hex) return null;
        }
        return token.ToLowerInvariant();
    }
}
