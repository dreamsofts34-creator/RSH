using System.Windows;
using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class SetupPage : UserControl, IUpdatableView
{
    public SetupPage()
    {
        InitializeComponent();
        ViewModelStore.Setup.Begin();
        DataContext = ViewModelStore.Setup;
        // SEC-C-04: Wi-Fi password masked dikhao (PasswordBox) — plain
        // TextBox nahi. Connection page jaisa pattern.
        WifiPasswordBox.Password = ViewModelStore.Connection.WifiPassword;
        // Wizard beech mein band ho / page se navigate ho to partial state
        // save nahi hoti — Discard() sirf in-memory capture reset karta hai.
        Unloaded += (_, _) => ViewModelStore.Setup.Discard();
    }

    // PasswordBox.Password bind nahi hota — change haath se ViewModel tak.
    private void WifiPasswordBox_PasswordChanged(object sender, RoutedEventArgs e)
    {
        if (ViewModelStore.Connection.WifiPassword != WifiPasswordBox.Password)
            ViewModelStore.Connection.WifiPassword = WifiPasswordBox.Password;
    }

    public void Tick()
    {
        var vm = ViewModelStore.Setup;
        vm.Tick();
        SetStar(RC0, vm.MinPos);
        SetStar(RC1, Math.Max(0, vm.MaxPos - vm.MinPos));
        SetStar(RC2, Math.Max(0, 100 - vm.MaxPos));
        SetStar(NC0, vm.LivePos);
        SetStar(NC2, Math.Max(0, 100 - vm.LivePos));
    }

    private static void SetStar(ColumnDefinition column, double value) =>
        column.Width = new GridLength(value, GridUnitType.Star);
}
