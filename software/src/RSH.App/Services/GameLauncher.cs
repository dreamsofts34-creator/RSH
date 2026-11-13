using System.Diagnostics;
using System.IO;
using Microsoft.Win32;
using RSH.Core.Security;

namespace RSH.App.Services;

/// <summary>
/// Game launcher (UI-4): 4 games — launch par bound profile khud active,
/// ek waqt mein ek hi game RUNNING, game na mile to saaf error.
/// Tamam I/O (registry, file, process, profile save) background threads
/// par hota hai — LaunchAsync/StopAsync/RefreshAsync ko hamesha
/// Task.Run se bulao (U-P1). Poll() UI thread se Tick mein chalta hai
/// aur koi I/O nahi karta (sirf HasExited check + background restore).
/// </summary>
public sealed class GameLauncher
{
    /// <summary>Ek game ki pehchan: process naam (pehla = launch exe ka), Steam folder + exe.</summary>
    public sealed record GameDefinition(
        string Name,
        string[] ProcessNames,
        string SteamFolder,
        string ExeName,
        string[] ExtraExePaths);

    public enum GameState
    {
        /// <summary>Abhi scan nahi hua (pehli RefreshAsync baqi hai).</summary>
        Unknown,
        NotInstalled,
        Ready,
        Running,
    }

    /// <summary>Ek game ki halat — service likhta hai (lock ke neeche), UI Tick parhta hai.</summary>
    public sealed class GameEntry
    {
        internal GameEntry(GameDefinition definition) => Definition = definition;

        public GameDefinition Definition { get; }
        public string Name => Definition.Name;
        public string? ExePath { get; internal set; }
        public bool IsInstalled => ExePath is not null;
        public string? BoundProfileName { get; internal set; }
        public GameState State { get; internal set; } = GameState.Unknown;
        public string StatusText { get; internal set; } = "Checking…";
        internal Process? TrackedProcess { get; set; }
        internal bool LaunchedByUs { get; set; }
        internal string? PreviousProfileName { get; set; }
    }

    // Assetto Corsa: AssettoCorsa.exe launcher hai — asal sim process
    // acs.exe hai (launcher khula rehta hai jab tak game chale). Dono
    // naam track karo taake game chalte hue "stopped" na dikhe.
    public static readonly IReadOnlyList<GameDefinition> Catalogue =
    [
        new("Assetto Corsa", ["AssettoCorsa", "acs"], "assettocorsa", "AssettoCorsa.exe", []),
        new("Assetto Corsa Competizione", ["acc"], "Assetto Corsa Competizione", "acc.exe", []),
        new("iRacing", ["iRacingSim64DX11"], "iRacing", "iRacingSim64DX11.exe",
            [@"%ProgramFiles(x86)%\iRacing\iRacingSim64DX11.exe", @"%ProgramFiles%\iRacing\iRacingSim64DX11.exe"]),
        new("Dirt Rally 2.0", ["dirt2"], "DiRT Rally 2.0", "dirt2.exe", []),
    ];

    private readonly AppServices _services;
    private readonly Action? _onProfileChanged;
    private readonly object _gate = new();
    private readonly SemaphoreSlim _refreshLock = new(1, 1);
    // Do rows se ek saath launch (double-click / do clicks) ek hi
    // faisla de: pehli launch Running bane, doosri blocked. Bina is ke
    // check-then-start race se do games chal sakti thin.
    private readonly SemaphoreSlim _launchGate = new(1, 1);

    public GameLauncher(AppServices services, Action? onProfileChanged = null)
    {
        _services = services;
        _onProfileChanged = onProfileChanged;
    }

    public IReadOnlyList<GameEntry> Entries { get; } =
        Catalogue.Select(d => new GameEntry(d)).ToList();

    /// <summary>Koi game chal raha hai (launch block ka faisla — lock ke neeche).</summary>
    public bool AnyRunning
    {
        get { lock (_gate) return Entries.Any(e => e.State == GameState.Running); }
    }

    public GameEntry? RunningEntry
    {
        get { lock (_gate) return Entries.FirstOrDefault(e => e.State == GameState.Running); }
    }

