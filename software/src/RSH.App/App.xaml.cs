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

        // v2.0.1 HOTFIX (P0 blank window): logger SAB SE PEHLE — AppServices
        // ya MainWindow banne se pehle bhi koi failure ho to file mein likha
        // ho. Pehle Log.Logger sirf AppServices ke baad banta tha — us se
        // pehle ki exception bilkul invisible thi (koi log, koi crash nahi).
        var bootstrapLogDir = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "RSH", "logs");
        Directory.CreateDirectory(bootstrapLogDir);
        Log.Logger = new LoggerConfiguration()
            .MinimumLevel.Is(LogEventLevel.Information)
            .WriteTo.File(Path.Combine(bootstrapLogDir, "rsh-.log"),
                rollingInterval: RollingInterval.Day, retainedFileCountLimit: 7, shared: true,
                outputTemplate: "{Timestamp:yyyy-MM-dd HH:mm:ss.fff} [{Level:u3}] {Message:lj}{NewLine}{Exception}")
            .CreateLogger();
        Log.Information("RSH started");

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
        Log.Information("App services created — active profile {Profile}", _services.Profiles.ActiveProfileName);

        // AUDIT FIX (F3): akhri-dam ka UI exception handler — koi bhi
        // na-sambhla exception app ko hard crash karne ke bajaye log ho
        // aur app chalti rahe.
        DispatcherUnhandledException += (_, args) =>
        {
            Log.Error(args.Exception, "Unhandled UI exception (handled, app continues)");
            args.Handled = true;
        };

        // v2.0.1 HOTFIX (P0): UI-thread ke bahar ya Task mein phansa
        // exception bhi file mein aaye — warna app khamoshi se aadhi
        // initialized haalat mein chalti rehti hai.
        AppDomain.CurrentDomain.UnhandledException += (_, args) =>
            Log.Fatal(args.ExceptionObject as Exception, "Unhandled non-UI exception (IsTerminating={IsTerminating})", args.IsTerminating);
        TaskScheduler.UnobservedTaskException += (_, args) =>
        {
            Log.Error(args.Exception, "Unobserved task exception");
            args.SetObserved();
        };

        // Logging: rolling file, 7 din — level settings se
        var level = _services.Settings.LogLevel switch
        {
            "Debug" => LogEventLevel.Debug,
            "Information" => LogEventLevel.Information,
            "Error" => LogEventLevel.Error,
            // v2.0.1: default pehle Warning tha — startup milestones
            // (Information) by default kabhi file mein aate hi nahi thay,
            // is liye blank-window jaisa incident diagnose hi nahi ho
            // sakta tha. Default ab Information hai.
            _ => LogEventLevel.Information,
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
        Log.Information("Theme applied: {Theme}", _services.Settings.Theme);
        // Phase F i18n: mehfooz language startup par lagao — MainWindow
        // banne se pehle, taake pehli render hi sahi language mein ho.
        UiStrings.ApplyLanguage(_services.Settings.Language);
        Log.Information("Language applied: {Language}", _services.Settings.Language);

        // v2.0.1 HOTFIX (P0): window banna / runtime / Show — in mein se
        // koi step fail ho to pehle woh bilkul invisible tha (logger bhi
        // nahi, milestone bhi nahi). Ab har step log hota hai aur failure
        // Fatal ke saath file mein likhi jati hai, phir dobara throw hoti
        // hai (behaviour waisa hi — sirf diagnose ho sakta hai).
        try
        {
            var mainWindow = new MainWindow(_services);
            MainWindow = mainWindow;
            Log.Information("Main window created");

            bool startMinimized = _services.Settings.StartMinimized
                                  || e.Args.Contains("--minimized");

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
                // v2.0.1 FREEZE FIX (U-P1): sync SaveProfilesAndApply()
                // yahan UI thread par file likhta tha — har auto-switch
                // par UI rukti thi. Async rasta: file I/O background par,
                // engine apply in-memory; fail ho to log, launch nahi rukti.
                _ = _services.SaveProfilesAndApplyAsync().ContinueWith(
                    static (t, state) => Log.Error(t.Exception,
                        "Profile auto-switch save failed for {Profile}", (string)state!),
                    match.Name, CancellationToken.None,
                    TaskContinuationOptions.OnlyOnFaulted, TaskScheduler.Default);
                ViewModelStore.RaiseProfileChanged();
                Log.Information("Profile auto-switched to {Profile} for game {Game}", match.Name, fg);
            }
        };
        switchTimer.Start();

        // v2.0.1 FREEZE FIX (U-P1/U-P8): PEHLE window dikhao, PHIR runtime
        // start karo — pehle StartRuntime() Show() se pehle UI thread par
        // chalta tha, is liye pehli paint us ka intezar karti thi. Ab
        // Show() foran hota hai aur runtime background par start hota hai
        // (chaaron Start() methods sirf thread/timer/Task shuru karte
        // hain — koi UI touch nahi, lock-protected — lekin paint phir bhi
        // kabhi in ka intezar nahi karta).
        if (startMinimized && _services.Settings.WizardCompleted)
        {
            // Minimized start par bhi Show lazmi hai — warna Loaded kabhi
            // nahi chalta, tray icon kabhi nahi banta aur app baghair kisi
            // UI ke chalti rehti hai. WindowState pehle se Minimized hai,
            // is liye koi flash nahi.
            mainWindow.WindowState = WindowState.Minimized;
        }
        mainWindow.Show();
        Log.Information("Main window shown{Minimized}",
            startMinimized && _services.Settings.WizardCompleted ? " minimized" : "");

        _ = Task.Run(() =>
        {
            try
            {
                _services.StartRuntime();
                Log.Information("RSH runtime started");
            }
            catch (Exception ex)
            {
                Log.Fatal(ex, "Failed to start RSH runtime");
            }
        });

        if (!_services.Settings.WizardCompleted)
        {
            // Pehli dafa wizard lazmi hai — minimized start par bhi.
            Log.Information("Showing first-run wizard");
            var wizard = new WizardWindow(_services) { Owner = mainWindow };
            wizard.ShowDialog();
            Log.Information("First-run wizard closed");
        }
        }
        catch (Exception ex)
        {
            Log.Fatal(ex, "Startup failed while creating/starting/showing the main window");
            throw;
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
