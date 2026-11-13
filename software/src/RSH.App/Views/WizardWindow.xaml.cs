using System.Net.Http;
using System.Net.Http.Headers;
using System.Text;
using System.Windows;
using System.Windows.Media;
using System.Windows.Threading;
using RSH.App.Services;
using RSH.App.ViewModels;
using RSH.Core.Processing;
using Microsoft.Win32;
using Wpf.Ui.Controls;

namespace RSH.App.Views;

/// <summary>
/// Advanced Auto-Setup Wizard — industry wali tartib (driver check →
/// link → firmware → auto-calibration → test), jahan har check khud
/// pass hota hai aur wizard khud aage barhta hai. Calibration ke
/// numbers user nahi likhta: AutoCalibrator (Core) centre aur
/// endpoints khud pakarta hai.
/// </summary>
public partial class WizardWindow : FluentWindow
{
    private readonly AppServices _services;
    private readonly CalibrationViewModel _calib = ViewModelStore.Calibration;
    private readonly DispatcherTimer _timer;
    private int _step;

    private int _vjoyPassTicks;
    private int _linkPassTicks;
    private int _advanceTicks = -1;   // calibration mukammal hone ke baad auto-advance ka counter
    private bool _advancedFromCalib;

    private readonly AutoCalibrator _steerCal = new();
    private readonly AutoCalibrator _pedalCal = new();
    private int _steerPhase;          // 0 = centre, 1 = range, 2 = ho gaya
    private bool _pedalsDone;
    private int _seenButtons;         // test step: jo buttons dabaye gaye (bits: up/down/accel/brake)
    private string? _fwFilePath;

    private static readonly Brush OkBrush = Freeze("#3FB950");
    private static readonly Brush BadBrush = Freeze("#F85149");

    private static Brush Freeze(string hex)
    {
        var b = new SolidColorBrush((Color)ColorConverter.ConvertFromString(hex));
        b.Freeze();
        return b;
    }

    private static readonly string[] Titles =
    [
        "Welcome to RSH",
        "Check the virtual joystick (vJoy)",
        "Connect to your wheel",
        "Firmware update (optional)",
        "Calibrate steering — automatically",
        "Calibrate pedals — automatically",
        "Test buttons and output",
    ];

