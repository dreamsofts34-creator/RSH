using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using RSH.App.Services;
using RSH.App.Themes;
using RSH.App.ViewModels;
using RSH.App.Views.Pages;
using RSH.Core.Windows;
using Serilog;

namespace RSH.App.Views;

/// <summary>
/// v2.0.4 — REAL classic main window: plain <see cref="Window"/> (OS title
/// bar), koi FluentWindow / WPF-UI chrome nahi. Win10 (build &lt; 22000) ya
/// --classic-window par <see cref="App"/> isi ko kholta hai; Win11 par
/// <see cref="MainWindow"/> bilkul waisa hi rehta hai.
///
/// Shell MainWindow jaisa hi hai (8 screens, same page factories, same
/// navigation + fallback, 30 Hz UI timer, tray best-effort guarded, theme
/// guarded) — farq sirf window chrome aur sidebar presentation hai
/// (literal text + system brushes + default RadioButton style).
/// Wizard yahan kabhi nahi khulta (FluentWindow hai) — <see cref="ShowWizard"/>
/// ke bajaye Setup page par le jata hai (wahi steps wahan hain).
/// </summary>
public partial class ClassicMainWindow : Window
{
    private readonly AppServices _services;
    private readonly DispatcherTimer _uiTimer;
    private object? _currentPage;
    private Wpf.Ui.Tray.Controls.NotifyIcon? _trayIcon;
    private bool _reallyExit;

    public ClassicMainWindow(AppServices services)
    {
        _services = services;
        InitializeComponent();

        // v2.0.4 — version window title mein (assembly se, hard-code nahi)
        // taake user ki photo se hi version confirm ho.
        Title = WindowTitle.ForAssembly(typeof(App).Assembly);

        Log.Information("Classic main window created (plain System.Windows.Window — no FluentWindow chrome)");

        // UI refresh sirf 30 Hz, sirf jab window samne ho — MainWindow jaisa.
        _uiTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(33) };
        _uiTimer.Tick += (_, _) =>
        {
            if (!IsVisible || WindowState == WindowState.Minimized) return;
            (_currentPage as IUpdatableView)?.Tick();
        };

        ViewModelStore.Dashboard.RequestWizard = ShowWizard;
        ViewModelStore.Settings.RequestWizard = ShowWizard;
        ViewModelStore.Setup.RequestHome = () => NavDashboard.IsChecked = true;
        ViewModelStore.Dashboard.RequestSetup = () => NavSetup.IsChecked = true;

