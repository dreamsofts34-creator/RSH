// Phase H WS5 — transport-switch UI wiring (Wi-Fi ↔ USB-serial, D26).
// WPF-free: SettingsViewModel WPF hai (net10.0-windows) aur Linux test
// project se reference nahi ho sakta — is liye toggle ki asal logic
// yahin hai. PinMapStore.cs wali precedent: ye file RSH.Tests mein
// link hoti hai (RSH.Tests.csproj), taake ViewModel-level tests fake
// target se Linux par chal sakein. SettingsViewModel sirf isay expose
// karta hai, SettingsPage.xaml is se bind karta hai.
//
// Fail-closed (A5 pattern, autostart toggle jaisa): switch fail ho to
// toggle asal state (Wi-Fi) par wapas aata hai + saaf wajah StatusText
// mein — khamoshi se serial ON kabhi nahi dikhta.
using System.ComponentModel;
using RSH.Core.Transport;

namespace RSH.App.Services;

/// <summary>
/// Transport switch ka target — <see cref="AppServices"/> ye implement
/// karta hai (us ke SwitchToSerialAsync / SwitchToWifi /
/// DetectAndSwitchToSerialAsync pehle se maujood hain, koi nayi logic
/// nahi); tests yahan fake dete hain.
/// </summary>
public interface ITransportSwitchTarget
{
    TransportKind ActiveTransport { get; }
    string? SerialPortName { get; }
    Task<(bool Ok, string? Error)> SwitchToSerialAsync(string portName, CancellationToken cancellationToken = default);
    void SwitchToWifi();
    Task<(bool Ok, string? Error)> DetectAndSwitchToSerialAsync(CancellationToken cancellationToken = default);
}

/// <summary>
/// Settings → Connection card ka state + logic. XAML
/// <c>Transport.IsUsbSerial</c> (two-way toggle),
/// <c>Transport.SelectedPortName</c> / <c>Transport.AvailablePorts</c>
/// (port dropdown) aur <c>Transport.StatusText</c> se bind karta hai.
/// Toggle sirf dikhawa nahi — <see cref="SetUseUsbSerialAsync"/> asal
/// switch chalata hai; fail ho to toggle Wi-Fi par wapas + wajah status
/// mein. Port khaali ho to toggle-on auto-detect karta hai
/// (DetectAndSwitchToSerialAsync); port chuna ho to sirf usi port par
/// switch hota hai — auto-detect fallback nahi (ghalat port par khamoshi
/// se switch kabhi nahi).
/// </summary>
public sealed class TransportSwitchController : INotifyPropertyChanged
{
    private readonly ITransportSwitchTarget _target;
    private readonly Func<string[]> _listPorts;
    private bool _isUsbSerial;
    private bool _busy;
    private bool _applying; // setter-triggered switch chal raha hai — revert loop guard
    private string? _selectedPortName;
    private string? _activePortName;
    private string _statusText = "";
    private string[] _availablePorts = [];

