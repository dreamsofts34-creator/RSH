using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class DiagnosticsPage : UserControl, IUpdatableView
{
    public DiagnosticsPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Diagnostics;
    }

    public void Tick() => ViewModelStore.Diagnostics.Tick();
}
