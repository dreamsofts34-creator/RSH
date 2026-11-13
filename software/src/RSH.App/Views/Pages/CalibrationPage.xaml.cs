using System.Windows.Controls;
using RSH.App.ViewModels;

namespace RSH.App.Views.Pages;

public partial class CalibrationPage : UserControl, IUpdatableView
{
    public CalibrationPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Calibration;
    }

    public void Tick() => ViewModelStore.Calibration.Tick();
}
