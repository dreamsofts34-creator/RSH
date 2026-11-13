using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class DashboardPage : UserControl, IUpdatableView
{
    public DashboardPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Dashboard;
    }

    public void Tick() => ViewModelStore.Dashboard.Tick();
}
