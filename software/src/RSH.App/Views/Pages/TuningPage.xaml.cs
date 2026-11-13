using System.Windows.Controls;
using RSH.App.Services;
using RSH.App.ViewModels;
using RSH.App.Views;
using RSH.Core.Pipeline;
using RSH.Core.Processing;
using ScottPlot.DataSources;
using ScottPlot.Plottables;

namespace RSH.App.Views.Pages;

/// <summary>
/// UI-5/UI-6: Tuning page. Root Grid hai (UserControl nahi) taake
/// MainWindow ki Func&lt;Grid&gt; factory baghair tabdeeli ke compile ho.
/// ViewModel page ke paar mehfooz rehta hai (navigation par page dobara
/// banta hai, VM nahi) — ViewModelStore pattern jaisa, lekin
/// ViewModelStore.cs doosre workstream ki file hai, is liye singleton
/// yahin rakha hai. Curve ScottPlot se banti hai; redraw ≤ 10 Hz (U-P6).
/// Curve points reusable arrays mein hain — ScottPlot un ke reference
/// rakhta hai, is liye har tick allocation nahi hoti (U-P7); points
/// sirf tab dobara likhe jate hain jab tuning badle.
/// </summary>
public partial class TuningPage : Grid, IUpdatableView
{
    private const int CurvePoints = 61;

    private static TuningViewModel? _sharedVm;
    private readonly TuningViewModel _vm;
    private readonly Scatter _curveLine;
    private readonly Scatter _diagonal;
    private readonly Scatter _dot;
    private readonly double[] _curveXs = new double[CurvePoints];
    private readonly double[] _curveYs = new double[CurvePoints];
    private readonly double[] _diagXs = new double[2];
    private readonly double[] _diagYs = new double[2];
    private readonly double[] _dotX = new double[1];
    private readonly double[] _dotY = new double[1];
    private ModuleKind _plottedModule = (ModuleKind)(-1);
    private double _plottedDz = double.NaN;
    private double _plottedCurve = double.NaN;
    private double _plottedScale = double.NaN;
    private DateTime _lastRefresh = DateTime.MinValue;

    public TuningPage()
    {
        _vm = _sharedVm ??= new TuningViewModel(AppServices.Current);
        DataContext = _vm;
        InitializeComponent();

        var plot = CurvePlot.Plot;

        // Plot app ke theme ke sath chalta hai (MonitorPage jaisa):
        // light mein safed plot + halki grid, dark mein gehri neeli.
        bool dark = Wpf.Ui.Appearance.ApplicationThemeManager.GetAppTheme()
                    == Wpf.Ui.Appearance.ApplicationTheme.Dark;
        plot.FigureBackground.Color = ScottPlot.Color.FromHex(dark ? "#0B1220" : "#FFFFFF");
        plot.DataBackground.Color = ScottPlot.Color.FromHex(dark ? "#131E33" : "#FFFFFF");
        plot.Axes.Color(ScottPlot.Color.FromHex(dark ? "#AEBEDD" : "#5A6480"));
        plot.Grid.MajorLineColor = ScottPlot.Color.FromHex(dark ? "#22345A" : "#DFE4F2");
        plot.Grid.MinorLineColor = ScottPlot.Color.FromHex(dark ? "#182642" : "#EDF0FA");

        _diagonal = plot.Add.Scatter(new ScatterSourceDoubleArray(_diagXs, _diagYs));
        _diagonal.Color = ScottPlot.Color.FromHex(dark ? "#3A4A6B" : "#C9C1EE");
        _diagonal.LineWidth = 1.5f;
        _diagonal.MarkerSize = 0f;
        _diagonal.LinePattern = ScottPlot.LinePattern.Dashed;

        _curveLine = plot.Add.Scatter(new ScatterSourceDoubleArray(_curveXs, _curveYs));
        _curveLine.Color = ScottPlot.Color.FromHex("#6C4CF1");
        _curveLine.LineWidth = 3f;
        _curveLine.MarkerSize = 0f;

        _dot = plot.Add.Scatter(new ScatterSourceDoubleArray(_dotX, _dotY));
        _dot.Color = ScottPlot.Color.FromHex("#6C4CF1");
        _dot.LineWidth = 0f;
        _dot.MarkerSize = 12f;

        plot.Axes.SetLimitsX(-1.1, 1.1);
        plot.Axes.SetLimitsY(-1.1, 1.1);
        CurvePlot.Refresh();
    }

    public void Tick()
    {
        _vm.Tick();
        UpdatePlot();
    }

    private void UpdatePlot()
    {
        if (!_vm.IsAnalog) return;

        bool pedal = _vm.IsPedal;
        double dz = _vm.DeadzonePct / 100.0;
        double curve = _vm.CurvePct / 100.0;
        double scale = _vm.IsSteering
            ? Math.Clamp(_vm.RotationDeg / 900.0, 0.2, 1.0)
            : 1.0;

        // Curve points sirf tab dobara likho jab tuning badle.
        if (_plottedModule != _vm.SelectedModule || _plottedDz != dz || _plottedCurve != curve || _plottedScale != scale)
        {
            RebuildCurve(pedal, dz, curve, scale);
            _plottedModule = _vm.SelectedModule;
            _plottedDz = dz;
            _plottedCurve = curve;
            _plottedScale = scale;
        }

        // Live dot: asal input (packet) vs asal output (engine state).
        double x = _vm.LiveCurveX;
        double y = _vm.LiveCurveY;
        _dot.IsVisible = !double.IsNaN(x) && !double.IsNaN(y);
        if (_dot.IsVisible)
        {
            _dotX[0] = x;
            _dotY[0] = y;
        }

        // Redraw ≤ 10 Hz (U-P6).
        var now = DateTime.UtcNow;
        if ((now - _lastRefresh).TotalMilliseconds < 100) return;
        _lastRefresh = now;
        CurvePlot.Refresh();
    }

    private void RebuildCurve(bool pedal, double dz, double curve, double scale)
    {
        for (int i = 0; i < CurvePoints; i++)
        {
            // Steering: -1..1; pedals: 0..1. Pipeline ka asal silsila:
            // deadzone -> curve (-> steering par scale DIVIDE, P1/D7).
            double x = pedal ? (double)i / (CurvePoints - 1) : -1.0 + 2.0 * i / (CurvePoints - 1);
            double y = InputMapper.ApplyCurve(InputMapper.ApplyDeadzone(x, dz), curve);
            if (!pedal) y = Math.Clamp(y / scale, -1, 1);
            _curveXs[i] = x;
            _curveYs[i] = y;
        }

        if (pedal)
        {
            _diagXs[0] = 0; _diagYs[0] = 0;
            _diagXs[1] = 1; _diagYs[1] = 1;
            CurvePlot.Plot.Axes.SetLimitsX(-0.05, 1.05);
            CurvePlot.Plot.Axes.SetLimitsY(-0.05, 1.05);
        }
        else
        {
            _diagXs[0] = -1; _diagYs[0] = -1;
            _diagXs[1] = 1; _diagYs[1] = 1;
            CurvePlot.Plot.Axes.SetLimitsX(-1.1, 1.1);
            CurvePlot.Plot.Axes.SetLimitsY(-1.1, 1.1);
        }
    }
}
