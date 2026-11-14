using System.Net.Http;
using System.Net.Http.Headers;
using System.Text;
using System.Windows;
using System.Windows.Media;
using System.Windows.Threading;
using Microsoft.Win32;
using RSH.App.Services;
using RSH.App.ViewModels;
using RSH.Core.Firmware;
using RSH.Core.Net;
using RSH.Core.Output;
using RSH.Core.Protocol;
using RSH.Core.Processing;
using RSH.Output.HIDMaestro;
using RSH.Output.VJoy;
using Serilog;
using Wpf.Ui.Appearance;
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
        "Check the wiring (pins)",
        "Firmware update (optional)",
        "Calibrate steering — automatically",
        "Calibrate pedals — automatically",
        "Test buttons and output",
    ];

    public WizardWindow(AppServices services)
    {
        _services = services;
        InitializeComponent();

        // v2.0.3 HOTFIX — Win10 par AUTOMATIC classic window (MainWindow
        // jaisa hi): FluentWindow ka ExtendsContentIntoTitleBar chrome
        // band karke plain WPF title bar use karo, TitleBar row 0 +
        // in-content TitleBar collapse (warna double title bar).
        // Classic false ho (Win11+) to yahan kuch nahi hota.
        if (App.ClassicWindowRequested)
        {
            ExtendsContentIntoTitleBar = false;
            // Win10 par Mica unsupported hai — classic mode mein None
            // karo, warna wizard ka background bhi paint nahi hota
            // (WizardWindow.xaml mein WindowBackdropType="Mica" hai).
            if (WindowBackdropType == WindowBackdropType.Mica)
                WindowBackdropType = WindowBackdropType.None;
            WizardTitleBarRow.Height = new GridLength(0);
            WizardTitleBar.Visibility = Visibility.Collapsed;
            Log.Information("Wizard classic window chrome applied (ExtendsContentIntoTitleBar=False, TitleBar collapsed)");
        }

        StepPins.DataContext = ViewModelStore.Setup;
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

        _step = Math.Clamp(step, 0, 7);
        StepWelcome.Visibility = _step == 0 ? Visibility.Visible : Visibility.Collapsed;
        StepVJoy.Visibility = _step == 1 ? Visibility.Visible : Visibility.Collapsed;
        StepLink.Visibility = _step == 2 ? Visibility.Visible : Visibility.Collapsed;
        StepPins.Visibility = _step == 3 ? Visibility.Visible : Visibility.Collapsed;
        StepFirmware.Visibility = _step == 4 ? Visibility.Visible : Visibility.Collapsed;
        StepSteering.Visibility = _step == 5 ? Visibility.Visible : Visibility.Collapsed;
        StepPedals.Visibility = _step == 6 ? Visibility.Visible : Visibility.Collapsed;
        StepTest.Visibility = _step == 7 ? Visibility.Visible : Visibility.Collapsed;
        StepTitle.Text = Titles[_step];
        StepCounter.Text = $"Step {_step + 1} of 8";
        BackButton.IsEnabled = _step > 0;
        NextButton.Content = _step == 7 ? "Finish" : "Next";
        _vjoyPassTicks = 0;
        _linkPassTicks = 0;
        _advanceTicks = -1;
        if (_step == 3) _ = ViewModelStore.Setup.RefreshPinsAsync();
        if (_step == 5) { _steerCal.Reset(); _steerPhase = 0; _advancedFromCalib = false; }
        if (_step == 6) { _pedalCal.Reset(); _pedalsDone = false; _advancedFromCalib = false; }
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
                    ? _services.Output switch
                    {
                        VJoyOutput => $"vJoy feeding on device {_services.Settings.VJoyDeviceId}.",
                        HidMaestroOutput hm => $"HIDMaestro feeding (experimental, profile {hm.ProfileId}).",
                        var other => $"{other.Name} feeding.",
                    }
                    : _services.Engine.LastOutputError ?? "vJoy not connected yet — the engine keeps retrying.";
                VJoyDetails.Text = ok ? OutputViewModel.OutputDetails(_services.Output) : "";
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

            case 5:
                _calib.Tick();
                LiveSteerRun.Text = _calib.HasData ? _calib.LiveSteering.ToString() : "no data";
                SteerMinRun.Text = _calib.SteerMin.ToString();
                SteerCentreRun.Text = _calib.SteerCenter.ToString();
                SteerMaxRun.Text = _calib.SteerMax.ToString();
                FeedSteering();
                break;

            case 6:
                _calib.Tick();
                LiveAccelRun.Text = _calib.HasData ? _calib.LiveAccel.ToString() : "no data";
                LiveBrakeRun.Text = _calib.HasData ? _calib.LiveBrake.ToString() : "no data";
                FeedPedals();
                break;

            case 7:
                FeedTest();
                break;
        }
    }

    // --- Auto-calibration: steering ---
    private void FeedSteering()
    {
        if (_steerPhase == 2) { TickAutoAdvance(6); return; }
        if (!_services.Source.TryGetLatest(out var p, out _, 1000)) return;

        if (_steerPhase == 0)
        {
            // V-C2 follow-up (final polish): AutoCalibrator mV-domain
            // mein kaam karta hai (span gate 660 mV) — raw packet value
            // seedha daalna ghalat calibration save kar deta jab firmware
            // raw-steering fallback mein ho. Intake par hi mV mein lao.
            _steerCal.AddCenterSample(WizardCalibrationIntake.SteeringToMillivolts(p));
            SteerAutoStatus.Text = $"Capturing centre… {_steerCal.CenterSampleCount}/20 samples. Hold the wheel still.";
            if (_steerCal.TryGetCenter(out var center))
            {
                _calib.SteerCenter = center;
                _steerPhase = 1;
                SteerPhaseText.Text = "Centre set. Now turn slowly to full left and full right — pause briefly at each end.";
                SteerAutoStatus.Text = "Capturing endpoints… turn to both locks.";
            }
        }
        else
        {
            _steerCal.AddSteeringRangeSample(WizardCalibrationIntake.SteeringToMillivolts(p));
            if (_steerCal.TryGetSteeringRange(out var min, out var max))
            {
                _calib.SteerMin = min;
                _calib.SteerMax = max;
                _calib.SaveCommand.Execute(null);
                _steerPhase = 2;
                SteerAutoStatus.Text = $"Done ✓ Min {min} · Centre {_calib.SteerCenter} · Max {max} — saved.";
            }
            else
            {
                SteerAutoStatus.Text = "Capturing endpoints… turn to both locks.";
            }
        }
    }

    // --- Auto-calibration: pedals ---
    private void FeedPedals()
    {
        if (_pedalsDone) { TickAutoAdvance(7); return; }
        if (!_services.Source.TryGetLatest(out var p, out _, 1000)) return;

        // V-C2 follow-up (final polish): pedals v1 mein hamesha raw ADC
        // hote hain — AutoCalibrator (mV-domain, span gate 660 mV) ko
        // dene se pehle mV mein lao (CalibrationTuning.PedalToMillivolts
        // jaisa hi — wahan bhi capture-time conversion hoti hai).
        _pedalCal.AddAccelSample(WizardCalibrationIntake.PedalToMillivolts(p.Accel));
        _pedalCal.AddBrakeSample(WizardCalibrationIntake.PedalToMillivolts(p.Brake));
        bool a = _pedalCal.TryGetAccelRange(out var aMin, out var aMax);
        bool b = _pedalCal.TryGetBrakeRange(out var bMin, out var bMax);
        if (a && b)
        {
            _calib.AccelMin = aMin; _calib.AccelMax = aMax;
            _calib.BrakeMin = bMin; _calib.BrakeMax = bMax;
            _calib.SaveCommand.Execute(null);
            _pedalsDone = true;
            PedalAutoStatus.Text = $"Done ✓ Accel {aMin}–{aMax} · Brake {bMin}–{bMax} — saved.";
        }
        else
        {
            PedalAutoStatus.Text = $"Capturing… Accel {(a ? "✓" : "…")} · Brake {(b ? "✓" : "…")} — press the remaining pedal fully.";
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
        // SEC-C-08: dialog filter enforcement nahi — extension yahan check karo.
        if (!string.Equals(System.IO.Path.GetExtension(_fwFilePath), ".bin", StringComparison.OrdinalIgnoreCase))
        {
            FwStatus.Text = "Only .bin firmware files can be uploaded.";
            return;
        }
        // SEC-C-03 / A12: EspIp upload se pehle dobara validate karo —
        // settings.json haath se badli ho to bhi hostname/URL yahan kabhi
        // nahi chalega (warna firmware + password attacker host par jata).
        if (!EspAddress.TryValidate(_services.Settings.EspIp, out var espIp, out string? ipError))
        {
            FwStatus.Text = $"Wheel address is invalid: {ipError} Fix it in Settings.";
            return;
        }
        // SEC-C-04: OTA password kabhi pre-fill nahi hota (PasswordBox,
        // khaali shuru hota hai) — khaali ho to upload hi nahi hota.
        if (string.IsNullOrWhiteSpace(FwPasswordBox.Password))
        {
            FwStatus.Text = "OTA password is empty — enter the board's per-device password (from pairing / first boot).";
            return;
        }
        UploadFwButton.IsEnabled = false;
        FwProgress.Visibility = Visibility.Visible;
        FwStatus.Text = "Checking the file…";
        try
        {
            // SEC-C-09 (TOCTOU) + SEC-C-07 (OOM): file EK dafa parho —
            // size cap parhne se pehle — validation aur upload dono isi
            // in-memory buffer par hote hain, file dobara kabhi nahi parhi jati.
            string path = _fwFilePath;
            var (bytes, validated, readError) = await Task.Run(() => ReadAndValidateFirmware(path));
            if (bytes is null || validated is null)
            {
                FwStatus.Text = $"Firmware reject: {readError ?? validated?.Error ?? "unknown error"}";
                return;
            }
            FwStatus.Text = $"Firmware v{validated.FirmwareVersion} verified (SHA-256 {validated.ImageSha256Hex?.Substring(0, 12)}…) — checking the board version…";
            // SEC-C-05 (anti-downgrade, Phase D): upload se pehle GET_CAPS
            // se board ka current version lo aur image se milao. Session
            // key na ho (na session mein, na DPAPI key store mein — SEC-D-05)
            // to caps nahi milte: gate warning de kar floor check par upload
            // hoti hai, aur upload ke baad confirm lazmi rehta hai (neeche).
            var (caps, capsError) = await _services.BoardSession.GetCapsAsync(espIp.ToString());
            var gate = FirmwareUploadGate.CheckUpload(validated, caps);
            if (!gate.Allowed)
            {
                FwStatus.Text = $"Firmware reject: {gate.Error}";
                return;
            }
            string gateNote = gate.Warning is null ? "" : $" {gate.Warning}";
            if (caps is null)
                FwStatus.Text = $"Firmware v{validated.FirmwareVersion} verified (SHA-256 {validated.ImageSha256Hex?.Substring(0, 12)}…) — board version check failed ({capsError}).{gateNote} Uploading…";
            else
                FwStatus.Text = $"Firmware v{validated.FirmwareVersion} verified (SHA-256 {validated.ImageSha256Hex?.Substring(0, 12)}…) — this upgrades board v{caps.FirmwareVersion}.{gateNote} Uploading — wheel data will pause…";
            using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(90) };
            string auth = Convert.ToBase64String(Encoding.ASCII.GetBytes($"admin:{FwPasswordBox.Password.Trim()}"));
            http.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Basic", auth);
            // SEC-D-12 (CSRF): /update ko per-boot token header chahiye —
            // WebOTA page (GET /) ke <meta name="csrf-token"> se nikalo.
            // Token na mile to legacy board samjho aur header ke baghair
            // bhejo (purana board token mangta hi nahi); naya board token
            // ke baghair POST reject karta hai — GET fail ho to POST wahi
            // error dega, is liye yahan fail nahi karte.
            try
            {
                string page = await http.GetStringAsync(new UriBuilder(Uri.UriSchemeHttp, espIp.ToString()).Uri);
                string? csrf = WebOta.TryExtractCsrfToken(page);
                if (csrf is not null)
                    http.DefaultRequestHeaders.Add(WebOta.CsrfHeaderName, csrf);
            }
            catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException or OperationCanceledException)
            {
                // Board se page nahi mila — POST koshish phir bhi karo.
            }
            using var form = new MultipartFormDataContent();
            var part = new ByteArrayContent(bytes);
            part.Headers.ContentType = new MediaTypeHeaderValue("application/octet-stream");
            form.Add(part, "firmware", System.IO.Path.GetFileName(path));
            // SEC-C-03: URL string interpolation se nahi — UriBuilder,
            // host sirf validated IP hota hai (userinfo/port/path kabhi nahi).
            var uri = new UriBuilder(Uri.UriSchemeHttp, espIp.ToString()) { Path = "/update" }.Uri;
            var resp = await http.PostAsync(uri, form);
            string body = await resp.Content.ReadAsStringAsync();
            if (resp.IsSuccessStatusCode && body.StartsWith("OK", StringComparison.Ordinal))
            {
                // SEC-C-02: HTTP 200 + "OK" akela kamyabi NAHI — board
                // restart ke baad GET_CAPS se naya version confirm ho to
                // hi kamyabi. Confirm na ho to UNKNOWN (kabhi success nahi).
                // "OK" ka matlab hai board ne image ki signature verify
                // ki (SEC-C-06, board-side verify-before-apply, Phase D).
                FwStatus.Text = "Board is restarting — verifying the new version (up to 30 s)…";
                var confirmed = await TryConfirmFirmwareVersionAsync(validated.FirmwareVersion!);
                FwStatus.Text = confirmed.Confirmed
                    ? $"{confirmed.Message} The board verified the image signature."
                    : confirmed.Message;
            }
            else
            {
                FwStatus.Text = $"The board refused it: {body.Trim()} (check the password).";
            }
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException or OperationCanceledException or System.IO.IOException)
        {
            // SEC-C-02: connection tootna KABHI "success" nahi — UNKNOWN hai.
            // Upload ke aakhir mein board restart par connection tootna aam
            // hai, lekin confirm kiye baghair "kamyab" kehna user ko ghalat
            // (purani/aadhi) firmware par aage barhne ka rasta kholta hai.
            FwStatus.Text = "Connection lost — update NOT VERIFIED. The board may be restarting, or the upload may have failed. " +
                "Press Next in a few seconds and check the link + firmware version; if the version is still old, try again.";
        }
        finally
        {
            FwProgress.Visibility = Visibility.Collapsed;
            UploadFwButton.IsEnabled = true;
            // SEC-C-04: password use ke baad saaf karo — string der tak na rakho.
            FwPasswordBox.Password = "";
        }
    }

    /// <summary>
    /// SEC-C-02 (Phase D): upload ke baad board restart ka intezar karo
    /// aur GET_CAPS se naya version confirm karo (30 s tak, har 2 s).
    /// Confirm ho to hi kamyabi — warna UNKNOWN. Board reboot ke baad
    /// nonce badalta hai; BoardSession HELLO reply se nonce khud update
    /// karta hai (counter 0 se), is liye har poll naya client banta hai.
    /// </summary>
    private async Task<PostUploadCheck> TryConfirmFirmwareVersionAsync(Version imageVersion)
    {
        PostUploadCheck last = FirmwareUploadGate.CheckPostUpload(null, imageVersion);
        var deadline = DateTime.UtcNow.AddSeconds(30);
        while (DateTime.UtcNow < deadline)
        {
            var (caps, _) = await _services.BoardSession.GetCapsAsync(_services.Settings.EspIp);
            last = FirmwareUploadGate.CheckPostUpload(caps, imageVersion);
            if (last.Confirmed) return last;
            await Task.Delay(TimeSpan.FromSeconds(2));
        }
        return last;
    }

    /// <summary>
    /// File ek dafa (capped) parho aur usi buffer ko validate karo (A1 +
    /// SEC-C-01/05/07/09). Background thread par chalao (U-P1: UI thread par I/O nahi).
    /// </summary>
    private static (byte[]? Bytes, FirmwareImageValidator.ValidationResult? Validated, string? Error) ReadAndValidateFirmware(string path)
    {
        try
        {
            using var stream = new System.IO.FileStream(path, System.IO.FileMode.Open, System.IO.FileAccess.Read, System.IO.FileShare.Read);
            if (stream.Length > FirmwareImageValidator.MaxFileSizeBytes)
                return (null, null, $"File is too large ({stream.Length:N0} bytes, cap {FirmwareImageValidator.MaxFileSizeBytes:N0}) — it will not be uploaded.");
            if (stream.Length == 0)
                return (null, null, "File is empty.");
            byte[] buffer = new byte[stream.Length];
            int read = 0;
            while (read < buffer.Length)
            {
                int n = stream.Read(buffer, read, buffer.Length - read);
                if (n == 0) break;
                read += n;
            }
            if (read != buffer.Length)
                return (null, null, "The file ended while reading — it looks incomplete.");
            if (stream.Length != buffer.Length)
                return (null, null, "The file changed while reading — choose it again.");
            var validated = FirmwareImageValidator.Validate(buffer);
            if (!validated.IsValid)
                return (null, validated, validated.Error);
            return (buffer, validated, null);
        }
        catch (Exception ex) when (ex is System.IO.IOException or UnauthorizedAccessException or NotSupportedException)
        {
            return (null, null, $"Could not read the file: {ex.Message}");
        }
    }

    // --- Auto-setup ke asal actions: sirf check nahi, kaam bhi ---

    private async void ConnectWifi_Click(object sender, RoutedEventArgs e)
    {
        // SEC-C-04: password khaali ho (abhi pair nahi hua) to netsh ko
        // khaali password mat do — pehle password darj karne ko kaho.
        // SEC-E-04: GetWheelPassword — pehle DPAPI KeyStore, warna
        // in-memory buffer; settings.json se kabhi nahi.
        string password = _services.GetWheelPassword();
        if (string.IsNullOrWhiteSpace(password))
        {
            WifiStatusText.Text = "Wheel password is not set — enter the board's per-device password (from pairing / first boot).";
            return;
        }
        WifiConnectButton.IsEnabled = false;
        WifiStatusText.Text = "Working…";
        var r = await System.Threading.Tasks.Task.Run(
            () => WindowsIntegration.ConnectToWheelWifi(password));
        WifiStatusText.Text = r.Ok ? "Connecting… packets should follow in seconds." : r.Message;
        WifiConnectButton.IsEnabled = true;
    }

    private void OpenVJoyConf_Click(object sender, RoutedEventArgs e)
    {
        VJoyActionStatus.Text = WindowsIntegration.OpenVJoyConfigure()
            ? "vJoy Configure opened — enable Device 1 with axes X, Y, Z + Slider, then come back."
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
        if (_step < 7) { ShowStep(_step + 1); return; }
        Finish();
    }

    private void RefreshPins_Click(object sender, RoutedEventArgs e) =>
        _ = ViewModelStore.Setup.RefreshPinsAsync();

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
        // QA F3 (LOW, U-P1): sync SaveSettings() UI thread par file
        // likhta tha — async rasta; wizard band hone ke baad bhi app
        // chalti rehti hai, is liye background save ke liye waqt hai.
        // Fail ho to log, wizard band hona nahi rukta.
        _ = _services.SaveSettingsAsync().ContinueWith(
            static t => Log.Error(t.Exception, "Wizard finish save failed"),
            CancellationToken.None,
            TaskContinuationOptions.OnlyOnFaulted, TaskScheduler.Default);
        Close();
    }
}

