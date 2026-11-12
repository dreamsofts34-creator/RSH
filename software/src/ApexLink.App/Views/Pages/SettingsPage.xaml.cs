using System.Windows.Controls;
using ApexLink.App.ViewModels;

namespace ApexLink.App.Views.Pages;

public partial class SettingsPage : UserControl
{
    public SettingsPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Settings;
    }
}
