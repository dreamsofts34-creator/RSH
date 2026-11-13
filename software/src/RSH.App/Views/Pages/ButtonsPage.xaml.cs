using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;
using RSH.App.Services;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class ButtonsPage : UserControl, IUpdatableView
{
    private static readonly Brush PressedFill = Frozen("#6C4CF1");
    private static readonly Brush IdleFill = Frozen("#ECE8FA");
    private static readonly Brush PressedText = Frozen("#FFFFFF");
    private static readonly Brush IdleText = Frozen("#1F1B2E");
    private static readonly Brush MapBorder = Frozen("#6C4CF1");

    private static Brush Frozen(string hex)
    {
        var b = new SolidColorBrush((Color)ColorConverter.ConvertFromString(hex));
        b.Freeze();
        return b;
    }

    private readonly Border[] _cells = new Border[32];
    private readonly TextBlock[] _labels = new TextBlock[32];
    private int _lastGearUp = -1, _lastGearDown = -1;

    public ButtonsPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Buttons;
        // 32 vJoy buttons ka grid — ek dafa banta hai; Tick sirf brush
        // badalta hai (per-frame allocation nahi — U-P7).
        for (int i = 0; i < 32; i++)
        {
            var label = new TextBlock
            {
                Text = (i + 1).ToString(),
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
                FontSize = 13,
                Foreground = IdleText,
            };
            var cell = new Border
            {
                Width = 44,
                Height = 36,
                CornerRadius = new CornerRadius(8),
                Background = IdleFill,
                Margin = new Thickness(0, 0, 6, 6),
                Child = label,
            };
            // UI-12 a11y: display-only cell hai (tab stop nahi), phir bhi
            // screen reader ko naam mile — "vJoy button 7".
            AutomationProperties.SetName(cell, $"vJoy button {i + 1}");
            _cells[i] = cell;
            _labels[i] = label;
            ButtonGrid.Children.Add(cell);
        }
    }

    public void Tick()
    {
        var vm = ViewModelStore.Buttons;
        vm.Tick();
        var services = AppServices.Current;
        if (services is null) return;

        // UI-8: button press → highlight. Tick 30 Hz (U-P2) hai, to
        // highlight ek tick (≤ 33 ms + packet age) mein lagta hai (< 50 ms).
        uint mask = services.Engine.CurrentState.ButtonsMask;
        for (int i = 0; i < 32; i++)
        {
            bool pressed = (mask & (1u << i)) != 0;
            _cells[i].Background = pressed ? PressedFill : IdleFill;
            _labels[i].Foreground = pressed ? PressedText : IdleText;
            _labels[i].FontWeight = pressed ? FontWeights.Bold : FontWeights.Normal;
        }

        if (vm.GearUpButton != _lastGearUp || vm.GearDownButton != _lastGearDown)
        {
            _lastGearUp = vm.GearUpButton;
            _lastGearDown = vm.GearDownButton;
            for (int i = 0; i < 32; i++)
            {
                _cells[i].BorderThickness = new Thickness(0);
                _cells[i].ToolTip = null;
            }
            MarkMapped(_lastGearUp, "Gear up");
            MarkMapped(_lastGearDown, "Gear down");
            MappingLegendText.Text = $"Gear up → vJoy {_lastGearUp} · Gear down → vJoy {_lastGearDown} · Buttons 1–32 live";
        }

        // Encoders: v2.1 packet ke cumulative counts; v1 stream mein nahi hote.
        if (services.Source.TryGetLatestV2(out var p2, out _, long.MaxValue) && p2.EncoderCount > 0)
        {
            Encoder1Text.Text = $"Encoder 1: {p2.Encoders[0]}";
            Encoder2Text.Text = p2.EncoderCount > 1 ? $"Encoder 2: {p2.Encoders[1]}" : "Encoder 2: —";
        }
        else
        {
            Encoder1Text.Text = "Encoder 1: —";
            Encoder2Text.Text = "Encoder 2: —";
        }
    }

    private void MarkMapped(int vjoyButton, string role)
    {
        if (vjoyButton is < 1 or > 32) return;
        var cell = _cells[vjoyButton - 1];
        cell.BorderThickness = new Thickness(2);
        cell.BorderBrush = MapBorder;
        cell.ToolTip = $"{role} → vJoy {vjoyButton}";
    }
}
