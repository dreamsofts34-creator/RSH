using System.Windows;
using System.Windows.Media;

namespace RSH.RenderTest;

public partial class MainWindow : Window
{
    private int _clicks;

    public MainWindow()
    {
        InitializeComponent();
        DiagnosticsText.Text =
            $"Render Tier: {RenderCapability.Tier}\n" +
            $"Process render mode: {RenderOptions.ProcessRenderMode}\n" +
            $"OS: {Environment.OSVersion}";
    }

    private void TestButton_Click(object sender, RoutedEventArgs e)
    {
        _clicks++;
        StatusText.Text = $"Button clicked {_clicks} time(s) at {DateTime.Now:HH:mm:ss} — the window renders and responds.";
    }
}