/// <summary>
/// Wizard calibration intake — packet units → mV-domain (V-C2 follow-up,
/// final polish). AutoCalibrator aur CalibrationViewModel ke fields
/// hamesha mV-domain mein hote hain (D10: span gate 660 mV, Save
/// IsMillivolts=true) — is liye conversion intake par hoti hai, save
/// par nahi (save-time conversion ghalat hoti: aadhi-captured mV values
/// dobara convert ho jati — CalibrationTuning.cs mein yahi faisla hai).
/// Alag WPF-free static class hai taake unit tests bina window ke is
/// rasta ko chala sakein (WizardWindow khud FluentWindow hai).
/// </summary>
internal static class WizardCalibrationIntake
{
    /// <summary>
    /// Steering packet value → mV. Flag true ho to pehle se mV hai;
    /// false ho to firmware raw-steering fallback mein hai (inputs.cpp:
    /// mV probe fail par useMv=false) — raw ADC (0..4095) ko
    /// <see cref="InputMapper.RawToMillivolts"/> se mV (0..3300) mein lao.
    /// </summary>
    internal static ushort SteeringToMillivolts(WheelPacket p) =>
        (ushort)(p.SteeringInMillivolts ? p.Steering : InputMapper.RawToMillivolts(p.Steering));

    /// <summary>Pedal packet value → mV. v1 pedals hamesha raw ADC hote hain.</summary>
    internal static ushort PedalToMillivolts(ushort raw) =>
        (ushort)InputMapper.RawToMillivolts(raw);
}
