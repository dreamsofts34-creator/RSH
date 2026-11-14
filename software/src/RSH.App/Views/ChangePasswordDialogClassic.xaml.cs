using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Input;
using RSH.App.Services;
using RSH.Core.Transport;
using CommunityToolkit.Mvvm.Input;

namespace RSH.App.Views;

/// <summary>
/// Change wheel password dialog — CLASSIC variant (O-1, Round 2 W2):
/// plain <see cref="Window"/> (OS title bar), classic mode (Win10 auto /
/// --classic-window) ke liye. Logic <see cref="ChangePasswordDialog"/>
/// jaisi hi hai (twin — dono ko ek saath update karo): validation
/// ControlClient.ValidateNewPassword se (8–63 chars, printable ASCII),
/// BoardSession.SetPasswordAsync (WS5 API) call hoti hai; kamyabi par
/// Succeeded + NewPassword; store-save fail ho to Succeeded phir bhi true
/// aur SaveWarning mein wajah (SEC-E-09). Win11 par ye variant kabhi nahi
/// khulti — wahan FluentWindow dialog waisi hi rehti hai.
/// (Window hai, ObservableObject nahi — plain INotifyPropertyChanged +
/// RelayCommand fields, Fluent variant jaisi hi.)
/// </summary>
public partial class ChangePasswordDialogClassic : Window, INotifyPropertyChanged, IChangePasswordDialogResult
{
    private readonly AppServices _services;
    private bool _changing;
    private string _validationText = "";
    private string _statusText = "";
    private bool _canChange;

    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>Kamyab change ke baad true — caller NewPassword parhe.</summary>
    public bool Succeeded { get; private set; }

    /// <summary>Kamyab change ke baad naya password (caller stored Wi-Fi password update kare).</summary>
    public string? NewPassword { get; private set; }

    /// <summary>SEC-E-09: board par password badal gaya lekin secure store
    /// mein save nahi ho saka (DPAPI/IO fail) — Succeeded phir bhi true hai;
    /// caller ye warning user ko dikhaye. Null = save theek hua.</summary>
    public string? SaveWarning { get; private set; }

    public string ValidationText
    {
        get => _validationText;
        private set { _validationText = value; OnPropertyChanged(); }
    }

    public string StatusText
    {
        get => _statusText;
        private set { _statusText = value; OnPropertyChanged(); }
    }

    public bool CanChange
    {
        get => _canChange;
        private set
        {
            _canChange = value;
            OnPropertyChanged();
            _changeCommand.NotifyCanExecuteChanged();
        }
    }

    public ICommand CancelCommand { get; }
    public ICommand ChangeCommand => _changeCommand;
    private readonly AsyncRelayCommand _changeCommand;

    public ChangePasswordDialogClassic(AppServices services)
    {
        _services = services;
        CancelCommand = new RelayCommand(Cancel);
        _changeCommand = new AsyncRelayCommand(ChangeAsync, () => CanChange);
        InitializeComponent();
        DataContext = this;
    }

    private void OnPropertyChanged([CallerMemberName] string? name = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));

    private void PasswordBox_PasswordChanged(object sender, RoutedEventArgs e) => Validate();

    private void Validate()
    {
        if (Succeeded)
        {
            ValidationText = "";
            CanChange = false;
            return;
        }
        string password = NewPasswordBox.Password;
        string confirm = ConfirmPasswordBox.Password;
        if (password.Length == 0 && confirm.Length == 0)
        {
            ValidationText = "";
            CanChange = false;
            return;
        }
        string? error = ControlClient.ValidateNewPassword(password);
        if (error is not null)
        {
            ValidationText = error;
            CanChange = false;
            return;
        }
        if (!string.Equals(password, confirm, StringComparison.Ordinal))
        {
            ValidationText = "Passwords do not match.";
            CanChange = false;
            return;
        }
        ValidationText = "";
        CanChange = true;
    }

    private void Cancel()
    {
        if (!_changing) Close();
    }

    private async Task ChangeAsync()
    {
        if (_changing || !CanChange) return;
        _changing = true;
        CanChange = false;
        StatusText = "Sending…";
        ValidationText = "";
        string password = NewPasswordBox.Password;
        try
        {
            var (sent, error) = await _services.BoardSession.SetPasswordAsync(_services.Settings.EspIp, password, _services.ActiveControlTransport);
            if (!sent)
            {
                StatusText = $"Password not changed: {error ?? "no session with the wheel"}";
                CanChange = true;
                return;
            }
            Succeeded = true;
            NewPassword = password;
            // SEC-E-09: sent==true + error!=null = board badal gaya, store
            // save fail hua — warning caller tak pahunchana lazmi hai.
            SaveWarning = error;
            StatusText = error is null
                ? "Password changed. The wheel restarts its access point — reconnect with the new password."
                : $"Password changed on the wheel, but: {error}";
            NewPasswordBox.Clear();
            ConfirmPasswordBox.Clear();
            NewPasswordBox.IsEnabled = false;
            ConfirmPasswordBox.IsEnabled = false;
        }
        catch (ArgumentException ex)
        {
            ValidationText = ex.Message;
            CanChange = true;
        }
        catch (Exception ex)
        {
            StatusText = $"Password not changed: {ex.Message}";
            CanChange = true;
        }
        finally
        {
            _changing = false;
        }
    }
}