    public WizardWindow(AppServices services)
    {
        _services = services;
        InitializeComponent();
        _timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(100) };
        _timer.Tick += (_, _) => RefreshStep();
        Loaded += (_, _) => { _timer.Start(); ShowStep(0); };
        Closed += (_, _) => _timer.Stop();
    }

    private void ShowStep(int step)
    {
        // F13 FIX: step badalne par Calibration page wala capture peeche
        // chalta nahi rehta — jo chal raha ho, band kar do.
        if (_calib.CapturingSteering) _calib.ToggleCaptureSteeringCommand.Execute(null);
        if (_calib.CapturingPedals) _calib.ToggleCapturePedalsCommand.Execute(null);

        _step = Math.Clamp(step, 0, 6);
        StepWelcome.Visibility = _step == 0 ? Visibility.Visible : Visibility.Collapsed;
        StepVJoy.Visibility = _step == 1 ? Visibility.Visible : Visibility.Collapsed;
        StepLink.Visibility = _step == 2 ? Visibility.Visible : Visibility.Collapsed;
        StepFirmware.Visibility = _step == 3 ? Visibility.Visible : Visibility.Collapsed;
        StepSteering.Visibility = _step == 4 ? Visibility.Visible : Visibility.Collapsed;
        StepPedals.Visibility = _step == 5 ? Visibility.Visible : Visibility.Collapsed;
        StepTest.Visibility = _step == 6 ? Visibility.Visible : Visibility.Collapsed;
        StepTitle.Text = Titles[_step];
        StepCounter.Text = $"Step {_step + 1} of 7";
        BackButton.IsEnabled = _step > 0;
        NextButton.Content = _step == 6 ? "Finish" : "Next";
        _vjoyPassTicks = 0;
        _linkPassTicks = 0;
        _advanceTicks = -1;
        if (_step == 4) { _steerCal.Reset(); _steerPhase = 0; _advancedFromCalib = false; }
        if (_step == 5) { _pedalCal.Reset(); _pedalsDone = false; _advancedFromCalib = false; }
        RefreshStep();
    }

    private void RefreshStep()
    {
        switch (_step)
        {
            case 1:
                bool ok = _services.Engine.OutputConnected;
                VJoyDot.Fill = ok ? OkBrush : BadBrush;
                VJoyStatus.Text = ok
                    ? $"vJoy feeding on device {_services.Settings.VJoyDeviceId}."
                    : _services.Engine.LastOutputError ?? "vJoy not connected yet — the engine keeps retrying.";
                var o = _services.Output;
                VJoyDetails.Text = ok
                    ? $"DLL v{o.DllVersionText} · Driver v{o.DriverVersionText} · Axes X:{(o.HasAxisX ? "yes" : "NO")} Y:{(o.HasAxisY ? "yes" : "NO")} Z:{(o.HasAxisZ ? "yes" : "NO")} · {o.ButtonCount} buttons"
                    : "";
                // Check pass rahe to wizard khud aage barhta hai (1.5 s)
                if (ok && ++_vjoyPassTicks >= 15) ShowStep(2);
                else if (!ok) _vjoyPassTicks = 0;
                break;

            case 2:
                var age = _services.Source.LastValidAgeMs;
                bool live = age is < 1000;
                LinkDot.Fill = live ? OkBrush : BadBrush;
                LinkStatus.Text = live
                    ? $"Receiving {_services.Source.Statistics.CurrentRatePerSecond} packets/sec — the link is alive."
                    : "Waiting for packets…";
                LinkDetails.Text = age is null
                    ? "No packet has ever arrived. Check: wheel powered, PC joined to RSH WiFi."
                    : $"Last packet {age} ms ago.";
                if (live && ++_linkPassTicks >= 15) ShowStep(3);
                else if (!live) _linkPassTicks = 0;
                break;

            case 4:
                _calib.Tick();
                LiveSteerRun.Text = _calib.HasData ? _calib.LiveSteering.ToString() : "no data";
                SteerMinRun.Text = _calib.SteerMin.ToString();
                SteerCentreRun.Text = _calib.SteerCenter.ToString();
                SteerMaxRun.Text = _calib.SteerMax.ToString();
                FeedSteering();
                break;

            case 5:
                _calib.Tick();
                LiveAccelRun.Text = _calib.HasData ? _calib.LiveAccel.ToString() : "no data";
                LiveBrakeRun.Text = _calib.HasData ? _calib.LiveBrake.ToString() : "no data";
                FeedPedals();
                break;

            case 6:
                FeedTest();
                break;
        }
    }

    // --- Auto-calibration: steering ---
    private void FeedSteering()
    {
        if (_steerPhase == 2) { TickAutoAdvance(5); return; }
        if (!_services.Source.TryGetLatest(out var p, out _, 1000)) return;

        if (_steerPhase == 0)
        {
            _steerCal.AddCenterSample(p.Steering);
            SteerAutoStatus.Text = $"Centre pakar raha hai… {_steerCal.CenterSampleCount}/20 samples. Wheel ko bilkul still rakho.";
            if (_steerCal.TryGetCenter(out var center))
            {
                _calib.SteerCenter = center;
                _steerPhase = 1;
                SteerPhaseText.Text = "Centre set ho gaya. Ab wheel ko aahista full left aur full right ghumao — har lock par ek lamha ruko.";
                SteerAutoStatus.Text = "Endpoints pakar raha hai…";
            }
        }
        else
        {
            _steerCal.AddSteeringRangeSample(p.Steering);
            if (_steerCal.TryGetSteeringRange(out var min, out var max))
            {
                _calib.SteerMin = min;
                _calib.SteerMax = max;
                _calib.SaveCommand.Execute(null);
                _steerPhase = 2;
                SteerAutoStatus.Text = $"Ho gaya ✓ Min {min} · Centre {_calib.SteerCenter} · Max {max} — save ho gaya.";
            }
            else
            {
                SteerAutoStatus.Text = "Endpoints pakar raha hai… dono lock tak ghumao.";
            }
        }
    }

    // --- Auto-calibration: pedals ---
    private void FeedPedals()
    {
        if (_pedalsDone) { TickAutoAdvance(6); return; }
        if (!_services.Source.TryGetLatest(out var p, out _, 1000)) return;

        _pedalCal.AddAccelSample(p.Accel);
        _pedalCal.AddBrakeSample(p.Brake);
        bool a = _pedalCal.TryGetAccelRange(out var aMin, out var aMax);
        bool b = _pedalCal.TryGetBrakeRange(out var bMin, out var bMax);
        if (a && b)
        {
            _calib.AccelMin = aMin; _calib.AccelMax = aMax;
            _calib.BrakeMin = bMin; _calib.BrakeMax = bMax;
            _calib.SaveCommand.Execute(null);
            _pedalsDone = true;
            PedalAutoStatus.Text = $"Ho gaya ✓ Accel {aMin}–{aMax} · Brake {bMin}–{bMax} — save ho gaya.";
        }
        else
        {
            PedalAutoStatus.Text = $"Pakarna jaari hai… Accel {(a ? "✓" : "…")} · Brake {(b ? "✓" : "…")} — jo baqi hai use poora dabao.";
        }
    }

    // --- Test step: buttons + zinda output ---
    private void FeedTest()
    {
        if (_services.Source.TryGetLatest(out var p, out _, 1000))
        {
            if (p.GearUp) _seenButtons |= 1;
            if (p.GearDown) _seenButtons |= 2;
            if (p.AccelButton) _seenButtons |= 4;
            if (p.BrakeButton) _seenButtons |= 8;
        }
        BtnUpRun.Text = (_seenButtons & 1) != 0 ? "pressed ✓" : "—";
        BtnDownRun.Text = (_seenButtons & 2) != 0 ? "pressed ✓" : "—";
        BtnAccelRun.Text = (_seenButtons & 4) != 0 ? "pressed ✓" : "—";
        BtnBrakeRun.Text = (_seenButtons & 8) != 0 ? "pressed ✓" : "—";

        var s = _services.Engine.CurrentState;
        OutSteerRun.Text = s.SteeringAxis.ToString();
        OutAccelRun.Text = s.AccelAxis.ToString();
        OutBrakeRun.Text = s.BrakeAxis.ToString();

        int seen = ((_seenButtons & 1) != 0 ? 1 : 0) + ((_seenButtons & 2) != 0 ? 1 : 0)
                 + ((_seenButtons & 4) != 0 ? 1 : 0) + ((_seenButtons & 8) != 0 ? 1 : 0);
        SummaryText.Text =
            $"vJoy {(_services.Engine.OutputConnected ? "✓" : "…")} · Link {(_services.Source.LastValidAgeMs is < 1000 ? "✓" : "…")} · " +
            $"Steering {(_steerPhase == 2 ? "✓" : "…")} · Pedals {(_pedalsDone ? "✓" : "…")} · Buttons {seen}/4";
    }

    private void TickAutoAdvance(int nextStep)
    {
        if (_advancedFromCalib) return;
        if (_advanceTicks < 0) _advanceTicks = 0;
        if (++_advanceTicks >= 10) { _advancedFromCalib = true; ShowStep(nextStep); }
    }

    // --- Firmware step ---
    private void ChooseFw_Click(object sender, RoutedEventArgs e)
    {
        var dlg = new OpenFileDialog { Filter = "Firmware (*.bin)|*.bin", Title = "Choose firmware .bin" };
        if (dlg.ShowDialog() == true)
        {
            _fwFilePath = dlg.FileName;
            FwFileName.Text = dlg.FileName;
            UploadFwButton.IsEnabled = true;
            FwStatus.Text = "";
        }
    }

    private async void UploadFw_Click(object sender, RoutedEventArgs e)
    {
        if (_fwFilePath is null) return;
        UploadFwButton.IsEnabled = false;
        FwProgress.Visibility = Visibility.Visible;
        FwStatus.Text = "Upload ho raha hai — wheel ka data is doran rukega…";
        try
        {
            byte[] bytes = await System.IO.File.ReadAllBytesAsync(_fwFilePath);
            using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(90) };
            string auth = Convert.ToBase64String(Encoding.ASCII.GetBytes($"admin:{FwPasswordBox.Text.Trim()}"));
            http.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Basic", auth);
            using var form = new MultipartFormDataContent();
            var part = new ByteArrayContent(bytes);
            part.Headers.ContentType = new MediaTypeHeaderValue("application/octet-stream");
            form.Add(part, "firmware", System.IO.Path.GetFileName(_fwFilePath));
            var resp = await http.PostAsync($"http://{_services.Settings.EspIp}/update", form);
            string body = await resp.Content.ReadAsStringAsync();
            FwStatus.Text = resp.IsSuccessStatusCode && body.StartsWith("OK", StringComparison.Ordinal)
                ? "Update ho gaya ✓ Board restart ho raha hai — Link dobara zinda hone mein chand second lagenge."
                : $"Board ne inkar kiya: {body.Trim()} (password check karo — default rsh-ota hai).";
        }
        catch (Exception)
        {
            // Upload ke aakhir mein board restart par connection tootna
            // aam hai — fail aur success ka farq yahan se pakarna mushkil,
            // is liye wazeh likha hai ke dobara check karo.
            FwStatus.Text = "Connection band ho gaya — aksar is ka matlab hai board restart ho raha hai (update kamyab). Kuch second baad Next karke link check karo; na ho to dobara koshish karo.";
        }
        finally
        {
            FwProgress.Visibility = Visibility.Collapsed;
            UploadFwButton.IsEnabled = true;
        }
    }

    // --- Auto-setup ke asal actions: sirf check nahi, kaam bhi ---

    private async void ConnectWifi_Click(object sender, RoutedEventArgs e)
    {
        WifiConnectButton.IsEnabled = false;
        WifiStatusText.Text = "Working…";
        var r = await System.Threading.Tasks.Task.Run(
            () => WindowsIntegration.ConnectToWheelWifi(_services.Settings.WheelWifiPassword));
        WifiStatusText.Text = r.Ok ? "Connecting… packets should follow in seconds." : r.Message;
        WifiConnectButton.IsEnabled = true;
    }

    private void OpenVJoyConf_Click(object sender, RoutedEventArgs e)
    {
        VJoyActionStatus.Text = WindowsIntegration.OpenVJoyConfigure()
            ? "vJoy Configure opened — enable Device 1 with axes X, Y, Z, then come back."
            : "vJoy Configure not found — install the driver first (button on the right).";
    }

    private void GetVJoy_Click(object sender, RoutedEventArgs e) => WindowsIntegration.OpenVJoyDownloadPage();

    private void RestartSteer_Click(object sender, RoutedEventArgs e)
    {
        _steerCal.Reset();
        _steerPhase = 0;
        _advancedFromCalib = false;
        _advanceTicks = -1;
        SteerPhaseText.Text = "Leave the wheel dead centre and still for two seconds — the centre sets itself.";
        SteerAutoStatus.Text = "";
    }

    private void RestartPedals_Click(object sender, RoutedEventArgs e)
    {
        _pedalCal.Reset();
        _pedalsDone = false;
        _advancedFromCalib = false;
        _advanceTicks = -1;
        PedalAutoStatus.Text = "";
    }

    private void Back_Click(object sender, RoutedEventArgs e) => ShowStep(_step - 1);

    private void Next_Click(object sender, RoutedEventArgs e)
    {
        if (_step < 6) { ShowStep(_step + 1); return; }
        Finish();
    }

    private void Skip_Click(object sender, RoutedEventArgs e)
    {
        // F13 FIX: Skip wizard ko "completed" NAHI kehta — user ne setup
        // kiya hi nahi, to wizard agli dafa phir pesh hoga. Sirf band karo.
        Close();
    }

    private void Finish()
    {
        _services.Settings.WizardCompleted = true;
        _services.Settings.FirstRunDone = true;
        _services.SaveSettings();
        Close();
    }
}
