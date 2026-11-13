using System.IO;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Media;
using RSH.App.Resources;
using RSH.App.Services;
using RSH.App.Themes;
using RSH.App.ViewModels;
using RSH.App.Views;
using RSH.Core.Windows;
using Serilog;
using Serilog.Events;
using Wpf.Ui.Appearance;

namespace RSH.App;

public partial class App : Application
{
    private SingleInstanceCoordinator? _singleInstance;
    private AppServices? _services;

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // A4 — Single instance: do engine kabhi nahi. Doosri instance
        // pehli ko named pipe se activate karti hai (bring-to-front),
        // phir khud band ho jati hai — koi duplicate engine nahi.
        _singleInstance = new SingleInstanceCoordinator();
        if (!_singleInstance.TryAcquire())
        {
            SingleInstanceCoordinator.SignalExisting();
            Shutdown();
            return;
        }
        _singleInstance.StartListening(() => Dispatcher.Invoke(BringMainWindowToFront));

        _services = new AppServices();

        // AUDIT FIX (F3): akhri-dam ka UI exception handler — koi bhi
        // na-sambhla exception app ko hard crash karne ke bajaye log ho
        // aur app chalti rahe.
        DispatcherUnhandledException += (_, args) =>
        {
            Log.Error(args.Exception, "Unhandled UI exception (handled, app continues)");
            args.Handled = true;
        };

        // Logging: rolling file, 7 din — level settings se
        var level = _services.Settings.LogLevel switch
        {
            "Debug" => LogEventLevel.Debug,
            "Information" => LogEventLevel.Information,
            "Error" => LogEventLevel.Error,
            _ => LogEventLevel.Warning,
        };
        Log.Logger = new LoggerConfiguration()
            .MinimumLevel.Is(level)
            // AUDIT FIX (F4): shared: true — Diagnostics zip export aaj ki
            // log file ko likhte hue bhi parh sake (warna sharing
            // violation par export aksar fail hota tha).
            .WriteTo.File(Path.Combine(_services.LogDirectory, "rsh-.log"),
                rollingInterval: RollingInterval.Day, retainedFileCountLimit: 7, shared: true,
                outputTemplate: "{Timestamp:yyyy-MM-dd HH:mm:ss.fff} [{Level:u3}] {Message:lj}{NewLine}{Exception}")
            .CreateLogger();

        WindowsIntegration.BeginHighResolutionTimer();

        // Design v2: ek dafa ki migration — naya light design default
        // hai (reference style). Baad mein user Settings se jo chaahe
        // chune, ye dobara nahi chalega.
        if (!_services.Settings.ThemeMigratedV12)
        {
            _services.Settings.Theme = "Light";
            _services.Settings.ThemeMigratedV12 = true;
            _services.SaveSettings();
        }

        // Brand accent — reference wala violet, poori app mein ek hi.
        ApplicationAccentColorManager.Apply(Color.FromRgb(0x6C, 0x4C, 0xF1));
        ApplyTheme(_services.Settings.Theme);
        // Phase F i18n: mehfooz language startup par lagao — MainWindow
        // banne se pehle, taake pehli render hi sahi language mein ho.
        UiStrings.ApplyLanguage(_services.Settings.Language);

        var mainWindow = new MainWindow(_services);
        MainWindow = mainWindow;

        bool startMinimized = _services.Settings.StartMinimized
                              || e.Args.Contains("--minimized");
        _services.StartRuntime();

        // Per-game auto profile switch (industry pattern — Fanatec /
        // Simucube): jo game samne aye, us ke liye set profile khud load.
        // Halka sa watcher: 1.5 s par sirf foreground process ka naam.
        string? lastForeground = null;
        var switchTimer = new System.Windows.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(1.5) };
        switchTimer.Tick += (_, _) =>
        {
            if (!_services.Settings.AutoSwitchProfiles) return;
            string? fg = WindowsIntegration.ForegroundProcessName();
            if (fg is null || fg == lastForeground) return;
            lastForeground = fg;
            var match = _services.Profiles.Profiles.FirstOrDefault(p =>
                p.GameProcessName.Length > 0 &&
                string.Equals(p.GameProcessName, fg, StringComparison.OrdinalIgnoreCase));
            if (match is not null && match.Name != _services.Profiles.ActiveProfileName)
            {
                _services.Profiles.ActiveProfileName = match.Name;
                _services.SaveProfilesAndApply();
                ViewModelStore.RaiseProfileChanged();
                Log.Information("Profile auto-switched to {Profile} for game {Game}", match.Name, fg);
            }
        };
        switchTimer.Start();

        if (!startMinimized)
        {
            mainWindow.Show();
            if (!_services.Settings.WizardCompleted)
            {
                var wizard = new WizardWindow(_services) { Owner = mainWindow };
                wizard.ShowDialog();
            }
        }
        else if (!_services.Settings.WizardCompleted)
        {
            // Pehli dafa minimized start par bhi wizard lazmi hai — window dikhao
            mainWindow.Show();
            var wizard = new WizardWindow(_services) { Owner = mainWindow };
            wizard.ShowDialog();
        }
    }

    public static void ApplyTheme(string theme)
    {
        // Phase F: "HighContrast" par WPF-UI chrome system theme (Light/Dark)
        // par rehta hai — RSH palette RSHThemeManager mein HighContrast
        // (SystemColors) hoti hai; baqi themes pehle jaisi.
        var t = theme switch
        {
            "Dark" => ApplicationTheme.Dark,
            "Light" => ApplicationTheme.Light,
            _ => ApplicationThemeManager.GetSystemTheme() == SystemTheme.Dark
                    ? ApplicationTheme.Dark : ApplicationTheme.Light,
        };
        ApplicationThemeManager.Apply(t);

        // UI-12 BUG FIX: pehle sirf WPF-UI theme badalta tha — RSH palette
        // (sidebar, nav, Ax* page content) Light par atka rehta tha, aur
        // MainWindow.Background par seedha brush lagane se
        // DynamicResource RSHBackgroundBrush mar jata tha (spec bg
        // #F5F3FF / #0B0E13 kabhi nahi lagta tha). Ab dono ek saath.
        RSHThemeManager.ApplyTheme(theme);
    }

    /// <summary>A4 — pehli instance ko samne lao: tray se nikalo, restore karo, foreground mein lao.</summary>
    private void BringMainWindowToFront()
    {
        var w = MainWindow;
        if (w is null) return;
        if (!w.IsVisible) w.Show();
        if (w.WindowState == WindowState.Minimized) w.WindowState = WindowState.Normal;
        w.Activate();
        // Activate() kabhi kabhi taskbar flash tak mehdood rehta hai —
        // foreground window ki darkhwast Windows par seedha karo.
        if (OperatingSystem.IsWindows())
            SetForegroundWindow(new System.Windows.Interop.WindowInteropHelper(w).Handle);
    }

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr hWnd);

    protected override void OnExit(ExitEventArgs e)
    {
        _services?.Dispose();
        WindowsIntegration.EndHighResolutionTimer();
        Log.CloseAndFlush();
        _singleInstance?.Dispose();
        base.OnExit(e);
    }
}
