using System.Windows.Controls;
using ApexLink.App.ViewModels;

namespace ApexLink.App.Views.Pages;

public partial class ProfilesPage : UserControl
{
    public ProfilesPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Profiles;
    }
}
