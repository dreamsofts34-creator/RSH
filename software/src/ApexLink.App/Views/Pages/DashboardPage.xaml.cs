using System.Windows.Controls;
using ApexLink.App.ViewModels;

namespace ApexLink.App.Views.Pages;

public partial class DashboardPage : UserControl, IUpdatableView
{
    public DashboardPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Dashboard;
    }

    public void Tick() => ViewModelStore.Dashboard.Tick();
}