    public TransportSwitchController(ITransportSwitchTarget target, Func<string[]>? listPorts = null)
    {
        _target = target ?? throw new ArgumentNullException(nameof(target));
        _listPorts = listPorts ?? (static () => Array.Empty<string>());
        _isUsbSerial = target.ActiveTransport == TransportKind.UsbSerial;
        _selectedPortName = target.SerialPortName;
        _activePortName = target.SerialPortName;
        StatusText = _isUsbSerial
            ? $"USB-serial par connected ({_activePortName ?? "board"})."
            : "Wi-Fi par connected.";
        try
        {
            RefreshPorts();
        }
        catch (Exception)
        {
            // Port list na mile to controller kabhi nahi torta — khaali
            // list + auto-detect rasta rehta hai.
            _availablePorts = [];
        }
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>Toggle ki state — asal transport ko reflect karti hai (switch ke baad update hoti hai).</summary>
    public bool IsUsbSerial
    {
        get => _isUsbSerial;
        set
        {
            if (value == _isUsbSerial) return;
            if (_busy || _applying)
            {
                // Switch chal raha hai — toggle wapas asal state par (binding dobara parhe).
                OnPropertyChanged(nameof(IsUsbSerial));
                return;
            }
            _ = SetUseUsbSerialAsync(value);
        }
    }

    /// <summary>Dropdown mein chuna port (null/khaali = auto-detect).</summary>
    public string? SelectedPortName
    {
        get => _selectedPortName;
        set => SetField(ref _selectedPortName, value, nameof(SelectedPortName));
    }

    /// <summary>System par maujood serial ports (RefreshPorts se taaza).</summary>
    public string[] AvailablePorts
    {
        get => _availablePorts;
        private set => SetField(ref _availablePorts, value, nameof(AvailablePorts));
    }

    /// <summary>Switch chal raha hai — is doran toggle ignore hota hai.</summary>
    public bool IsBusy
    {
        get => _busy;
        private set => SetField(ref _busy, value, nameof(IsBusy));
    }

    /// <summary>Serial active ho to us ka port naam, warna null.</summary>
    public string? ActivePortName
    {
        get => _activePortName;
        private set => SetField(ref _activePortName, value, nameof(ActivePortName));
    }

    /// <summary>Aakhri switch ka natija — kamyabi ya saaf wajah (UI mein dikhta hai).</summary>
    public string StatusText
    {
        get => _statusText;
        private set => SetField(ref _statusText, value, nameof(StatusText));
    }

    /// <summary>
    /// Asal switch — tests isi ko await karte hain (toggle setter isay
    /// fire-and-forget chalata hai). True = mangi hui state mil gayi,
    /// false = nahi mili (busy-ignored, fail, ya cancel). Fail par state
    /// Wi-Fi par rehti hai (target pehle se fail-closed hai — aadhi
    /// switch nahi hoti).
    /// </summary>
    public async Task<bool> SetUseUsbSerialAsync(bool useSerial, CancellationToken cancellationToken = default)
    {
        if (_busy) return false;
        if (useSerial == _isUsbSerial && useSerial == (_target.ActiveTransport == TransportKind.UsbSerial))
            return true; // pehle se wahi state — kuch nahi karna
        _applying = true;
        IsBusy = true;
        try
        {
            if (!useSerial)
            {
                _target.SwitchToWifi();
                _isUsbSerial = false;
                ActivePortName = null;
                StatusText = "Wi-Fi par wapas — wheel Wi-Fi (UDP) par hai.";
                return true;
            }

            (bool ok, string? error) = _selectedPortName is { Length: > 0 } port
                ? await _target.SwitchToSerialAsync(port, cancellationToken).ConfigureAwait(false)
                : await _target.DetectAndSwitchToSerialAsync(cancellationToken).ConfigureAwait(false);
            if (!ok)
            {
                // Fail: toggle Wi-Fi par wapas + saaf wajah — target khud
                // Wi-Fi par hi rehta hai (SwitchToSerialAsync fail-closed).
                _isUsbSerial = false;
                ActivePortName = null;
                StatusText = $"USB-serial par switch nahi ho saka: {error ?? "unknown error"}";
                return false;
            }
            _isUsbSerial = true;
            ActivePortName = _target.SerialPortName;
            StatusText = $"USB-serial par connected ({ActivePortName ?? "board"}).";
            return true;
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            _isUsbSerial = _target.ActiveTransport == TransportKind.UsbSerial;
            ActivePortName = _target.SerialPortName;
            StatusText = "Switch cancel ho gaya — transport wahi hai jo pehle tha.";
            return false;
        }
        catch (Exception ex)
        {
            _isUsbSerial = _target.ActiveTransport == TransportKind.UsbSerial;
            ActivePortName = _target.SerialPortName;
            StatusText = $"USB-serial par switch nahi ho saka: {ex.Message}";
            return false;
        }
        finally
        {
            IsBusy = false;
            _applying = false;
            OnPropertyChanged(nameof(IsUsbSerial));
        }
    }

    /// <summary>System ports dobara parho (dropdown taaza). Selection barqarar rehti hai, port gayab bhi ho jaye to.</summary>
    public void RefreshPorts()
    {
        string[] ports;
        try
        {
            ports = _listPorts() ?? [];
        }
        catch (Exception)
        {
            ports = [];
        }
        AvailablePorts = ports;
    }

    private void SetField<T>(ref T field, T value, string propertyName)
    {
        if (EqualityComparer<T>.Default.Equals(field, value)) return;
        field = value;
        OnPropertyChanged(propertyName);
    }

    private void OnPropertyChanged(string propertyName) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
}
