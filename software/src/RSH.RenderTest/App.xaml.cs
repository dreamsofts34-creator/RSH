using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;

namespace RSH.RenderTest;

/// <summary>Minimal blank-window diagnostic: plain WPF Window, no
/// FluentWindow, no NavigationView, no WPF-UI, no tray, no theme, no
/// NuGet packages at all. Decisive matrix on the user's PC:
/// renders -&gt; FluentWindow/WPF-UI chrome culprit; blank -&gt; GPU/OS
/// level; blank here but renders with --software-render -&gt; GPU/driver
/// render-thread failure (H2).</summary>
public partial class App : Application
{
    protected override void OnStartup(StartupEventArgs e)
    {
        // Same diagnostic flag as RSH.exe: --software-render forces
        // process-wide software rendering BEFORE any window exists.
        if (e.Args.Contains("--software-render"))
        {
            RenderOptions.ProcessRenderMode = RenderMode.SoftwareOnly;
        }

        base.OnStartup(e);

        var mainWindow = new MainWindow();
        MainWindow = mainWindow;
        mainWindow.Show();
    }
}
