using System.Globalization;
using System.Windows;
using System.Windows.Data;
using System.Windows.Media;
using Wpf.Ui.Controls;

namespace ApexLink.App.Controls;

/// <summary>true = sabz (theek), false = surkh/boora (masla) — status binders ke liye.</summary>
public sealed class BoolToStatusBrushConverter : IValueConverter
{
    private static readonly Brush Ok = Frozen("#3FB950");
    private static readonly Brush Bad = Frozen("#F85149");

    private static Brush Frozen(string hex)
    {
        var b = new SolidColorBrush((Color)ColorConverter.ConvertFromString(hex));
        b.Freeze();
        return b;
    }

    public object Convert(object value, Type targetType, object parameter, CultureInfo culture) =>
        value is true ? Ok : Bad;

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) =>
        throw new NotSupportedException();
}

public sealed class BoolToVisibilityConverter : IValueConverter
{
    // ConverterParameter="Invert" do to ulta: true = Collapsed, false = Visible
    // (F11: "koi data nahi" wale banner ke liye).
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture)
    {
        bool b = value is true;
        if (parameter as string == "Invert") b = !b;
        return b ? Visibility.Visible : Visibility.Collapsed;
    }

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) =>
        throw new NotSupportedException();
}

/// <summary>0..1 ya -1..1 ko فیصد text mein ("42%").</summary>
public sealed class NormToPercentConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture) =>
        value is double d ? $"{Math.Abs(d) * 100:F0}%" : "0%";

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) =>
        throw new NotSupportedException();
}

/// <summary>
/// AUDIT FIX (F10): steering ke liye simat ke saath فیصد — NormToPercent
/// Math.Abs use karta tha, to left full lock aur right full lock dono
/// "100%" dikhte thay. Ab "L 42%" / "R 42%" / "0%" (manfi = left,
/// InputMapper ka convention).
/// </summary>
public sealed class SteeringPercentConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture)
    {
        if (value is not double d) return "0%";
        double pct = Math.Abs(d) * 100;
        if (pct < 0.5) return "0%";
        return d < 0 ? $"L {pct:F0}%" : $"R {pct:F0}%";
    }

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) =>
        throw new NotSupportedException();
}

/// <summary>
/// F12: capture chal raha ho to button ka label "Stop capture", warna
/// ConverterParameter mein diya gaya shuru label.
/// </summary>
public sealed class CaptureButtonTextConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture) =>
        value is true ? "Stop capture" : parameter as string ?? "Start capture";

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) =>
        throw new NotSupportedException();
}

/// <summary>F12: capture active ho to button Primary dikhe, warna Secondary.</summary>
public sealed class CaptureButtonAppearanceConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture) =>
        value is true ? ControlAppearance.Primary : ControlAppearance.Secondary;

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) =>
        throw new NotSupportedException();
}
