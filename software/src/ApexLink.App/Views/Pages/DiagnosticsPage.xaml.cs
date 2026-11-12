using System.Windows.Controls;
using ApexLink.App.ViewModels;

namespace ApexLink.App.Views.Pages;

public partial class DiagnosticsPage : UserControl, IUpdatableView
{
    public DiagnosticsPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Diagnostics;
    }

    public void Tick() => ViewModelStore.Diagnostics.Tick();
}
