using System.IO;
using System.Windows;
using System.Windows.Media;
using ApexLink.App.Services;
using ApexLink.App.ViewModels;
using ApexLink.App.Views;
using Serilog;
using Serilog.Events;
using Wpf.Ui.Appearance;

namespace ApexLink.App;

public partial class App : Application
{
    private static Mutex? _singleInstance;
    private AppServices? _services;

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // Single instance — do engine kabhi nahi (docs ka usool)
        _singleInstance = new Mutex(initiallyOwned: true, "ApexLink.SingleInstance", out bool isFirst);
        if (!isFirst)
        {
            MessageBox.Show("ApexLink pehle se chal raha hai (system tray dekho).", "ApexLink",
                MessageBoxButton.OK, MessageBoxImage.Information);
            Shutdown();
            return;
        }

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
            .WriteTo.File(Path.Combine(_services.LogDirectory, "apexlink-.log"),
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
        var t = theme switch
        {
            "Dark" => ApplicationTheme.Dark,
            "Light" => ApplicationTheme.Light,
            _ => ApplicationThemeManager.GetSystemTheme() == SystemTheme.Dark
                    ? ApplicationTheme.Dark : ApplicationTheme.Light,
        };
        ApplicationThemeManager.Apply(t);

        // Reference-style backdrop: halka pastel (light) / gehri indigo
        // (dark) — cards is par saaf ubhar kar aate hain.
        var bg = new SolidColorBrush(t == ApplicationTheme.Light
            ? Color.FromRgb(0xE9, 0xED, 0xFB)
            : Color.FromRgb(0x17, 0x19, 0x26));
        bg.Freeze();
        foreach (Window w in Current.Windows)
            if (w is MainWindow) w.Background = bg;
    }

    protected override void OnExit(ExitEventArgs e)
    {
        _services?.Dispose();
        WindowsIntegration.EndHighResolutionTimer();
        Log.CloseAndFlush();
        _singleInstance?.Dispose();
        base.OnExit(e);
    }
}
