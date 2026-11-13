using System.Numerics;
using System.Windows;
using System.Windows.Controls;
using RSH.App.Services;
using RSH.App.ViewModels;
using RSH.Core.Output;
using RSH.Output.HIDMaestro;
using RSH.Output.VJoy;

namespace RSH.App.Views.Pages;

public partial class OutputPage : UserControl, IUpdatableView
{
    public OutputPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Output;
    }

    private void GetVJoy_Click(object sender, RoutedEventArgs e) =>
        WindowsIntegration.OpenVJoyDownloadPage();

    public void Tick()
    {
        var vm = ViewModelStore.Output;
        vm.Tick();
        var services = AppServices.Current;
        if (services is null) return;

        // UI-8: live axis/button values — yehi games dekhte hain (ProcessedState).
        var state = services.Engine.CurrentState;
        SteerBar.Value = Math.Clamp(state.SteeringAxis, 1, 32768);
        AccelBar.Value = Math.Clamp(state.AccelAxis, 1, 32768);
        BrakeBar.Value = Math.Clamp(state.BrakeAxis, 1, 32768);
        SteerValueText.Text = $"{state.SteeringAxis}  ({state.SteeringNorm * 100:F0}%)";
        AccelValueText.Text = $"{state.AccelAxis}  ({state.AccelNorm * 100:F0}%)";
        BrakeValueText.Text = $"{state.BrakeAxis}  ({state.BrakeNorm * 100:F0}%)";
        int pressed = BitOperations.PopCount(state.ButtonsMask)
                    + BitOperations.PopCount(state.ButtonsMaskEx);
        LiveMaskText.Text = $"Buttons pressed: {pressed} · Mask 0x{state.ButtonsMask:X8}";

        // vJoy na mile to warning + install link (R-M13); axis band ho to guide.
        // HIDMaestro active ho to vJoy install text ghalat hai — backend ke mutabiq likho.
        VjoyWarningCard.Visibility = vm.Connected ? Visibility.Collapsed : Visibility.Visible;
        if (!vm.Connected)
        {
            AxisGuideText.Text = services.ActiveOutputBackend == OutputBackendKind.HidMaestro
                ? "HIDMaestro (experimental) is not connected — check the profile id and admin rights, or switch back to vJoy."
                : "Install the vJoy driver first. Then open vJoyConf and enable axes X, Y and Z with at least 2 buttons on the RSH device.";
            return;
        }
        // vJoy ya HIDMaestro — dono ke paas HasAxisX/Y/Z + ButtonCount hain.
        bool hasX, hasY, hasZ;
        int buttonCount;
        switch (services.Output)
        {
            case VJoyOutput vjoy:
                hasX = vjoy.HasAxisX; hasY = vjoy.HasAxisY; hasZ = vjoy.HasAxisZ; buttonCount = vjoy.ButtonCount;
                break;
            case HidMaestroOutput hm:
                hasX = hm.HasAxisX; hasY = hm.HasAxisY; hasZ = hm.HasAxisZ; buttonCount = hm.ButtonCount;
                break;
            default:
                hasX = hasY = hasZ = true; buttonCount = 2;
                break;
        }
        var missing = new List<string>();
        if (!hasX) missing.Add("X");
        if (!hasY) missing.Add("Y");
        if (!hasZ) missing.Add("Z");
        if (missing.Count > 0 || buttonCount < 2)
        {
            string axes = missing.Count > 0 ? $"Axis {string.Join(", ", missing)} is disabled" : "Axes X, Y and Z are enabled";
            string buttons = buttonCount < 2 ? $"only {buttonCount} button(s) enabled" : $"{buttonCount} buttons enabled";
            AxisGuideText.Text = $"{axes}; {buttons}. Open vJoyConf, enable axes X, Y and Z with at least 2 buttons on device {vm.DeviceId}, then press Apply device.";
        }
        else
        {
            AxisGuideText.Text = $"Axes X, Y and Z are enabled with {buttonCount} buttons — games can read the wheel.";
        }
    }
}
