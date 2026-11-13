using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Shapes;
using RSH.App.Services;
using RSH.App.ViewModels;
using RSH.Core.Processing;

namespace RSH.App.Views.Pages;

public partial class ProcessingPage : UserControl, IUpdatableView
{
    private readonly ProcessingViewModel _vm = ViewModelStore.Processing;
    private double _lastCurve = double.NaN;
    private readonly Ellipse _dot = new()
    {
        Width = 14,
        Height = 14,
        Fill = new SolidColorBrush(Color.FromRgb(0x1F, 0x1B, 0x2E)),
    };

    public ProcessingPage()
    {
        InitializeComponent();
        DataContext = _vm;
        Loaded += (_, _) => DrawCurve(force: true);
        CurveCanvas.SizeChanged += (_, _) => DrawCurve(force: true);
    }

    public void Tick()
    {
        DrawCurve(force: false);
        MoveDot();
    }

    private void DrawCurve(bool force)
    {
        double strength = _vm.CurvePercent / 100.0;
        if (!force && Math.Abs(strength - _lastCurve) < 0.001) return;
        _lastCurve = strength;

        double w = CurveCanvas.ActualWidth;
        double h = CurveCanvas.ActualHeight;
        if (w < 50 || h < 50) return;

        CurveCanvas.Children.Clear();
        var diagBrush = new SolidColorBrush(Color.FromRgb(0xC9, 0xC1, 0xEE));
        var accent = new SolidColorBrush(Color.FromRgb(0x6C, 0x4C, 0xF1));

        CurveCanvas.Children.Add(new Line { X1 = 0, Y1 = h, X2 = w, Y2 = 0, Stroke = diagBrush, StrokeThickness = 2, StrokeDashArray = [4, 4] });

        var points = new PointCollection();
        for (int i = 0; i <= 60; i++)
        {
            double x = -1.0 + i * (2.0 / 60);
            double y = InputMapper.ApplyCurve(x, strength);
            points.Add(new Point((x + 1) / 2 * w, h - (y + 1) / 2 * h));
        }
        CurveCanvas.Children.Add(new Polyline { Points = points, Stroke = accent, StrokeThickness = 4, StrokeLineJoin = PenLineJoin.Round });
        CurveCanvas.Children.Add(_dot);
        MoveDot();
    }

    private void MoveDot()
    {
        double w = CurveCanvas.ActualWidth;
        double h = CurveCanvas.ActualHeight;
        if (w < 50 || h < 50) return;

        double y = Math.Clamp(AppServices.Current.Engine.CurrentState.SteeringNorm, -1.0, 1.0);
        double strength = _vm.CurvePercent / 100.0;
        double lo = -1.0, hi = 1.0;
        for (int i = 0; i < 24; i++)
        {
            double mid = (lo + hi) / 2;
            if (InputMapper.ApplyCurve(mid, strength) < y) lo = mid; else hi = mid;
        }
        double x = (lo + hi) / 2;
        Canvas.SetLeft(_dot, (x + 1) / 2 * w - 7);
        Canvas.SetTop(_dot, h - (y + 1) / 2 * h - 7);
    }
}
