using System.IO;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Interop;
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

    /// <summary>v2.0.2 diagnostic flag --classic-window: true ho to
    /// MainWindow FluentWindow chrome ke bajaye plain WPF title bar use
    /// karta hai (ExtendsContentIntoTitleBar=False). v2.0.3: Win10
    /// (build &lt; 22000) par AUTOMATIC true — flag ki zaroorat nahi;
    /// --fluent-window isay override karke Fluent force karta hai.
    /// Default Win11+ par false — behaviour bilkul waisa hi jaisa pehle tha.</summary>
    internal static bool ClassicWindowRequested { get; private set; }

    /// <summary>v2.0.3 diagnostic flag --plain-nav: true ho to MainWindow
    /// ki sidebar original-style saada hoti hai (literal text + system
    /// brushes + default RadioButton style — UiStrings/DynamicResource
    /// bindings nahi). Win10 (build &lt; 22000) par AUTOMATIC true —
    /// classic chrome ke saath (user koi flag test nahi karega, is liye
    /// Win10 par dono fallbacks ek saath auto ON); Win11+ par false.
    /// --fluent-window dono ko off karta hai (poora modern UI).
    /// Default Win11+ par false — behaviour bilkul waisa hi jaisa pehle tha.</summary>
    internal static bool PlainNavigationRequested { get; private set; }

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // v2.0.1 HOTFIX (P0 blank window): logger SAB SE PEHLE — AppServices
        // ya MainWindow banne se pehle bhi koi failure ho to file mein likha
        // ho. Pehle Log.Logger sirf AppServices ke baad banta tha — us se
        // pehle ki exception bilkul invisible thi (koi log, koi crash nahi).
        // v2.0.4 — NO-LOGS AUDIT ka hal: BootstrapLogging helper (ek hi
        // logic, dono logger-config jagah): primary %AppData%\RSH\logs
        // likhne-layak na ho to %TEMP%\RSH\logs fallback, aur Serilog
        // SelfLog ON (%TEMP%\RSH\serilog-selflog.txt) taake sink ki
        // khamosh failure nazar aaye. Pehle: na fallback tha, na SelfLog,
        // aur Directory.CreateDirectory try/catch ke bahar tha.
        BootstrapLogging.ConfigureLogger(
            LogEventLevel.Information, out bool bootstrapFallback);
        if (bootstrapFallback)
            Log.Warning("Primary log directory not writable — bootstrap logs going to fallback {FallbackDir}",
                BootstrapLogging.FallbackLogDirectory);
        // v2.0.2 HOTFIX: version assembly se lo (hard-code nahi) — taake
        // agle report se foran pata chale kaunsi binary chali thi. Dono
        // likho agar farq ho (informational mein commit hash bhi hota hai).
        var appVersion = AppVersionInfo.ForAssembly(typeof(App).Assembly);
        if (appVersion.InformationalVersion is not null
            && appVersion.InformationalVersion != appVersion.AssemblyVersion.ToString())
            Log.Information("RSH started v{DisplayVersion} (assembly {AssemblyVersion}, informational {InformationalVersion})",
                appVersion.DisplayVersion, appVersion.AssemblyVersion, appVersion.InformationalVersion);
        else
            Log.Information("RSH started v{DisplayVersion} (assembly {AssemblyVersion})",
                appVersion.DisplayVersion, appVersion.AssemblyVersion);
        // v2.0.4 — startup milestones ka second channel (Serilog se azaad):
        // %TEMP%\RSH\startup-status.txt — fresh file har run, taake log na
        // bhi mile to pata chale startup kahan tak pohancha tha.
        StartupStatus.Reset(appVersion.DisplayVersion);
        // v2.0.4 — log ki FULL PATH likho (log + status file dono mein):
        // Serilog rolling naam mein dashes NAHI hote (rsh-20261111.log,
        // rsh-2026-11-11.log nahi) — user dashed naam dhoondh raha tha.
        // Exact path likhne se agle run par koi andaza nahi lagana parega.
        string logFilePath = BootstrapLogging.CurrentLogFilePath;
        Log.Information("Log file: {LogFile}", logFilePath);
        StartupStatus.Write($"log file: {logFilePath}");

        // v2.0.2 HOTFIX — version-aware single instance: mutex/pipe naam
        // mein version hai (Global\RSH.SingleInstance.2.0.2 jaisa), is liye
        // purani version ki instance nayi ko kabhi nahi rokti. Us se PEHLE
        // purani version ka instance terminate karo (same user + RSH.exe +
        // file version < current — SingleInstanceCoordinator dekho); cleanup
        // fail ho to bhi startup nahi rukti, versioned mutex phir bhi
        // same-version duplicates rokta hai.
        // QA F4 (LOW) NOTE — is scan ko Show() ke baad / background par
        // NAHI le jaya gaya (jaan-boojh kar): ye purane version ko is
        // liye PEHLE maarta hai taake neeche AppServices ctor shared
        // settings.json / profiles.json parhne se pehle purana instance
        // mar chuka ho — warna purana instance (tray toggle /
        // auto-switch timer) naye instance ke stale in-memory snapshot
        // ke saath file par likh sakta hai (lost update) aur dono
        // engines thodi der saath chalte. Aam case mein cost sirf
        // legacy-mutex probe hai (process enum + Kill/Wait tabhi jab
        // purana instance asal mein mojood ho — tab correctness
        // delay se zyada ahem hai). TryAcquire / settings load se
        // pehle rehna lazmi hai; is block ko neeche mat sarkao.
        if (OperatingSystem.IsWindows())
        {
            try
            {
                int terminated = SingleInstanceCoordinator.TerminateOlderInstances(
                    appVersion.AssemblyVersion, msg => Log.Information("{StaleInstanceLog}", msg));
                if (terminated > 0)
                    Log.Information("Terminated {Count} older RSH instance(s) before startup", terminated);
            }
            catch (Exception ex)
            {
                Log.Warning(ex, "Older-instance cleanup failed — continuing with versioned single instance");
            }
        }

        // A4 — Single instance: do engine kabhi nahi. Doosri instance
        // (SAME version) pehli ko named pipe se activate karti hai
        // (bring-to-front), phir khud band ho jati hai — koi duplicate
        // engine nahi.
        _singleInstance = new SingleInstanceCoordinator(appVersion.MutexName, appVersion.PipeName);
        if (!_singleInstance.TryAcquire())
        {
            SingleInstanceCoordinator.SignalExisting(appVersion.PipeName);
            Log.Information("Same-version RSH instance already running — activating it and exiting");
            Shutdown();
            return;
        }
        _singleInstance.StartListening(() => Dispatcher.Invoke(BringMainWindowToFront));

        _services = new AppServices();
        Log.Information("App services created — active profile {Profile}", _services.Profiles.ActiveProfileName);

        // v2.0.2 HOTFIX — autostart stale-entry repair: Run key mein purane
        // folder ki taraf ishara karti entry agle login par purana version
        // dobara la sakti thi. Settings on ho ya registry entry mojood ho
        // to use current exe par le aao; failure startup nahi rokti.
        if (OperatingSystem.IsWindows())
        {
            try
            {
                bool autostartDesired = _services.Settings.Autostart || WindowsIntegration.GetAutostart();
                if (autostartDesired)
                {
                    if (WindowsIntegration.EnsureAutostartCurrent(true, out bool autostartChanged, out string? autostartError))
                    {
                        if (autostartChanged)
                            Log.Information("Autostart entry updated to current exe (stale entry repaired)");
                    }
                    else
                    {
                        Log.Warning("Autostart entry could not be updated to current exe: {Error}",
                            autostartError ?? "unknown error");
                    }
                }
            }
            catch (Exception ex)
            {
                Log.Warning(ex, "Autostart repair failed — continuing");
            }
        }

        // v2.0.2 diagnostic flags (opt-in — default behaviour bilkul
        // waisa hi jaisa pehle tha; flag na ho to yahan kuch nahi hota).
        // --software-render: GPU/driver render-thread failure (H2) ka
        // test — koi window banne se pehle process-wide software
        // rendering on karo (RenderOptions.ProcessRenderMode static hai,
        // ek dafa set ho to poore process par lagta hai). Render Tier
        // log mein likho taake agla report bataye ke GPU tier kya tha.
        // --classic-window: FluentWindow chrome ke bajaye plain WPF
        // title bar (WPF-UI #1594 jaisa blank-window case) — flag
        // MainWindow tak static holder se pohchta hai.
        // --plain-nav: sidebar original-style saada (literal text +
        // system brushes, UiStrings/DynamicResource bindings nahi) —
        // Win10 par auto ON (classic ke saath), flag bhi static
        // holder se MainWindow tak pohchta hai.
        if (e.Args.Contains("--software-render"))
        {
            RenderOptions.ProcessRenderMode = RenderMode.SoftwareOnly;
            Log.Information("Software rendering requested (Render Tier: {Tier})", RenderCapability.Tier);
        }
        // v2.0.3 HOTFIX — Win10 par AUTOMATIC classic window: FluentWindow +
        // ExtendsContentIntoTitleBar Win10 (build < 22000) par paint nahi
        // hota (blank window), jabke plain WPF window usi PC par render
        // hoti hai — is liye Win10 par flag ke baghair hi classic; Win11
        // par FluentWindow waisa hi. --classic-window force karta hai,
        // --fluent-window (escape hatch) Fluent force karta hai.
        // Non-Windows par classic=false (OS version sirf Windows par parho).
        bool classicFlag = e.Args.Contains("--classic-window");
        bool fluentFlag = e.Args.Contains("--fluent-window");
        bool plainNavFlag = e.Args.Contains("--plain-nav");
        // v2.0.4 — OS version DO zariyon se (belt-and-braces): Environment.OSVersion
        // (manifest par inhisar — shipped exe mein manifest verify hai: RT_MANIFEST
        // maujood, source se byte-identical — aur .NET 5+ waise bhi sahi version
        // deta hai) aur RtlGetVersion-based OperatingSystem.IsWindowsVersionAtLeast
        // (manifest-independent). Faisla RtlGetVersion se hota hai — Win11
        // boundary (build 22000) ke liye wahi authority. Dono mein farq ho to
        // WARNING log karo — yehi asal diagnostic hai (agar Environment.OSVersion
        // kabhi jhoot bole to log mein nazar aaye).
        Version osVersion = OperatingSystem.IsWindows()
            ? Environment.OSVersion.Version
            : new Version(0, 0);
        bool rtlWin11OrNewer = OperatingSystem.IsWindows()
            && OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000);
        bool envWin11OrNewer = OperatingSystem.IsWindows()
            && !WindowChromePolicy.RequiresClassicWindow(osVersion);
        if (OperatingSystem.IsWindows() && rtlWin11OrNewer != envWin11OrNewer)
        {
            Log.Warning("OS version sources disagree: Environment.OSVersion={OSVersion} (Win11+={EnvWin11}) vs RtlGetVersion Win11+={RtlWin11} — using RtlGetVersion for the window decision",
                osVersion, envWin11OrNewer, rtlWin11OrNewer);
        }
        ClassicWindowRequested = OperatingSystem.IsWindows()
            && WindowChromePolicy.ShouldUseClassicWindow(rtlWin11OrNewer, classicFlag, fluentFlag);
        // v2.0.3 HOTFIX — Win10 par AUTOMATIC plain navigation: classic
        // chrome ke saath sidebar bhi original-style saada ho (literal
        // text + system brushes, UiStrings/DynamicResource bindings
        // nahi) — user koi flag test nahi karega, is liye Win10 par
        // dono fallbacks ek saath auto ON. Win11+ par modern sidebar
        // waisa hi. --plain-nav force-on, --fluent-window dono ko off.
        PlainNavigationRequested = OperatingSystem.IsWindows()
            && WindowChromePolicy.ShouldUsePlainNavigation(rtlWin11OrNewer, plainNavFlag, fluentFlag);
        string chromeReason = OperatingSystem.IsWindows()
            ? WindowChromePolicy.DescribeDecision(osVersion, ClassicWindowRequested)
            : "non-Windows — classic window not applicable";
        Log.Information("Window chrome: {Mode} (OS {OSVersion}, classic-flag={ClassicFlag}, fluent-flag={FluentFlag}, reason: {Reason})",
            ClassicWindowRequested ? "classic window" : "FluentWindow",
            osVersion, classicFlag, fluentFlag, chromeReason);
        Log.Information("Navigation: {NavMode} (OS {OSVersion}, plain-nav-flag={PlainNavFlag}, fluent-flag={FluentFlag})",
            PlainNavigationRequested ? "plain navigation (original-style sidebar)" : "modern navigation",
            osVersion, plainNavFlag, fluentFlag);
        StartupStatus.Write(OperatingSystem.IsWindows()
            ? $"OS {osVersion} — {(ClassicWindowRequested ? "classic window" : "FluentWindow")} / {(PlainNavigationRequested ? "plain navigation" : "modern navigation")}"
            : "non-Windows — classic window not applicable");

        // (Purana unversioned single-instance + duplicate AppServices block
        // yahan tha — v2.0.2 mein upar versioned block se replace ho gaya;
        // doosri dafa TryAcquire/AppServices nahi chalne chahiye.)

        // AUDIT FIX (F3): akhri-dam ka UI exception handler — koi bhi
        // na-sambhla exception app ko hard crash karne ke bajaye log ho
        // aur app chalti rahe.
        DispatcherUnhandledException += (_, args) =>
        {
            Log.Error(args.Exception, "Unhandled UI exception (handled, app continues)");
            // v2.0.4 — handler HATAO mat (v2.0.1 resilience: crash se
            // bachata hai), lekin swallowed exception ko status file mein
            // bhi likho — warna UI-thread exception (render/layout pass
            // se aane wali bhi) nigal kar app "khaali shell" ki tarah
            // chalti rehti hai aur diagnostics mein kuch nazar nahi aata.
            // StartupStatus.WriteException kabhi throw nahi karta.
            StartupStatus.WriteException("Unhandled UI exception (handled, app continues)", args.Exception);
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
        // v2.0.4 — DOBARA CONFIG NAHI: logger EK dafa bootstrap par ban
        // chuka hai; yahan sirf settings wala level lagao
        // (LoggingLevelSwitch — logger replace nahi hota). REPRO NOTE:
        // replace se lines nahi khotin (shared:true sink foran likhta
        // hai) — single-config hygiene hai, no-logs ka root cause nahi.
        BootstrapLogging.SetMinimumLevel(level);
        Log.Information("Log directory: {LogDirectory} (today's log file: {LogFile})",
            BootstrapLogging.CurrentLogDirectory, BootstrapLogging.CurrentLogFilePath);

        WindowsIntegration.BeginHighResolutionTimer();

        // Design v2: ek dafa ki migration — naya light design default
        // hai (reference style). Baad mein user Settings se jo chaahe
        // chune, ye dobara nahi chalega.
        if (!_services.Settings.ThemeMigratedV12)
        {
            _services.Settings.Theme = "Light";
            _services.Settings.ThemeMigratedV12 = true;
            // QA F3 (LOW, U-P1): sync SaveSettings() UI thread par file
            // likhta tha — async rasta (file I/O background par; fail ho
            // to log, startup nahi rukti). In-memory theme pehle se set
            // hai (neeche ApplyTheme), save sirf persist karta hai.
            _ = _services.SaveSettingsAsync().ContinueWith(
                static t => Log.Error(t.Exception, "Theme migration save failed"),
                CancellationToken.None,
                TaskContinuationOptions.OnlyOnFaulted, TaskScheduler.Default);
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
            // v2.0.4 — REAL classic window: ClassicWindowRequested ho to
            // plain System.Windows.Window (ClassicMainWindow), warna
            // MainWindow (FluentWindow) bilkul waisa hi. v2.0.3 ka
            // "classic" asal mein FluentWindow hi rehta tha (sirf 3
            // properties flip hoti thin) — is liye Win10 par blank rehta
            // tha. MainWindow property type Window hai — dono fit hote
            // hain; Show()/WindowState/wizard Owner sab base type par.
            Window mainWindow = App.ClassicWindowRequested
                ? new ClassicMainWindow(_services)
                : new MainWindow(_services);
            MainWindow = mainWindow;
            Log.Information("Main window created ({WindowType})", mainWindow.GetType().Name);
            StartupStatus.Write($"window created ({mainWindow.GetType().Name})");

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
        StartupStatus.Write(startMinimized && _services.Settings.WizardCompleted
            ? "window shown (minimized)"
            : "window shown");

        _ = Task.Run(() =>
        {
            try
            {
                _services.StartRuntime();
                Log.Information("RSH runtime started");
                StartupStatus.Write("runtime started");
            }
            catch (Exception ex)
            {
                Log.Fatal(ex, "Failed to start RSH runtime");
            }
        });

        if (!_services.Settings.WizardCompleted)
        {
            if (App.ClassicWindowRequested)
            {
                // v2.0.4 — classic mode mein FluentWindow wizard NA kholo:
                // wizard bhi FluentWindow hai (Views/WizardWindow.xaml:1)
                // aur classic path ka maqsad hi Fluent chrome se bachna
                // hai. Setup screen mein wahi steps hain (Connect → Pins
                // → Calibrate), is liye skip safe degradation hai, feature
                // change nahi. Win11 par wizard waisa hi.
                Log.Warning("First-run wizard skipped in classic mode — use Setup screen");
                StartupStatus.Write("wizard skipped (classic mode — use Setup screen)");
            }
            else
            {
                // Pehli dafa wizard lazmi hai — minimized start par bhi.
                Log.Information("Showing first-run wizard");
                var wizard = new WizardWindow(_services) { Owner = mainWindow };
                wizard.ShowDialog();
                Log.Information("First-run wizard closed");
                StartupStatus.Write("wizard closed");
            }
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
