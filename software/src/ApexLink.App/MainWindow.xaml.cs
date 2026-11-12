using System.Windows;
using System.Windows.Threading;
using ApexLink.App.Services;
using ApexLink.App.ViewModels;
using ApexLink.App.Views;
using ApexLink.App.Views.Pages;
using Wpf.Ui.Controls;

namespace ApexLink.App;

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
        // 200 Hz apni jagah, UI usay sust nahi karti.
        _uiTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(33) };
        _uiTimer.Tick += (_, _) =>
        {
            if (!IsVisible || WindowState == WindowState.Minimized) return;
            (_currentPage as IUpdatableView)?.Tick();
        };

        ViewModelStore.Dashboard.RequestWizard = ShowWizard;
        ViewModelStore.Settings.RequestWizard = ShowWizard;
        ViewModelStore.Setup.RequestHome = () => NavHome.IsChecked = true;
        ViewModelStore.Dashboard.RequestSetup = () => NavSetup.IsChecked = true;

        Loaded += (_, _) =>
        {
            SetupTray();
            _uiTimer.Start();
            NavHome.IsChecked = true;
        };
    }

    private readonly Dictionary<string, Func<System.Windows.Controls.UserControl>> _pages = new()
    {
        ["Home"] = () => new DashboardPage(),
        ["Setup"] = () => new SetupPage(),
        ["Tune"] = () => new ProcessingPage(),
        ["Buttons"] = () => new ButtonsPage(),
        ["Profiles"] = () => new ProfilesPage(),
        ["Manual"] = () => new CalibrationPage(),
        ["Monitor"] = () => new MonitorPage(),
        ["Output"] = () => new OutputPage(),
        ["Connection"] = () => new ConnectionPage(),
        ["Diagnostics"] = () => new DiagnosticsPage(),
        ["Settings"] = () => new SettingsPage(),
    };

    private void Nav_Checked(object sender, RoutedEventArgs e)
    {
        if (PageHost is null || sender is not System.Windows.Controls.RadioButton { Tag: string tag }) return;
        if (!_pages.TryGetValue(tag, out var make)) return;
        var page = make();
        PageHost.Content = page;
        _currentPage = page;
    }

    private void Advanced_Click(object sender, RoutedEventArgs e) =>
        AdvPanel.Visibility = AdvPanel.Visibility == Visibility.Visible ? Visibility.Collapsed : Visibility.Visible;

    private void ShowWizard()
    {
        var wizard = new WizardWindow(_services) { Owner = this };
        wizard.ShowDialog();
    }

    private void SetupTray()
    {
        var menu = new System.Windows.Controls.ContextMenu();

        var openItem = new System.Windows.Controls.MenuItem { Header = "Open ApexLink" };
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

        var exitItem = new System.Windows.Controls.MenuItem { Header = "Exit ApexLink" };
        exitItem.Click += (_, _) =>
        {
            _reallyExit = true;
            Close();
            Application.Current.Shutdown();
        };
        menu.Items.Add(exitItem);

        _trayIcon = new Wpf.Ui.Tray.Controls.NotifyIcon
        {
            TooltipText = "ApexLink",
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
