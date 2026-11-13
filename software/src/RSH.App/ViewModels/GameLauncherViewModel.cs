using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using RSH.App.Services;
using RSH.Core.Security;

namespace RSH.App.ViewModels;

/// <summary>
/// Game launcher VM (UI-4): Dashboard par 4 games. Launch/Stop click ka
/// jawab foran milta hai (status turant badalta hai, &lt;100 ms) — asal
/// kaam (process start, profile save) background par hota hai (U-P1).
/// Row state har Tick (30 Hz) par service se sync hoti hai.
/// </summary>
public sealed partial class GameLauncherViewModel : ObservableObject
{
    private readonly GameLauncher _launcher;

    public GameLauncher Launcher => _launcher;

    public ObservableCollection<GameRowViewModel> Rows { get; } = [];

    public GameLauncherViewModel(AppServices services, Action? onProfileChanged = null)
    {
        _launcher = new GameLauncher(services, onProfileChanged);
        foreach (var entry in _launcher.Entries)
            Rows.Add(new GameRowViewModel(this, entry));
        _ = RefreshInitialAsync();
    }

    private async Task RefreshInitialAsync()
    {
        try { await Task.Run(() => _launcher.RefreshAsync()); }
        catch (Exception)
        {
            // Scan ki nakami row par "Checking…" chhorti hai — launch par
            // dobara scan hota hai, is liye kuch khamoshi se nahi rukta.
        }
    }

    public void Tick()
    {
        _launcher.Poll();
        foreach (var row in Rows) row.Sync();
    }
}

/// <summary>Launcher ki ek row: game naam + bound profile + Launch/Stop.</summary>
public sealed partial class GameRowViewModel : ObservableObject
{
    private readonly GameLauncherViewModel _owner;
    private readonly GameLauncher.GameEntry _entry;
    private bool _busy;

    [ObservableProperty] private string _name = "";
    [ObservableProperty] private string _boundProfileText = "";
    [ObservableProperty] private string _statusText = "Checking…";
    [ObservableProperty] private bool _isRunning;
    [ObservableProperty] private bool _isInstalled;
    [ObservableProperty] private bool _canLaunch;
    [ObservableProperty] private bool _canStop;

    internal GameRowViewModel(GameLauncherViewModel owner, GameLauncher.GameEntry entry)
    {
        _owner = owner;
        _entry = entry;
        Name = entry.Name;
        Sync();
    }

    internal void Sync()
    {
        IsRunning = _entry.State == GameLauncher.GameState.Running;
        IsInstalled = _entry.IsInstalled;
        BoundProfileText = _entry.BoundProfileName is { } bound
            ? $"Profile: {bound}"
            : "No profile bound";
        // _busy (launch/stop chal raha) ho to row ka foran wala status
        // ("Launching…") rehne do — service ka nateeja await ke baad
        // Sync() mein aayega.
        if (!_busy) StatusText = _entry.StatusText;
        CanLaunch = IsInstalled && !IsRunning && !_owner.Launcher.AnyRunning && !_busy;
        CanStop = IsRunning && !_busy;
    }

    [RelayCommand]
    private async Task Launch()
    {
        if (_busy) return;
        // SEC-E-11: launch se pehle exe path gate — VDF se nikli ajeeb
        // path (traversal / relative / steamapps se bahir) yahan rukti
        // hai; Process.Start tak kabhi nahi pahunchti.
        if (!SteamLibraryPathValidator.IsPlausibleGameExePath(_entry.ExePath))
        {
            StatusText = "Launch blocked: the game's exe path failed validation (not a Steam library path). Re-scan from the Dashboard.";
            return;
        }
        _busy = true;
        StatusText = "Launching…";
        CanLaunch = false;
        CanStop = false;
        try
        {
            // Asal kaam background par — click ka jawab pehle hi mil gaya.
            var (_, message) = await Task.Run(() => _owner.Launcher.LaunchAsync(_entry));
            StatusText = message;
        }
        catch (Exception ex)
        {
            StatusText = $"Launch failed: {ex.Message}";
        }
        finally
        {
            _busy = false;
            Sync();
        }
    }

    [RelayCommand]
    private async Task Stop()
    {
        if (_busy) return;
        _busy = true;
        StatusText = "Stopping…";
        CanLaunch = false;
        CanStop = false;
        try
        {
            var (_, message) = await Task.Run(() => _owner.Launcher.StopAsync(_entry));
            StatusText = message;
        }
        catch (Exception ex)
        {
            StatusText = $"Stop failed: {ex.Message}";
        }
        finally
        {
            _busy = false;
            Sync();
        }
    }
}
