using System.Windows.Controls;
using ApexLink.App.Services;
using ApexLink.App.ViewModels;
using ScottPlot.Plottables;

namespace ApexLink.App.Views.Pages;

public partial class MonitorPage : UserControl, IUpdatableView
{
    private readonly DataStreamer _steering;
    private readonly DataStreamer _accel;
    private readonly DataStreamer _brake;
    private ushort _lastSeq;
    private bool _hasSeq;

    public MonitorPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Monitor;

        var plot = LivePlot.Plot;

        // Plot app ke theme ke sath chalta hai (design v2): light
        // theme mein safed plot + halki grid, dark mein gehri neeli.
        bool dark = Wpf.Ui.Appearance.ApplicationThemeManager.GetAppTheme()
                    == Wpf.Ui.Appearance.ApplicationTheme.Dark;
        plot.FigureBackground.Color = ScottPlot.Color.FromHex(dark ? "#0B1220" : "#FFFFFF");
        plot.DataBackground.Color = ScottPlot.Color.FromHex(dark ? "#131E33" : "#FFFFFF");
        plot.Axes.Color(ScottPlot.Color.FromHex(dark ? "#AEBEDD" : "#5A6480"));
        plot.Grid.MajorLineColor = ScottPlot.Color.FromHex(dark ? "#22345A" : "#DFE4F2");
        plot.Grid.MinorLineColor = ScottPlot.Color.FromHex(dark ? "#182642" : "#EDF0FA");
        plot.Legend.BackgroundColor = ScottPlot.Color.FromHex(dark ? "#131E33" : "#FFFFFF");
        plot.Legend.FontColor = ScottPlot.Color.FromHex(dark ? "#E8EEF7" : "#333A52");
        plot.Legend.OutlineColor = ScottPlot.Color.FromHex(dark ? "#22345A" : "#DFE4F2");

        plot.Axes.SetLimitsY(-1.1, 1.1);
        _steering = plot.Add.DataStreamer(400);
        _steering.LegendText = "Steering";
        _steering.Color = ScottPlot.Color.FromHex(dark ? "#4CC2FF" : "#6C4CF1");
        _accel = plot.Add.DataStreamer(400);
        _accel.LegendText = "Accelerator";
        _accel.Color = ScottPlot.Color.FromHex("#3FB950");
        _brake = plot.Add.DataStreamer(400);
        _brake.LegendText = "Brake";
        _brake.Color = ScottPlot.Color.FromHex("#F85149");
        plot.ShowLegend();
        LivePlot.Refresh();
    }

    public void Tick()
    {
        ViewModelStore.Monitor.Tick();

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
            LivePlot.Refresh();
        }
    }
}
