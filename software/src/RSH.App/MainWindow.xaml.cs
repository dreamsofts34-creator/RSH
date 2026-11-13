using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using RSH.App.Services;
using RSH.App.Themes;
using RSH.App.ViewModels;
using RSH.App.Views;
using RSH.App.Views.Pages;
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
            // Saved theme (Settings → Appearance, UI-11) RSH palettes par lagao.
            RSHThemeManager.ApplyTheme(_services.Settings.Theme);
            SetupTray();
            _uiTimer.Start();
            NavDashboard.IsChecked = true;
        };
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
        if (!_pages.TryGetValue(tag, out var make)) return;
        var page = make();
        PageHost.Content = page;
        _currentPage = page;
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
        _trayIcon.Register();
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
        if (!_reallyExit && _services.Settings.CloseToTray)
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
