using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class OutputPage : UserControl, IUpdatableView
{
    public OutputPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Output;
    }

    public void Tick() => ViewModelStore.Output.Tick();
}
