using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class SettingsPage : UserControl
{
    public SettingsPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Settings;
        // Modules + GPIO pins ka init (VM primary-ctor hai — page se ek dafa).
        ViewModelStore.Settings.EnsureLoaded();
    }
}
