using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Input;
using RSH.App.Services;
using RSH.Core.Transport;
using CommunityToolkit.Mvvm.Input;

namespace RSH.App.Views;

/// <summary>
/// Change wheel password dialog (UI-11 Data): BoardSession.SetPasswordAsync
/// (WS5 API — signature nahi badli) ko call karta hai. Validation
/// ControlClient.ValidateNewPassword se (firmware password.h rule:
/// 8–63 chars, printable ASCII 0x21–0x7E, space nahi) — ghalat password
/// board tak kabhi nahi jata. Kamyabi par Succeeded + NewPassword.
/// SEC-E-04 (Phase F): caller stored password ko settings.json mein
/// <b>kabhi nahi</b> likhta — SetPasswordAsync khud DPAPI KeyStore mein
/// save karta hai. Agar wo save fail ho to board ka password badal chuka
/// hota hai: Succeeded phir bhi true hai, lekin SaveWarning mein wajah
/// hoti hai aur caller use user ko dikhata hai.
/// (Window hai, ObservableObject nahi — is liye MVVM source generator
/// attributes nahi, plain INotifyPropertyChanged + RelayCommand fields.)
/// </summary>
public partial class ChangePasswordDialog : Wpf.Ui.Controls.FluentWindow, INotifyPropertyChanged
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
    /// mein save nahi ho saka (DPAPI/IO fail) — Succeeded phir bhi true
    /// hai; caller ye warning user ko dikhaye. Null = save theek hua.</summary>
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

    public ChangePasswordDialog(AppServices services)
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