    /// <summary>
    /// UI Tick (UI thread) se: band hua process pakro + bound profile
    /// naam taaza karo (Profiles page se badle hon to). Koi I/O nahi —
    /// restore background par jata hai.
    /// </summary>
    public void Poll()
    {
        RefreshBoundProfiles();
        GameEntry[] snapshot;
        lock (_gate) snapshot = Entries.ToArray();
        foreach (var entry in snapshot)
        {
            Process? proc;
            lock (_gate) proc = entry.TrackedProcess;
            if (proc is null) continue;
            bool exited;
            try { exited = proc.HasExited; }
            catch (Exception) { exited = true; }   // zinda process ka HasExited kabhi throw nahi karta — kharab handle = band samjho
            if (exited) _ = FinalizeExitAsync(entry, proc);
        }
    }

    /// <summary>
    /// Steam + running processes scan (background thread — registry,
    /// file I/O, process enumeration). Dobara chalana mehfooz hai.
    /// </summary>
    public async Task RefreshAsync(CancellationToken cancellationToken = default)
    {
        if (!await _refreshLock.WaitAsync(0, cancellationToken).ConfigureAwait(false))
            return;   // pehle se scan chal raha hai — dobara nahi
        try
        {
            string? steamRoot = FindSteamRoot();
            List<string> libraries = steamRoot is null
                ? []
                : FindSteamLibraries(steamRoot);
            foreach (var entry in Entries)
            {
                cancellationToken.ThrowIfCancellationRequested();
                string? exe = FindExe(entry.Definition, libraries);
                Process? running = FindRunningProcess(entry.Definition);
                lock (_gate)
                {
                    entry.ExePath = exe;
                    if (entry.State != GameState.Running)
                    {
                        entry.State = exe is null ? GameState.NotInstalled
                            : running is not null ? GameState.Running
                            : GameState.Ready;
                        entry.StatusText = exe is null ? "Not installed."
                            : running is not null ? "Running."
                            : "Ready.";
                    }
                    // Pehle se chal raha game attach karo (launch nahi —
                    // profile switch sirf apni launch par hota hai).
                    if (running is not null && entry.TrackedProcess is null)
                        AttachTo(entry, running, launchedByUs: false);
                }
            }
            RefreshBoundProfiles();
        }
        finally
        {
            _refreshLock.Release();
        }
    }

    /// <summary>
    /// Game launch (background thread — process start + profile save).
    /// Click ka jawab foran milta hai: caller pehle StatusText
    /// "Launching…" kare, phir Task.Run se ye bulaye.
    /// </summary>
    public async Task<(bool Ok, string Message)> LaunchAsync(GameEntry entry)
    {
        // Launch serialize karo — do rows ki concurrent launch ek hi
        // waqt "koi game nahi chal raha" dekh sakti thi.
        await _launchGate.WaitAsync().ConfigureAwait(false);
        try { return await LaunchInnerAsync(entry).ConfigureAwait(false); }
        finally { _launchGate.Release(); }
    }

