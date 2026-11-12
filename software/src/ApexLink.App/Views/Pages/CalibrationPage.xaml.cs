using System.Windows.Controls;
using ApexLink.App.ViewModels;

namespace ApexLink.App.Views.Pages;

public partial class CalibrationPage : UserControl, IUpdatableView
{
    public CalibrationPage()
    {
        InitializeComponent();
        DataContext = ViewModelStore.Calibration;
    }

    public void Tick() => ViewModelStore.Calibration.Tick();
}
