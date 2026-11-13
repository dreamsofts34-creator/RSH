using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Media;
using System.Windows.Threading;
using RSH.App.Services;
using RSH.App.Themes;
using RSH.App.ViewModels;
using RSH.App.Views;
using RSH.App.Views.Pages;
using Serilog;
using Wpf.Ui.Controls;

namespace RSH.App;

public partial class MainWindow : FluentWindow
{
    private readonly AppServices _services;
    private readonly DispatcherTimer _uiTimer;
    private object? _currentPage;
    private Wpf.Ui.Tray.Controls.NotifyIcon? _trayIcon;
    private bool _reallyExit;

    public MainWindow(AppServices services)
    {
        _services = services;
        InitializeComponent();

        // v2.0.2 diagnostic flag --classic-window, v2.0.3 Win10 par
        // AUTOMATIC: FluentWindow ka ExtendsContentIntoTitleBar chrome
        // band karke plain WPF title bar use karo (WPF-UI #1594 jaisa
        // blank-window case). TitleBar row ki height 0 + in-content
        // TitleBar collapse — warna OS title bar + in-content TitleBar
        // double dikhenge. WindowBackdropType ko nahi chherte — None hi
        // rehta hai. Classic false ho (Win11+) to yahan kuch nahi hota.
        if (App.ClassicWindowRequested)
        {
            ExtendsContentIntoTitleBar = false;
            TitleBarRow.Height = new GridLength(0);
            WindowTitleBar.Visibility = Visibility.Collapsed;
            Log.Information("Classic window chrome applied (ExtendsContentIntoTitleBar=False, TitleBar collapsed)");
        }

        // v2.0.3 HOTFIX — Win10 par AUTOMATIC plain navigation (classic
        // ke saath; user koi flag test nahi karega): sidebar ko
        // original-style saada banao. --plain-nav force karta hai,
        // --fluent-window isay bhi off karta hai. Classic se independent
        // faisla hai — is liye alag if, classic block ke baad.
        if (App.PlainNavigationRequested) ApplyPlainNavigation();

        Log.Information("Main window created (InitializeComponent done)");

        // UI refresh sirf 30 Hz, sirf jab window samne ho — pipeline
        // 200 Hz apni jagah, UI usay sust nahi karti (U-P2).
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
            // v2.0.1 HOTFIX (P0 blank window): pehle ye ek hi lambda tha —
            // kisi ek step mein exception (khaas taur par SetupTray mein
            // NotifyIcon.Register fail) baaqi sab maar deti thi: na tray,
            // na UI timer, na pehli navigation — aur DispatcherUnhandled-
            // Exception use khamoshi se nigal kar app ko khaali shell ki
            // tarah chalata rehta tha. Ab har step alag try/catch mein hai
            // aur har step log hota hai, taake agla report bataye ke kahan
            // atka tha.
            try
            {
                // Saved theme (Settings → Appearance, UI-11) RSH palettes par lagao.
                RSHThemeManager.ApplyTheme(_services.Settings.Theme);
                Log.Information("Theme applied on window load: {Theme}", _services.Settings.Theme);
            }
            catch (Exception ex)
            {
                Log.Error(ex, "Theme apply failed on window load (continuing)");
            }

            try
            {
                SetupTray();
                Log.Information("Tray icon created");
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
                Log.Error(ex, "UI timer failed to start");
            }

            try
            {
                NavDashboard.IsChecked = true;
                Log.Information("First navigation requested: Dashboard");
            }
            catch (Exception ex)
            {
                Log.Error(ex, "First navigation failed");
                ShowNavigationFallback("Dashboard");
            }
        };
    }

    /// <summary>v2.0.3 HOTFIX — plain navigation: sidebar ko original-style
    /// saada banao (literal text + system brushes + default RadioButton
    /// style). UiStrings Content/Name bindings aur RSHNavStyle /
    /// DynamicResource brushes hatao — Win10 blank-window ka doosra
    /// suspect. 8/8 nav buttons rehte hain (Tag + Checked handler
    /// barkarar — parity): sirf un ki presentation badalti hai.
    /// Har step alag try/catch mein — ek element na mile to baaqi phir
    /// bhi hon; poori method kabhi startup nahi todti.</summary>
    private void ApplyPlainNavigation()
    {
        try
        {
            RadioButton[] navButtons = [NavDashboard, NavTuning, NavSetup, NavButtons, NavOutput, NavProfiles, NavMonitor, NavSettings];
            foreach (var btn in navButtons)
            {
                try
                {
                    if (btn is null) continue;
                    BindingOperations.ClearBinding(btn, ContentControl.ContentProperty);
                    BindingOperations.ClearBinding(btn, AutomationProperties.NameProperty);
                    if (btn.Tag is string tag)
                    {
                        btn.Content = tag;
                        AutomationProperties.SetName(btn, tag);
                    }
                    btn.Style = null;
                    btn.Foreground = SystemColors.ControlTextBrush;
                }
                catch (Exception ex)
                {
                    Log.Warning(ex, "Plain navigation: nav button {Button} could not be simplified — continuing", btn?.Name ?? "?");
                }
            }

            try
            {
                if (SidebarBorder is not null)
                {
                    SidebarBorder.Background = SystemColors.WindowBrush;
                    SidebarBorder.BorderBrush = SystemColors.ControlDarkBrush;
                }
            }
            catch (Exception ex)
            {
                Log.Warning(ex, "Plain navigation: sidebar brushes could not be simplified — continuing");
            }

            try
            {
                if (LogoBadge is not null)
                    LogoBadge.Background = new SolidColorBrush(Color.FromRgb(0x6C, 0x4C, 0xF1));
                if (LogoLetter is not null)
                {
                    LogoLetter.Text = "R";
                    LogoLetter.Foreground = Brushes.White;
                }
                if (LogoTitle is not null)
                {
                    LogoTitle.Text = "RSH";
                    LogoTitle.Foreground = SystemColors.ControlTextBrush;
                }
                if (LogoSubtitle is not null)
                {
                    LogoSubtitle.Text = "Real Sim Hub";
                    LogoSubtitle.Foreground = SystemColors.ControlTextBrush;
                }
            }
            catch (Exception ex)
            {
                Log.Warning(ex, "Plain navigation: logo could not be simplified — continuing");
            }

            try
            {
                Background = SystemColors.WindowBrush;
                FontFamily = new FontFamily("Segoe UI");
                PageHost.ClearValue(Control.ForegroundProperty);
            }
            catch (Exception ex)
            {
                Log.Warning(ex, "Plain navigation: window brushes could not be simplified — continuing");
            }

            Log.Information("Plain navigation applied (original-style sidebar, no UiStrings/DynamicResource bindings)");
        }
        catch (Exception ex)
        {
            Log.Error(ex, "Plain navigation failed — continuing with modern sidebar");
        }
    }

    // UI-2: 8 screens, spec §0.2 order — har entry asal page hai
    // (UI-3 Dashboard, UI-5/UI-6 Tuning, UI-7 Setup, UI-8 Buttons +
    // Output, UI-9 Profiles, UI-10 Monitor, UI-11 Settings).
    // Tuning ki factory Workstream 2 ki hai — usay mat chhedo.
    // Dict Func<FrameworkElement> hai (Func<UserControl> NAHI):
    // TuningPage ka root Grid hai, baqi pages UserControl — dono
    // FrameworkElement hain, is liye yahi type dono ko leta hai.
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
            FontFamily = new FontFamily("Inter, Segoe UI"),
            Margin = new Thickness(24, 24, 24, 4),
        };
        heading.SetResourceReference(System.Windows.Controls.TextBlock.ForegroundProperty, "RSHTextBrush");
        return new Grid { Children = { heading } };
    }

    private void Nav_Checked(object sender, RoutedEventArgs e)
    {
        if (PageHost is null || sender is not RadioButton { Tag: string tag }) return;
        // v2.0.1 HOTFIX (P0): page bante hue exception pehle yahin se
        // DispatcherUnhandledException mein jati thi aur khamoshi se nigal
        // li jati thi — PageHost khaali reh jata tha (blank content).
        // Ab failure log hoti hai aur placeholder dikhta hai.
        if (!_pages.TryGetValue(tag, out var make))
        {
            Log.Warning("Navigation requested for unknown page {Page}", tag);
            return;
        }
        try
        {
            var page = make();
            PageHost.Content = page;
            _currentPage = page;
            Log.Information("Navigated to {Page}", tag);
        }
        catch (Exception ex)
        {
            Log.Error(ex, "Failed to create page {Page} — showing placeholder", tag);
            ShowNavigationFallback(tag);
        }
    }

    /// <summary>v2.0.1: page create fail ho to khaali jagah ke bajaye wajah dikhao.</summary>
    private void ShowNavigationFallback(string tag)
    {
        try
        {
            PageHost.Content = MakePlaceholder($"{tag} could not be loaded — see the log in %AppData%\\RSH\\logs");
            _currentPage = null;
        }
        catch (Exception ex)
        {
            Log.Error(ex, "Navigation fallback also failed for {Page}", tag);
        }
    }

    private void ShowWizard()
    {
        var wizard = new WizardWindow(_services) { Owner = this };
        wizard.ShowDialog();
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
            _services.SaveSettings();
        };
        menu.Items.Add(outputItem);

        // AUDIT FIX (F2): checkbox sirf setup par ek dafa initialize hota
        // tha — Dashboard se output band karo to tray purana (checked)
        // dikhata rehta tha. Ab menu khulte hi settings se taaza hota hai.
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
        // v2.0.1 HOTFIX (P0): Register() native shell call hai — fail ho
        // (tray na mile) to poora startup nahi marna chahiye. Caller
        // (Loaded) isay log karta hai; yahan sirf tray ko null karo taake
        // OnClosing mein Unregister par kuch ghalat na ho.
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
        // v2.0.2 HOTFIX — CloseToTray trap: pehle CloseToTray=true hone par
        // hamesha hide hota tha, chahe tray icon bana hi na ho (v2.0.0/2.0.1
        // mein Loaded mar jane par tray kabhi banta hi nahi tha) — X dabane
        // par app asal mein band hone ke bajaye invisible process ban kar
        // zinda rehti thi. Ab sirf tab hide karo jab tray asal mein mojood
        // ho (_trayIcon != null); warna asal exit (tray null hone par
        // _trayIcon?.Unregister() pehle se null-safe hai).
        // Default CloseToTray=true rehta hai — koi UX change nahi.
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
