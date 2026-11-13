using System.Windows;
using System.Windows.Media;

namespace RSH.App.Controls;

/// <summary>
/// Steering gauge: bare gol dairay par needle — wheel jidhar, needle
/// udhar (AngleNorm -1..1 ko ±135° par map). Halka phulka custom render:
/// koi image nahi, koi bhaari control nahi — low-end PC par bhi sust nahi.
/// </summary>
public sealed class SteeringGauge : FrameworkElement
{
    public static readonly DependencyProperty AngleNormProperty =
        DependencyProperty.Register(nameof(AngleNorm), typeof(double), typeof(SteeringGauge),
            new FrameworkPropertyMetadata(0.0, FrameworkPropertyMetadataOptions.AffectsRender));

    public double AngleNorm
    {
        get => (double)GetValue(AngleNormProperty);
        set => SetValue(AngleNormProperty, value);
    }

    private static Brush BrushFromResource(string key, string fallbackHex)
    {
        if (Application.Current?.TryFindResource(key) is Brush b) return b;
        var brush = new SolidColorBrush((Color)ColorConverter.ConvertFromString(fallbackHex));
        brush.Freeze();
        return brush;
    }

    protected override void OnRender(DrawingContext dc)
    {
        double size = Math.Min(ActualWidth, ActualHeight);
        if (size <= 10) return;
        var center = new Point(ActualWidth / 2, ActualHeight / 2);
        double radius = size / 2 - 6;

        var trackBrush = BrushFromResource("ControlStrokeColorDefaultBrush", "#28FFFFFF");
        var accentBrush = BrushFromResource("AccentFillColorDefaultBrush", "#4CC2FF");
        var textBrush = BrushFromResource("TextFillColorSecondaryBrush", "#A0FFFFFF");

        // Bahir wala ring
        dc.DrawEllipse(null, new Pen(trackBrush, 3), center, radius, radius);
        // Center nishan (12 bajay ki taraf chhoti line)
        dc.DrawLine(new Pen(textBrush, 2),
            new Point(center.X, center.Y - radius),
            new Point(center.X, center.Y - radius + 10));

        double angleDeg = Math.Clamp(AngleNorm, -1, 1) * 135.0;
        double rad = (angleDeg - 90) * Math.PI / 180.0;
        var needleEnd = new Point(
            center.X + Math.Cos(rad) * (radius - 14),
            center.Y + Math.Sin(rad) * (radius - 14));

        // Deflection arc: center se needle tak rang (StreamGeometry arc)
        var arc = new StreamGeometry();
        using (var ctx = arc.Open())
        {
            var top = new Point(center.X, center.Y - (radius - 7));
            ctx.BeginFigure(top, false, false);
            ctx.ArcTo(needleEnd == top ? new Point(top.X + 0.01, top.Y) : PointOnCircle(center, radius - 7, angleDeg),
                new Size(radius - 7, radius - 7), angleDeg,
                Math.Abs(angleDeg) > 180, angleDeg >= 0 ? SweepDirection.Clockwise : SweepDirection.Counterclockwise, true, false);
        }
        arc.Freeze();
        dc.DrawGeometry(null, new Pen(accentBrush, 4) { StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round }, arc);

        // Needle + hub
        dc.DrawLine(new Pen(accentBrush, 4) { StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round },
            center, needleEnd);
        dc.DrawEllipse(accentBrush, null, center, 7, 7);
    }

    private static Point PointOnCircle(Point c, double r, double angleDeg)
    {
        double rad = (angleDeg - 90) * Math.PI / 180.0;
        return new Point(c.X + Math.Cos(rad) * r, c.Y + Math.Sin(rad) * r);
    }
}

/// <summary>Pedal bar: 0..1 value, rang wala bhara hua hisa — OnRender se, koi animation overhead nahi.</summary>
public sealed class PedalBar : FrameworkElement
{
    public static readonly DependencyProperty ValueProperty =
        DependencyProperty.Register(nameof(Value), typeof(double), typeof(PedalBar),
            new FrameworkPropertyMetadata(0.0, FrameworkPropertyMetadataOptions.AffectsRender));

    public static readonly DependencyProperty FillProperty =
        DependencyProperty.Register(nameof(Fill), typeof(Brush), typeof(PedalBar),
            new FrameworkPropertyMetadata(Brushes.LimeGreen, FrameworkPropertyMetadataOptions.AffectsRender));

    public double Value
    {
        get => (double)GetValue(ValueProperty);
        set => SetValue(ValueProperty, value);
    }

    public Brush Fill
    {
        get => (Brush)GetValue(FillProperty);
        set => SetValue(FillProperty, value);
    }

    protected override void OnRender(DrawingContext dc)
    {
        if (ActualWidth <= 4 || ActualHeight <= 4) return;
        var track = new Rect(0, 0, ActualWidth, ActualHeight);
        var trackBrush = Application.Current?.TryFindResource("ControlFillColorSecondaryBrush") as Brush ?? Brushes.DimGray;
        dc.DrawRoundedRectangle(trackBrush, null, track, 4, 4);

        double w = ActualWidth * Math.Clamp(Value, 0, 1);
        if (w > 1)
            dc.DrawRoundedRectangle(Fill, null, new Rect(0, 0, w, ActualHeight), 4, 4);
    }
}
