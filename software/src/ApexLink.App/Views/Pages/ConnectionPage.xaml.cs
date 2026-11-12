using System.Windows.Controls;
using ApexLink.App.ViewModels;

namespace ApexLink.App.Views.Pages;

public partial class ConnectionPage : UserControl, IUpdatableView
{
    public ConnectionPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Connection;
    }

    public void Tick() => ViewModelStore.Connection.Tick();
}
