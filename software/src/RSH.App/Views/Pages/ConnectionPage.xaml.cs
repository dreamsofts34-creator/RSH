using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class ConnectionPage : UserControl, IUpdatableView
{
    public ConnectionPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Connection;
    }

    public void Tick() => ViewModelStore.Connection.Tick();
}
