using System.Windows.Controls;
using ApexLink.App.ViewModels;

namespace ApexLink.App.Views.Pages;

public partial class ButtonsPage : UserControl, IUpdatableView
{
    public ButtonsPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Buttons;
    }

    public void Tick() => ViewModelStore.Buttons.Tick();
}
