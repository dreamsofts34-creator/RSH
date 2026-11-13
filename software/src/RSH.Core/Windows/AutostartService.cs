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
