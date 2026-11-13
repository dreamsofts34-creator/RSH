using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using RSH.App.Resources;
using RSH.App.Services;
using RSH.App.Themes;
using RSH.App.ViewModels;
using ScottPlot.Plottables;

namespace RSH.App.Views.Pages;

public partial class MonitorPage : UserControl, IUpdatableView
{
    private readonly DataStreamer _steering;
    private readonly DataStreamer _accel;
    private readonly DataStreamer _brake;
    private ushort _lastSeq;
    private bool _hasSeq;
    private DateTime _lastPlotRefresh = DateTime.MinValue;

    public MonitorPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Monitor;

        var plot = LivePlot.Plot;
        plot.Axes.SetLimitsY(-1.1, 1.1);
        _steering = plot.Add.DataStreamer(400);
        _accel = plot.Add.DataStreamer(400);
        _brake = plot.Add.DataStreamer(400);
        plot.ShowLegend();
        ApplyPlotTheme();

        // Phase F: theme/language badle to plot live dobara range — page
        // dobara khole baghair. Unloaded par unsubscribe (leak nahi).
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        ApplyPlotTheme();
        RSHThemeManager.ThemeChanged += OnThemeChanged;
        UiStrings.Instance.PropertyChanged += OnStringsChanged;
    }

    private void OnUnloaded(object sender, RoutedEventArgs e)
    {
        RSHThemeManager.ThemeChanged -= OnThemeChanged;
        UiStrings.Instance.PropertyChanged -= OnStringsChanged;
    }

    private void OnThemeChanged(object? sender, string theme) => ApplyPlotTheme();

    private void OnStringsChanged(object? sender, PropertyChangedEventArgs e) => ApplyPlotTheme();

    /// <summary>
    /// Phase F: plot ke rang theme + language ke saath live badalte hain.
    /// Pehle rang sirf constructor mein ek dafa parhe jate thay — theme
    /// switch ke baad plot tab tak purana rehta tha jab tak page dobara
    /// na khule (UI-LOCKED pending #8). U-P6 barkarar: ye sirf theme /
    /// language change par Refresh karta hai, har tick par nahi.
    /// High Contrast mein plot OS ke SystemColors istemal karta hai
    /// (background/text/grid), steering line Highlight rang ki hoti hai;
    /// accel/brake data colours rehte hain (legend se pehchan).
    /// </summary>
    private void ApplyPlotTheme()
    {
        var plot = LivePlot.Plot;
        string theme = RSHThemeManager.CurrentTheme;
        bool dark = theme == RSHThemeManager.Dark;
        bool hc = theme == RSHThemeManager.HighContrast;

        // Plot app ke theme ke sath chalta hai (design v2): light
        // theme mein safed plot + halki grid, dark mein gehri neeli.
        string bg = dark ? "#0B1220" : hc ? Hex(SystemColors.WindowColor) : "#FFFFFF";
        string dataBg = dark ? "#131E33" : hc ? Hex(SystemColors.WindowColor) : "#FFFFFF";
        string axes = dark ? "#AEBEDD" : hc ? Hex(SystemColors.WindowTextColor) : "#5A6480";
        string gridMajor = dark ? "#22345A" : hc ? Hex(SystemColors.GrayTextColor) : "#DFE4F2";
        string gridMinor = dark ? "#182642" : hc ? Hex(SystemColors.GrayTextColor) : "#EDF0FA";
        string legendFont = dark ? "#E8EEF7" : hc ? Hex(SystemColors.WindowTextColor) : "#333A52";

        plot.FigureBackground.Color = ScottPlot.Color.FromHex(bg);
        plot.DataBackground.Color = ScottPlot.Color.FromHex(dataBg);
        plot.Axes.Color(ScottPlot.Color.FromHex(axes));
        plot.Grid.MajorLineColor = ScottPlot.Color.FromHex(gridMajor);
        plot.Grid.MinorLineColor = ScottPlot.Color.FromHex(gridMinor);
        plot.Legend.BackgroundColor = ScottPlot.Color.FromHex(dataBg);
        plot.Legend.FontColor = ScottPlot.Color.FromHex(legendFont);
        plot.Legend.OutlineColor = ScottPlot.Color.FromHex(gridMajor);

        _steering.LegendText = UiStrings.GetString("Steering");
        _steering.Color = ScottPlot.Color.FromHex(dark ? "#4CC2FF" : hc ? Hex(SystemColors.HighlightColor) : "#6C4CF1");
        _accel.LegendText = UiStrings.GetString("Accelerator");
        _accel.Color = ScottPlot.Color.FromHex("#3FB950");
        _brake.LegendText = UiStrings.GetString("Brake");
        _brake.Color = ScottPlot.Color.FromHex("#F85149");
        LivePlot.Refresh();
    }

    private static string Hex(System.Windows.Media.Color c) => $"#{c.R:X2}{c.G:X2}{c.B:X2}";

    public void Tick()
    {
        ViewModelStore.Monitor.Tick();
        ViewModelStore.Diagnostics.Tick();

        var services = AppServices.Current;
        // Graph sirf naye packet par aage barhta hai — ruka hua wheel
        // graph ko freeze rakhta hai, jhooti harkat nahi dikhata.
        if (services.Source.TryGetLatest(out var packet, out _, 500))
        {
            if (_hasSeq && packet.Sequence == _lastSeq) return;
            _hasSeq = true;
            _lastSeq = packet.Sequence;

            var state = services.Engine.CurrentState;
            _steering.Add(state.SteeringNorm);
            _accel.Add(state.AccelNorm);
            _brake.Add(state.BrakeNorm);
            _steering.ViewScrollLeft();
            _accel.ViewScrollLeft();
            _brake.ViewScrollLeft();
            // U-P6: ScottPlot redraw ≤ 10 Hz — har packet par Refresh
            // (200 Hz) CPU + GC khata tha. Points har tick add hote
            // hain (ring buffer), redraw sirf 100 ms mein ek dafa.
            if ((DateTime.UtcNow - _lastPlotRefresh) >= TimeSpan.FromMilliseconds(100))
            {
                _lastPlotRefresh = DateTime.UtcNow;
                LivePlot.Refresh();
            }
        }
    }
}
