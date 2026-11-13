using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class ConnectionPage : UserControl, IUpdatableView
{
    public ConnectionPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Connection;
        // SEC-C-04: Wi-Fi password masked dikhao (PasswordBox) — plain
        // TextBox nahi. Stored value se bharta hai (dots nazar aate hain,
        // password nahi); change ViewModel mein jata hai.
        WifiPasswordBox.Password = ViewModelStore.Connection.WifiPassword;
    }

    // PasswordBox.Password bind nahi hota — change haath se ViewModel tak.
    private void WifiPasswordBox_PasswordChanged(object sender, System.Windows.RoutedEventArgs e)
    {
        if (ViewModelStore.Connection.WifiPassword != WifiPasswordBox.Password)
            ViewModelStore.Connection.WifiPassword = WifiPasswordBox.Password;
    }

    public void Tick() => ViewModelStore.Connection.Tick();
}