        Loaded += (_, _) =>
        {
            // v2.0.4 — Loaded milestone: agar render pipeline kabhi
            // complete na ho to Loaded fire hi nahi hota — status file
            // mein ye line batayegi ke window load hui ya nahi.
            StartupStatus.Write("classic window loaded (Loaded fired)");
            // Har step alag try/catch mein (MainWindow.Loaded jaisa) —
            // kisi ek step ki failure baaqi sab na maare.
            try
            {
                RSHThemeManager.ApplyTheme(_services.Settings.Theme);
                Log.Information("Theme applied on classic window load: {Theme}", _services.Settings.Theme);
            }
            catch (Exception ex)
            {
                Log.Error(ex, "Theme apply failed on classic window load (continuing)");
            }

            try
            {
                SetupTray();
                Log.Information("Tray icon created (classic window)");
            }
            catch (Exception ex)
            {
                Log.Error(ex, "Tray icon setup failed — continuing without tray");
                _trayIcon = null;
            }

            try
            {
                _uiTimer.Start();
            }
            catch (Exception ex)
            {
                Log.Error(ex, "UI timer failed to start (classic window)");
            }

            try
            {
                NavDashboard.IsChecked = true;
                Log.Information("First navigation requested: Dashboard (classic window)");
            }
            catch (Exception ex)
            {
                Log.Error(ex, "First navigation failed (classic window)");
                ShowNavigationFallback("Dashboard");
            }
        };
    }

    // UI-2: 8 screens, spec §0.2 order — MainWindow wali factories hi
    // (pages shared hain, dobara nahi bane). TuningPage ka root Grid hai,
    // baqi pages UserControl — is liye Func<FrameworkElement>.
    private readonly Dictionary<string, Func<FrameworkElement>> _pages = new()
    {
        ["Dashboard"] = () => new DashboardPage(),
        ["Tuning"] = () => new RSH.App.Views.Pages.TuningPage(),
        ["Setup"] = () => new SetupPage(),
        ["Buttons"] = () => new ButtonsPage(),
        ["Output"] = () => new OutputPage(),
        ["Profiles"] = () => new ProfilesPage(),
        ["Monitor"] = () => new MonitorPage(),
        ["Settings"] = () => new SettingsPage(),
    };

    private static Grid MakePlaceholder(string title)
    {
        var heading = new System.Windows.Controls.TextBlock
        {
            Text = title,
            FontSize = 26,
            FontWeight = FontWeights.Bold,
            FontFamily = new FontFamily("Segoe UI"),
            Foreground = SystemColors.ControlTextBrush,
            Margin = new Thickness(24, 24, 24, 4),
        };
        return new Grid { Children = { heading } };
    }

    private void Nav_Checked(object sender, RoutedEventArgs e)
    {
        if (PageHost is null || sender is not RadioButton { Tag: string tag }) return;
        if (!_pages.TryGetValue(tag, out var make))
        {
            Log.Warning("Navigation requested for unknown page {Page} (classic window)", tag);
            return;
        }
        try
        {
            var page = make();
            PageHost.Content = page;
            _currentPage = page;
            Log.Information("Navigated to {Page} (classic window)", tag);
            StartupStatus.Write($"navigated to {tag} (classic window)");
        }
        catch (Exception ex)
        {
            Log.Error(ex, "Failed to create page {Page} — showing placeholder (classic window)", tag);
            ShowNavigationFallback(tag);
        }
    }

    /// <summary>Page create fail ho to khaali jagah ke bajaye wajah dikhao.</summary>
    private void ShowNavigationFallback(string tag)
    {
        try
        {
            PageHost.Content = MakePlaceholder($"{tag} could not be loaded — see the log in %AppData%\\RSH\\logs or %TEMP%\\RSH\\startup-status.txt");
            _currentPage = null;
        }
        catch (Exception ex)
        {
            Log.Error(ex, "Navigation fallback also failed for {Page} (classic window)", tag);
        }
    }

    /// <summary>
    /// v2.0.4 — classic mode mein FluentWindow wizard NA kholo: wizard bhi
    /// FluentWindow hai, classic path ka maqsad hi Fluent chrome se bachna
    /// hai. Setup screen mein wahi steps hain (Connect → Pins →
    /// Calibrate), is liye wahan le jao — safe degradation, feature
    /// change nahi.
    /// </summary>
    private void ShowWizard()
    {
        Log.Warning("First-run wizard skipped in classic mode — use Setup screen");
        try
        {
            NavSetup.IsChecked = true;
        }
        catch (Exception ex)
        {
            Log.Error(ex, "Could not navigate to Setup after skipping wizard (classic window)");
        }
    }

    private void SetupTray()
    {
        var menu = new System.Windows.Controls.ContextMenu();

        var openItem = new System.Windows.Controls.MenuItem { Header = "Open RSH" };
        openItem.Click += (_, _) => ShowFromTray();
        menu.Items.Add(openItem);

        var outputItem = new System.Windows.Controls.MenuItem
        {
            Header = "Output enabled",
            IsCheckable = true,
            IsChecked = _services.Settings.OutputEnabled,
        };
        outputItem.Click += (_, _) =>
        {
            _services.Settings.OutputEnabled = outputItem.IsChecked;
            _ = _services.SaveSettingsAsync().ContinueWith(
                static t => Log.Error(t.Exception, "Tray output toggle save failed"),
                CancellationToken.None,
                TaskContinuationOptions.OnlyOnFaulted, TaskScheduler.Default);
        };
        menu.Items.Add(outputItem);

        menu.Opened += (_, _) => outputItem.IsChecked = _services.Settings.OutputEnabled;

        menu.Items.Add(new System.Windows.Controls.Separator());

        var exitItem = new System.Windows.Controls.MenuItem { Header = "Exit RSH" };
        exitItem.Click += (_, _) =>
        {
            _reallyExit = true;
            Close();
            Application.Current.Shutdown();
        };
        menu.Items.Add(exitItem);

        _trayIcon = new Wpf.Ui.Tray.Controls.NotifyIcon
        {
            TooltipText = "RSH",
            Menu = menu,
            FocusOnLeftClick = false,
        };
#pragma warning disable CS8622 // WPF-UI Tray ke delegate ki nullability annotation hamare handler se nahi milti — args istemal hi nahi hote
        _trayIcon.LeftDoubleClick += TrayIcon_LeftDoubleClick;
#pragma warning restore CS8622
        try
        {
            _trayIcon.Register();
        }
        catch (Exception ex)
        {
            Log.Warning(ex, "Tray icon registration failed — continuing without tray");
            _trayIcon = null;
            throw;
        }
    }

    private void TrayIcon_LeftDoubleClick(object? sender, RoutedEventArgs e) => ShowFromTray();

    private void ShowFromTray()
    {
        Show();
        if (WindowState == WindowState.Minimized) WindowState = WindowState.Normal;
        Activate();
    }

    protected override void OnClosing(System.ComponentModel.CancelEventArgs e)
    {
        // CloseToTray trap (MainWindow jaisa): sirf tab hide karo jab tray
        // asal mein mojood ho; warna asal exit.
        if (!_reallyExit && _services.Settings.CloseToTray && _trayIcon is not null)
        {
            e.Cancel = true;
            Hide();
            return;
        }
        _uiTimer.Stop();
        _trayIcon?.Unregister();
        base.OnClosing(e);
    }
}
