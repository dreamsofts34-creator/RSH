using System.Windows.Controls;
using ApexLink.App.ViewModels;

namespace ApexLink.App.Views.Pages;

public partial class OutputPage : UserControl, IUpdatableView
{
    public OutputPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Output;
    }

    public void Tick() => ViewModelStore.Output.Tick();
}