    private async Task<(bool Ok, string Message)> LaunchInnerAsync(GameEntry entry)
    {
        GameEntry? other;
        lock (_gate) other = Entries.FirstOrDefault(e => e.State == GameState.Running);
        if (other is not null)
        {
            if (other == entry)
            {
                SetStatus(entry, "Running.");
                return (false, $"{entry.Name} is already running.");
            }
            SetStatus(entry, $"{other.Name} is already running — stop it first.");
            return (false, $"{other.Name} is already running — stop it first.");
        }

        string? exe;
        lock (_gate) exe = entry.ExePath;
        if (exe is null)
        {
            // Shayad game scan ke baad install hua ho — ek dafa dobara dhoondo.
            await RefreshAsync().ConfigureAwait(false);
            lock (_gate) exe = entry.ExePath;
            if (exe is null)
            {
                SetStatus(entry, $"{entry.Name} was not found — install it via Steam, then try again.");
                return (false, $"{entry.Name} was not found — install it via Steam, then try again.");
            }
        }

        // Pehle se chal raha ho (scan aur launch ke beech mein khula ho)
        // to naya instance nahi — attach karo.
        Process? already = FindRunningProcess(entry.Definition);
        if (already is not null)
        {
            lock (_gate) AttachTo(entry, already, launchedByUs: false);
            return (false, $"{entry.Name} is already running.");
        }

        // Bound profile khud active ho (topbar + Profiles sync ke liye
        // ProfileChanged uthata hai). Na mile to launch baghair switch
        // ke hota hai — game kabhi profile ki wajah se nahi rukta.
        string? bound = FindBoundProfileName(entry.Definition);
        string? previous = null;
        bool switched = false;
        if (bound is not null &&
            !string.Equals(_services.Profiles.ActiveProfileName, bound, StringComparison.OrdinalIgnoreCase))
        {
            previous = _services.Profiles.ActiveProfileName;
            _services.Profiles.ActiveProfileName = bound;
            try
            {
                await _services.SaveProfilesAndApplyAsync().ConfigureAwait(false);
                switched = true;
                _onProfileChanged?.Invoke();
            }
            catch (Exception ex)
            {
                // Profile save na ho to launch phir bhi honi chahiye —
                // purani profile wapas aur saaf wajah status mein.
                _services.Profiles.ActiveProfileName = previous;
                SetStatus(entry, $"Could not activate profile '{bound}': {ex.Message}");
                return (false, $"Could not activate profile '{bound}': {ex.Message}");
            }
        }

        Process? proc;
        try
        {
            proc = Process.Start(new ProcessStartInfo
            {
                FileName = exe,
                WorkingDirectory = Path.GetDirectoryName(exe) ?? "",
                UseShellExecute = true,
            });
        }
        catch (Exception ex)
        {
            if (switched && previous is not null)
            {
                // Launch fail — profile wapas (best-effort).
                try
                {
                    _services.Profiles.ActiveProfileName = previous;
                    await _services.SaveProfilesAndApplyAsync().ConfigureAwait(false);
                    _onProfileChanged?.Invoke();
                }
                catch (Exception) { /* launch ki asal wajah hi status mein jayegi */ }
            }
            SetStatus(entry, $"Could not start {entry.Name}: {ex.Message}");
            return (false, $"Could not start {entry.Name}: {ex.Message}");
        }
        if (proc is null)
        {
            if (switched && previous is not null)
            {
                try
                {
                    _services.Profiles.ActiveProfileName = previous;
                    await _services.SaveProfilesAndApplyAsync().ConfigureAwait(false);
                    _onProfileChanged?.Invoke();
                }
                catch (Exception) { }
            }
            SetStatus(entry, $"Could not start {entry.Name}: the process did not start.");
            return (false, $"Could not start {entry.Name}: the process did not start.");
        }

        string runningText = bound is not null
            ? $"Running — profile '{bound}' is active."
            : "Running.";
        lock (_gate)
        {
            entry.PreviousProfileName = switched ? previous : null;
            AttachTo(entry, proc, launchedByUs: true, statusText: runningText);
        }
        return (true, runningText);
    }

    /// <summary>Game band karo (background thread — Kill + bounded wait).</summary>
    public async Task<(bool Ok, string Message)> StopAsync(GameEntry entry)
    {
        Process? proc;
        lock (_gate) proc = entry.TrackedProcess;
        if (proc is null)
        {
            lock (_gate)
            {
                if (entry.State == GameState.Running)
                {
                    entry.State = GameState.Ready;
                    entry.StatusText = "Ready.";
                }
            }
            return (true, "Ready.");
        }

        // SEC-E-10 (Phase F): Stop SIRF tracked process ko maarta hai — naam
        // se milne wale doosre processes ko haath nahi lagata. Pehle yahan
        // ProcessNames par SafeGetProcessesByName loop tha jo user ne khud
        // khola hua doosra same-name instance bhi maar deta tha (misal
        // doosri Assetto Corsa). Helper process (acs.exe) agar launcher ke
        // marne ke baad akela chal raha ho to RefreshAsync usay dobara
        // attach karke Running dikhata hai aur user usay alag se Stop kar
        // sakta hai — kisi ajnabi process ko maarne se ye behtar hai.
        try
        {
            if (!proc.HasExited) proc.Kill(entireProcessTree: false);
        }
        catch (Exception ex)
        {
            SetStatus(entry, $"Could not stop {entry.Name}: {ex.Message}");
            return (false, $"Could not stop {entry.Name}: {ex.Message}");
        }

        // Bounded wait — background thread par hai, UI kabhi block nahi.
        try { await Task.Run(() => proc.WaitForExit(5000)).ConfigureAwait(false); }
        catch (Exception) { /* wait ki nakami = Exited/Poll finalise karega */ }

        bool exited;
        try { exited = proc.HasExited; }
        catch (Exception) { exited = true; }
        if (exited)
        {
            await FinalizeExitAsync(entry, proc, closedText: "Stopped.").ConfigureAwait(false);
            return (true, "Stopped.");
        }
        return (true, "Stop sent — waiting for the game to close.");
    }

