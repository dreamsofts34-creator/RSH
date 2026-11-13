using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class ButtonsPage : UserControl, IUpdatableView
{
    public ButtonsPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Buttons;
    }

    public void Tick() => ViewModelStore.Buttons.Tick();
}
