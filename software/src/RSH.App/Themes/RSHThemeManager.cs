using System.Windows;
using Wpf.Ui.Appearance;

namespace RSH.App.Themes;

/// <summary>
/// Theme switch ka mechanism (UI-1, Settings → Appearance se badlega — UI-11).
/// Sirf resource dictionaries badalta hai: RSHTheme.xaml ke andar merged
/// palette (RSHTheme.Light.xaml / RSHTheme.Dark.xaml) replace hoti hai.
/// Koi layout rebuild nahi, koi heavy kaam nahi (U-P10). Saare theme-dependent
/// brushes RSHTheme.xaml mein DynamicResource se bandhe hain, is liye swap
/// foran poori window par lagta hai. UI thread par koi I/O nahi (U-P1) —
/// palettes assembly resources hain, file/network access nahi.
/// </summary>
public static class RSHThemeManager
{
    public const string Light = "Light";
    public const string Dark = "Dark";
    public const string System = "System";

    /// <summary>Is waqt ka active RSH palette: "Light" ya "Dark".</summary>
    public static string CurrentTheme { get; private set; } = Light;

    /// <summary>
    /// Theme lagao: "Light" | "Dark" | "System" (system = OS theme, WPF-UI se).
    /// Ghalat/empty value par "System" jaisa sulook hota hai.
    /// </summary>
    public static void ApplyTheme(string? theme)
    {
        string resolved = theme switch
        {
            Dark => Dark,
            Light => Light,
            _ => ApplicationThemeManager.GetSystemTheme() == SystemTheme.Dark ? Dark : Light,
        };
        if (SwapPalette(resolved))
            CurrentTheme = resolved;
    }

    private static bool SwapPalette(string theme)
    {
        var app = Application.Current;
        if (app is null) return false;

        ResourceDictionary palette;
        try
        {
            // AssemblyName "RSH" hai (RSH.App.csproj) — pack URI usi se banta hai.
            palette = new ResourceDictionary
            {
                Source = new Uri(
                    $"pack://application:,,,/RSH;component/Themes/RSHTheme.{theme}.xaml",
                    UriKind.Absolute),
            };
        }
        catch (Exception)
        {
            return false;
        }

        // RSHTheme.xaml app-level merge ho ya kisi window (MainWindow) mein —
        // dono jagah dhoondho; jahan mile, us ke andar ki palette badlo.
        foreach (var merged in app.Resources.MergedDictionaries)
            if (TrySwapIn(merged, palette)) return true;
        foreach (Window window in app.Windows)
            foreach (var merged in window.Resources.MergedDictionaries)
                if (TrySwapIn(merged, palette)) return true;
        return false;
    }

    private static bool TrySwapIn(ResourceDictionary themeDictionary, ResourceDictionary palette)
    {
        if (themeDictionary.Source is null ||
            !themeDictionary.Source.OriginalString.EndsWith("Themes/RSHTheme.xaml", StringComparison.OrdinalIgnoreCase))
            return false;

        for (int i = 0; i < themeDictionary.MergedDictionaries.Count; i++)
        {
            var source = themeDictionary.MergedDictionaries[i].Source?.OriginalString ?? string.Empty;
            if (source.EndsWith("RSHTheme.Light.xaml", StringComparison.OrdinalIgnoreCase) ||
                source.EndsWith("RSHTheme.Dark.xaml", StringComparison.OrdinalIgnoreCase))
            {
                themeDictionary.MergedDictionaries[i] = palette;
                return true;
            }
        }

        themeDictionary.MergedDictionaries.Add(palette);
        return true;
    }
}
