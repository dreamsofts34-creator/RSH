namespace RSH.Core.Windows;

/// <summary>Autostart ke liye registry jaisi key/value store ka abstraction (test ke liye).</summary>
public interface IAutostartStore
{
    /// <summary>Autostart value parho; na ho to <c>null</c>.</summary>
    string? ReadValue();

    /// <summary>Autostart value likho.</summary>
    void WriteValue(string value);

    /// <summary>Autostart value hatao (na ho to kuch nahi).</summary>
    void DeleteValue();
}

/// <summary>
/// Autostart verify (A5, SW-10): likhne ke baad dobara parh kar tasdeeq.
/// <para>
/// Pehle <c>SetAutostart</c> <c>void</c> tha aur khaali <c>catch {}</c> se
/// har failure nigal jata tha — UI ON dikhata tha jab ke registry mein
/// kuch likha hi nahi tha. Ab: likho → dobara parho → match na ho to
/// failure. Koi khaali catch nahi; har exception caller tak wajah ke
/// saath jati hai taake UI error dikha sake.
/// </para>
/// </summary>
public sealed class AutostartService
{
    private readonly IAutostartStore _store;
    private readonly Func<string> _currentExePath;

    public AutostartService(IAutostartStore store, Func<string> currentExePath)
    {
        _store = store;
        _currentExePath = currentExePath;
    }

    public bool GetAutostart()
    {
        try { return _store.ReadValue() is string; }
        catch { return false; } // parhna fail = autostart OFF samjho (safe direction)
    }

    /// <summary>Registry/store mein mojood autostart value (null = koi entry nahi); parhna fail ho to null.</summary>
    public string? GetStoredValue()
    {
        try { return _store.ReadValue(); }
        catch { return null; }
    }

    /// <summary>
    /// Is exe ke liye expected autostart value (<c>"&lt;exe&gt;" --minimized</c>);
    /// exe path khaali ho to null (likha nahi ja sakta).
    /// </summary>
    public string? ExpectedValue()
    {
        string exe = _currentExePath();
        return exe.Length == 0 ? null : $"\"{exe}\" --minimized";
    }

    /// <summary>
    /// Pure logic: kya stored value stale hai? Sirf tab <c>true</c> jab entry
    /// mojood ho AUR expected se mukhtalif ho (purane folder/version ki
    /// taraf ishara — v2.0.2 HOTFIX, neeche dekho).
    /// </summary>
    public static bool IsStaleValue(string? storedValue, string? expectedValue) =>
        storedValue is not null && expectedValue is not null && storedValue != expectedValue;

    /// <summary>
    /// v2.0.2 HOTFIX — stale autostart entry repair:
    /// <para>
    /// Autostart HKCU Run key mein <c>"&lt;exe path&gt;" --minimized</c>
    /// likhta hai — aur wo path sirf tab likha jata tha jab user ne toggle
    /// on kiya tha. User naya version naye folder mein extract karke chalaye
    /// to Run key abhi bhi PURANE folder ki taraf ishara karti rehti hai:
    /// agle login par purana (blank) version dobara chal parta hai, aur
    /// restart ke baad bhi purana instance nayi instance ko rok sakta hai
    /// (R-S1 refuted hua — restart se process mar jata hai — lekin
    /// autostart use login par dobara zinda kar deta hai, is liye ye edge
    /// case abhi bhi zinda tha). Is liye startup par: entry mojood ho aur
    /// current exe se mukhtalif ho to use current exe par overwrite karo.
    /// Entry na ho ya pehle se sahi ho to kuch nahi karna.
    /// </para>
    /// </summary>
    /// <param name="repaired">Entry overwrite hui?</param>
    /// <param name="error">Failure ki wajah; kamyabi par null.</param>
    /// <returns><c>true</c> = repair ki zaroorat nahi thi ya kamyab hui.</returns>
    public bool RepairStaleEntry(out bool repaired, out string? error)
    {
        repaired = false;
        error = null;
        string? stored = GetStoredValue();
        if (stored is null) return true; // koi entry nahi — repair karne ko kuch nahi
        string? expected = ExpectedValue();
        if (expected is null)
        {
            error = "App ka exe path nahi mila — stale autostart entry repair nahi ho sakti.";
            return false;
        }
        if (!IsStaleValue(stored, expected)) return true; // pehle se sahi
        if (!TrySetAutostart(true, out error)) return false;
        repaired = true;
        return true;
    }

    /// <summary>
    /// Autostart ko current exe par le aao (startup repair):
    /// <paramref name="desiredOn"/> = settings kehti on hai YA registry mein
    /// entry mojood hai. On ho to stale entry repair karo, entry na ho to
    /// banao (nayi folder/machine — settings on, registry khaali); off ho to
    /// kuch nahi karna. <paramref name="changed"/> = entry likhi gayi?
    /// </summary>
    public bool EnsureAutostartCurrent(bool desiredOn, out bool changed, out string? error)
    {
        changed = false;
        error = null;
        if (!desiredOn) return true;
        string? stored = GetStoredValue();
        string? expected = ExpectedValue();
        if (expected is null)
        {
            error = "App ka exe path nahi mila — autostart ensure nahi ho sakta.";
            return false;
        }
        if (stored == expected) return true; // pehle se sahi
        if (!TrySetAutostart(true, out error)) return false;
        changed = true;
        return true;
    }

    /// <summary>Autostart lagao/hatao; kamyabi sirf tab jab read-back match kare.</summary>
    public bool TrySetAutostart(bool enabled, out string? error)
    {
        error = null;
        try
        {
            if (enabled)
            {
                string exe = _currentExePath();
                if (exe.Length == 0)
                {
                    error = "App ka exe path nahi mila — autostart set nahi ho sakta.";
                    return false;
                }
                string expected = $"\"{exe}\" --minimized";
                _store.WriteValue(expected);
                if (_store.ReadValue() != expected)
                {
                    error = "Registry write verify nahi hui — value wapas parhne par match nahi karti.";
                    return false;
                }
            }
            else
            {
                _store.DeleteValue();
                if (_store.ReadValue() is not null)
                {
                    error = "Registry se autostart value delete nahi hui.";
                    return false;
                }
            }
            return true;
        }
        catch (Exception ex)
        {
            error = ex.Message;
            return false;
        }
    }
}
