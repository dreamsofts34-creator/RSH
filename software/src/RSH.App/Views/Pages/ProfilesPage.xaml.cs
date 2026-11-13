using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class ProfilesPage : UserControl
{
    public ProfilesPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Profiles;
        // Singleton VM hai — launcher / doosri jagah se profile badla ho
        // to navigation par taaza list dikhao (UI-9 sync fix).
        ViewModelStore.Profiles.RefreshOnNavigate();
    }
}