    /// <summary>Entry ka status text — VM har Tick par yahin se parhti hai (wahid source).</summary>
    private void SetStatus(GameEntry entry, string text)
    {
        lock (_gate) entry.StatusText = text;
    }

    /// <summary>
    /// Entry ko ek process se joro (Running bane). Sirf _gate ke neeche
    /// bulaya jata hai. Internal hai taake RSH.App.Tests sandbox mein stub
    /// processes attach karke Stop/restore test kar sake (asan test seam —
    /// production behaviour wahi hai).
    /// </summary>
    internal void AttachTo(GameEntry entry, Process proc, bool launchedByUs, string? statusText = null)
    {
        // Sirf _gate ke neeche bulaya jata hai.
        entry.TrackedProcess = proc;
        entry.LaunchedByUs = launchedByUs;
        entry.State = GameState.Running;
        entry.StatusText = statusText ?? "Running.";
        try
        {
            proc.EnableRaisingEvents = true;
            proc.Exited += (_, _) => _ = FinalizeExitAsync(entry, proc);
        }
        catch (Exception) { /* Exited hook na lage to Poll backup hai */ }
    }

    private async Task FinalizeExitAsync(GameEntry entry, Process proc, string closedText = "Closed.")
    {
        // Claim: sirf pehli dafa finalize ho (Exited + Poll + Stop
        // teeno ek hi exit par aa sakte hain).
        bool claimed;
        lock (_gate)
        {
            claimed = ReferenceEquals(entry.TrackedProcess, proc);
            if (claimed) entry.TrackedProcess = null;
        }
        if (!claimed) return;

        try
        {
            // Purani profile wapas — sirf tab jab game hamari launch thi
            // aur bound profile abhi bhi active hai (user ne beech mein
            // khud kuch badla ho to us ki pasand rehti hai).
            string? previous;
            string? bound;
            lock (_gate)
            {
                previous = entry.PreviousProfileName;
                bound = entry.BoundProfileName;
                entry.PreviousProfileName = null;
            }
            if (entry.LaunchedByUs && previous is not null && bound is not null &&
                string.Equals(_services.Profiles.ActiveProfileName, bound, StringComparison.OrdinalIgnoreCase))
            {
                _services.Profiles.ActiveProfileName = previous;
                await _services.SaveProfilesAndApplyAsync().ConfigureAwait(false);
                _onProfileChanged?.Invoke();
            }
        }
        catch (Exception)
        {
            // Restore best-effort hai — game band ho gaya, profile agli
            // launch par theek ho jayegi.
        }
        finally
        {
            lock (_gate)
            {
                entry.LaunchedByUs = false;
                if (entry.State == GameState.Running)
                {
                    entry.State = GameState.Ready;
                    entry.StatusText = closedText;
                }
            }
            try { proc.Dispose(); } catch (Exception) { }
        }
    }

    private void RefreshBoundProfiles()
    {
        // UI thread (Poll/Tick) se bhi chalta hai — Profiles list ki
        // structural tabdeeli UI thread par hoti hai, is liye yahan
        // parhna mehfooz hai; background se aaye to try/catch hai.
        foreach (var entry in Entries)
        {
            try { entry.BoundProfileName = FindBoundProfileName(entry.Definition); }
            catch (Exception) { /* agle Tick par dobara */ }
        }
    }

    private string? FindBoundProfileName(GameDefinition definition)
    {
        foreach (string processName in definition.ProcessNames)
        {
            foreach (var profile in _services.Profiles.Profiles)
            {
                if (string.Equals(NormalizeProcessName(profile.GameProcessName), processName, StringComparison.OrdinalIgnoreCase))
                    return profile.Name;
            }
        }
        return null;
    }

    private static string NormalizeProcessName(string value)
    {
        string name = value.Trim();
        if (name.EndsWith(".exe", StringComparison.OrdinalIgnoreCase))
            name = name[..^4];
        return name;
    }

    private static Process? FindRunningProcess(GameDefinition definition)
    {
        foreach (string name in definition.ProcessNames)
        {
            var found = SafeGetProcessesByName(name);
            for (int i = 0; i < found.Count; i++)
            {
                if (i == 0) continue;   // pehla wapas — baqi dispose
                found[i].Dispose();
            }
            if (found.Count > 0) return found[0];
        }
        return null;
    }

    private static List<Process> SafeGetProcessesByName(string name)
    {
        try { return Process.GetProcessesByName(name).ToList(); }
        catch (Exception) { return []; }
    }

    private static string? FindExe(GameDefinition definition, List<string> libraries)
    {
        foreach (string library in libraries)
        {
            string candidate = Path.Combine(library, "steamapps", "common", definition.SteamFolder, definition.ExeName);
            try
            {
                if (File.Exists(candidate)) return candidate;
            }
            catch (Exception) { /* agla candidate */ }
        }
        foreach (string raw in definition.ExtraExePaths)
        {
            try
            {
                string expanded = Environment.ExpandEnvironmentVariables(raw);
                if (File.Exists(expanded)) return expanded;
            }
            catch (Exception) { /* agla candidate */ }
        }
        return null;
    }

    private static string? FindSteamRoot()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(@"Software\Valve\Steam", writable: false);
            if (key?.GetValue("SteamPath") is string path && path.Length > 0) return path;
        }
        catch (Exception) { /* registry na mile — fallback */ }
        try
        {
            using var key = Registry.LocalMachine.OpenSubKey(@"SOFTWARE\WOW6432Node\Valve\Steam", writable: false);
            if (key?.GetValue("InstallPath") is string path && path.Length > 0) return path;
        }
        catch (Exception) { /* registry na mile — fallback */ }
        return @"C:\Program Files (x86)\Steam";
    }

    /// <summary>libraryfolders.vdf se tamam Steam library paths (pehla = khud Steam root).</summary>
    internal static List<string> FindSteamLibraries(string steamRoot)
    {
        var libraries = new List<string> { steamRoot };
        try
        {
            string vdf = Path.Combine(steamRoot, "steamapps", "libraryfolders.vdf");
            var info = new FileInfo(vdf);
            if (!info.Exists || info.Length > 1_048_576) return libraries;
            foreach (string line in File.ReadAllLines(vdf))
            {
                string? path = ParseVdfPath(line);
                // SEC-E-11 (Phase F, W2 validator — 1-line wiring): VDF se nikli
                // har path istemal se PEHLE validate ho (traversal / relative /
                // UNC / Windows-dir reject) — ghalat path khamoshi se drop hota
                // hai, scan kabhi nahi rukta. Internal hai taake RSH.App.Tests
                // temp VDF se ye wiring test kar sake.
                if (path is not null &&
                    SteamLibraryPathValidator.TryValidateLibraryPath(path, out string? normalized, out _) &&
                    normalized is not null &&
                    !libraries.Contains(normalized, StringComparer.OrdinalIgnoreCase))
                    libraries.Add(normalized);
            }
        }
        catch (Exception) { /* vdf na parhi jaye to sirf Steam root */ }
        return libraries;
    }

    /// <summary>libraryfolders.vdf ki ek line se "path" ki value — nahi mili to null.</summary>
    internal static string? ParseVdfPath(string line)
    {
        int key = line.IndexOf("\"path\"", StringComparison.OrdinalIgnoreCase);
        if (key < 0) return null;
        int open = line.IndexOf('"', key + 6);
        if (open < 0) return null;
        int close = line.IndexOf('"', open + 1);
        if (close < 0) return null;
        string value = line[(open + 1)..close].Replace(@"\\", @"\");
        return value.Length > 0 ? value : null;
    }
}
