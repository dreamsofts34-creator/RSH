using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;

namespace RSH.Core.Windows;

/// <summary>
/// Single instance + bring-to-front (A4, SW-10).
/// <para>
/// Mutex <c>Global\RSH.SingleInstance</c> pehli instance ki pehchan hai;
/// doosri instance named pipe par "activate" bhej kar pehli ko samne lati
/// hai, phir khud band ho jati hai. Named pipe is liye: named
/// <c>EventWaitHandle</c> Linux par <c>PlatformNotSupportedException</c>
/// deta hai (sandbox mein verify kiya), jab ke named pipe + named mutex
/// dono platforms par chalte hain — is liye yehi mechanism unit test mein
/// bhi verify ho sakta hai.
/// </para>
/// <para>
/// v2.0.2 HOTFIX — version-aware single instance: mutex/pipe naam mein app
/// version hota hai (<c>Global\RSH.SingleInstance.2.0.2</c>,
/// <see cref="AppVersionInfo"/> se — assembly se, hard-code nahi), taake
/// purani version ki instance nayi version ko kabhi na roke. Same-version
/// doosri instance ka behaviour waisa hi hai jaisa pehle tha (pipe se
/// activate → khud band). Purani version ki instance mile to use terminate
/// kiya jata hai (<see cref="TerminateOlderInstances"/>) — faisla neeche
/// "purana instance terminate karne ka faisla" mein.
/// </para>
/// <para>Windows par asal bring-to-front (window restore + foreground)
/// caller ki zimmedari hai — <see cref="StartListening"/> ka callback UI
/// thread par marshal karke window ko activate kare.</para>
/// </summary>
public sealed class SingleInstanceCoordinator : IDisposable
{
    /// <summary>
    /// Legacy (unversioned) mutex — v2.0.1 aur pehle ki versions yehi
    /// istemal karti thin. Sirf purane instance ki pehchan ke liye;
    /// naya code versioned naam istemal kare
    /// (<see cref="AppVersionInfo.MutexNameForVersion(Version)"/>).
    /// </summary>
    public const string DefaultMutexName = @"Global\RSH.SingleInstance";

    /// <summary>Legacy (unversioned) pipe — v2.0.1 aur pehle ki versions.</summary>
    public const string DefaultPipeName = "RSH.SingleInstance";

    /// <summary>Terminate karne ke liye dhoonda jane wala process naam (RSH.exe).</summary>
    public const string ProcessName = "RSH";

    /// <summary>Terminate karne ke liye dhoondi jane wali exe file (case-insensitive).</summary>
    public const string ExeFileName = "RSH.exe";

    private readonly string _mutexName;
    private readonly string _pipeName;
    private Mutex? _mutex;
    private CancellationTokenSource? _listenCts;
    private Task? _listenTask;
    private bool _disposed;

    public SingleInstanceCoordinator(string mutexName = DefaultMutexName, string pipeName = DefaultPipeName)
    {
        _mutexName = mutexName;
        _pipeName = pipeName;
    }

    /// <summary>
    /// Is app version ke liye coordinator banao (mutex/pipe naam versioned).
    /// Version <paramref name="appAssembly"/> (RSH.App) se aati hai —
    /// RSH.Core ki apni assembly se nahi.
    /// </summary>
    public static SingleInstanceCoordinator ForAppVersion(System.Reflection.Assembly appAssembly)
    {
        var version = AppVersionInfo.ForAssembly(appAssembly);
        return new SingleInstanceCoordinator(version.MutexName, version.PipeName);
    }

    /// <summary>
    /// Mutex hasil karo. <c>true</c> = ye pehli instance hai (aage chalao);
    /// <c>false</c> = pehli instance pehle se chal rahi hai.
    /// Abandoned mutex (purana process marte hue mutex chhor gaya) ko
    /// acquired samjho — wo process ab nahi hai, is liye aage chalna sahi hai.
    /// </summary>
    public bool TryAcquire()
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        try
        {
            _mutex = new Mutex(initiallyOwned: true, _mutexName, out bool createdNew);
            return createdNew;
        }
        catch (AbandonedMutexException)
        {
            // Ctor ne handle nahi diya (abandoned mutex ab hamara hai,
            // lekin object kho gaya) — dobara kholo aur acquire karo.
            _mutex = new Mutex(initiallyOwned: false, _mutexName, out _);
            try { _mutex.WaitOne(); } catch (AbandonedMutexException) { /* ab hamara hai */ }
            return true;
        }
    }

    /// <summary>Pehli instance mein activation listener shuru karo (background).</summary>
    public void StartListening(Action onActivate)
    {
        ArgumentNullException.ThrowIfNull(onActivate);
        ObjectDisposedException.ThrowIf(_disposed, this);
        if (_mutex is null) throw new InvalidOperationException("Pehle TryAcquire() chalao.");
        _listenCts = new CancellationTokenSource();
        var token = _listenCts.Token;
        _listenTask = Task.Run(async () =>
        {
            while (!token.IsCancellationRequested)
            {
                try
                {
                    using var server = new NamedPipeServerStream(
                        _pipeName, PipeDirection.In, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous);
                    await server.WaitForConnectionAsync(token).ConfigureAwait(false);
                    using var reader = new StreamReader(server);
                    await reader.ReadLineAsync(token).ConfigureAwait(false);
                    if (!token.IsCancellationRequested)
                        onActivate();
                }
                catch (OperationCanceledException) { break; }
                catch (IOException) { /* client beech mein toot gaya — agla connection suno */ }
                catch (ObjectDisposedException) { break; }
            }
        }, token);
    }

    /// <summary>Chal rahi pehli instance ko activate karne ka signal bhejo (best-effort).</summary>
    public static bool SignalExisting(string pipeName = DefaultPipeName, int timeoutMs = 1000)
    {
        try
        {
            using var client = new NamedPipeClientStream(".", pipeName, PipeDirection.Out);
            client.Connect(timeoutMs);
            using var writer = new StreamWriter(client) { AutoFlush = true };
            writer.WriteLine("activate");
            return true;
        }
        catch
        {
            // Pehli instance ka pipe na mile (race: wo abhi band ho rahi ho)
            // to signal nahi bhej sakte — doosri instance phir bhi band hoti hai.
            return false;
        }
    }

    // ================= v2.0.2: purani version ka instance terminate =================

    /// <summary>
    /// Faisla (pure logic, unit-testable): kya candidate process ko
    /// terminate karna chahiye? Sirf tab <c>true</c> jab SAB shartein
    /// poori hon — aik bhi na ho to <c>false</c> (fail-closed, kabhi
    /// blind kill nahi):
    /// <list type="bullet">
    /// <item>candidate pid current pid se mukhtalif ho;</item>
    /// <item>exe file ka naam <c>RSH.exe</c> ho (case-insensitive);</item>
    /// <item>candidate ka owner maloom ho AUR current user ke barabar ho
    /// (owner maloom na ho sake to <c>false</c> — doosre user ka process
    /// kabhi kill nahi);</item>
    /// <item>candidate ki file version maloom ho AUR current version se
    /// <b>strictly purani</b> ho (same/newer version kabhi kill nahi).</item>
    /// </list>
    /// </summary>
    public static bool ShouldTerminateOlderInstance(
        Version? candidateFileVersion,
        Version currentVersion,
        string? candidateExePath,
        string? candidateUserName,
        string currentUserName,
        int candidatePid,
        int currentPid)
    {
        if (candidatePid == currentPid) return false;
        // Windows path ("C:\RSH-old\RSH.exe") ka file naam Linux par bhi
        // sahi nikle — backslash ko separator mano (Windows par bhi
        // DirectorySeparatorChar hi hai, is liye behaviour ek jaisa).
        string exeFileName = Path.GetFileName((candidateExePath ?? "").Replace('\\', '/'));
        if (!string.Equals(exeFileName, ExeFileName, StringComparison.OrdinalIgnoreCase)) return false;
        // Owner maloom na ho (null/khaali) to fail-closed: kill nahi.
        if (string.IsNullOrWhiteSpace(candidateUserName)) return false;
        if (!string.Equals(candidateUserName, currentUserName, StringComparison.OrdinalIgnoreCase)) return false;
        if (candidateFileVersion is null) return false;
        return candidateFileVersion < currentVersion;
    }

    /// <summary>File version string ("2.0.0.0") ko <see cref="Version"/> mein badlo; ghalat/khaali par null.</summary>
    public static Version? ParseFileVersion(string? fileVersionString)
    {
        if (string.IsNullOrWhiteSpace(fileVersionString)) return null;
        return Version.TryParse(fileVersionString.Trim(), out Version? v) ? v : null;
    }

    /// <summary>
    /// Purani version ke RSH instance terminate karo, phir gin-ti wapas karo.
    /// <para>
    /// FAISLA + MECHANISM (v2.0.2): v2.0.1 mein purani blank v2.0.0 instance
    /// nayi instance ko mutex par rokti thi — aur restart ke baad bhi
    /// autostart (Run key, purane folder ki taraf) use dobara la sakta tha.
    /// Is liye startup par, versioned mutex lene SE PEHLE: (1) legacy
    /// unversioned mutex ko probe karo — v2.0.1 aur pehle ki har instance
    /// use pakarti thi, is liye holder ka hona hi purane instance ki
    /// nishani hai (probe par koi holder na ho to enumeration chhoro);
    /// (2) holder ho to <c>RSH</c> naam ke processes mein se sirf wohi
    /// terminate karo jo <see cref="ShouldTerminateOlderInstance"/> ke
    /// mutabiq hon — same user + <c>RSH.exe</c> + file version &lt; current;
    /// (3) har candidate ke liye exit ka wait timeout ke saath
    /// (default 5 s); har step <paramref name="log"/> mein likho; kisi
    /// candidate tak access fail ho (MainModule/owner) to use skip karo +
    /// log karo, kabhi blind kill nahi. Same/newer version ya doosre user
    /// ka process kabhi kill nahi hota.
    /// </para>
    /// <para>
    /// Non-Windows par kuch nahi karta (0 wapas) — Linux sandbox mein
    /// asal Windows process enumeration verify nahi ho sakta; decision
    /// logic ke unit test <see cref="ShouldTerminateOlderInstance"/> par hain.
    /// </para>
    /// </summary>
    /// <param name="currentVersion">Is instance ki assembly version (4 hissa, masalan 2.0.2.0).</param>
    /// <param name="log">Har faisla/step ki log line (null = khamoshi).</param>
    /// <param name="waitTimeout">Kill ke baad exit ka wait; null = 5 s.</param>
    /// <returns>Terminate kiye gaye (exit confirm hue) processes ki gin-ti.</returns>
    public static int TerminateOlderInstances(
        Version currentVersion,
        Action<string>? log = null,
        TimeSpan? waitTimeout = null)
    {
        if (!OperatingSystem.IsWindows())
        {
            log?.Invoke("TerminateOlderInstances: non-Windows — kuch nahi karna.");
            return 0;
        }

        // (1) Legacy mutex probe: koi purana instance hai hi nahi to aage nahi barho.
        if (!IsLegacyMutexHeld())
        {
            log?.Invoke("TerminateOlderInstances: legacy mutex ka koi holder nahi — purana instance nahi hai.");
            return 0;
        }

        TimeSpan timeout = waitTimeout ?? TimeSpan.FromSeconds(5);
        int currentPid = Environment.ProcessId;
        string currentUser = Environment.UserName;
        int terminated = 0;

        Process[] candidates;
        try { candidates = Process.GetProcessesByName(ProcessName); }
        catch (Exception ex)
        {
            log?.Invoke($"TerminateOlderInstances: process list nahi mili — kuch terminate nahi kiya ({ex.Message}).");
            return 0;
        }

        foreach (Process proc in candidates)
        {
            using (proc)
            {
                if (proc.Id == currentPid) continue;
                try
                {
                    ProcessModule? module = proc.MainModule;
                    string? exePath = module?.FileName;
                    Version? fileVersion = module is null
                        ? null
                        : ParseFileVersion(FileVersionInfo.GetVersionInfo(module.FileName).FileVersion);
                    string? owner = GetProcessUserName(proc);
                    if (!ShouldTerminateOlderInstance(
                            fileVersion, currentVersion, exePath, owner, currentUser, proc.Id, currentPid))
                    {
                        log?.Invoke(
                            $"TerminateOlderInstances: skip pid={proc.Id} exe={exePath ?? "?"} " +
                            $"version={fileVersion?.ToString() ?? "?"} user={owner ?? "?"} (kill criteria poori nahi).");
                        continue;
                    }

                    log?.Invoke(
                        $"TerminateOlderInstances: terminating older instance pid={proc.Id} " +
                        $"exe={exePath} version={fileVersion} user={owner}.");
                    proc.Kill();
                    if (proc.WaitForExit((int)timeout.TotalMilliseconds))
                    {
                        terminated++;
                        log?.Invoke($"TerminateOlderInstances: pid={proc.Id} exit ho gaya.");
                    }
                    else
                    {
                        log?.Invoke(
                            $"TerminateOlderInstances: WARNING pid={proc.Id} {timeout.TotalSeconds}s mein exit nahi hua — " +
                            "dobara kill nahi kar rahe (single attempt).");
                    }
                }
                catch (Exception ex)
                {
                    // Access fail / process beech mein band — skip + log, kabhi blind kill nahi.
                    log?.Invoke($"TerminateOlderInstances: skip pid={proc.Id} — access/inspect fail ({ex.Message}).");
                }
            }
        }

        return terminated;
    }

    /// <summary>Legacy unversioned mutex ka koi holder hai? (Probe sirf kholta hai, acquire nahi karta.)</summary>
    private static bool IsLegacyMutexHeld()
    {
        try
        {
            using var probe = new Mutex(initiallyOwned: false, DefaultMutexName, out bool createdNew);
            return !createdNew;
        }
        catch
        {
            // Probe fail = pata nahi — fail-closed: holder mana jata hai,
            // taake enumeration faisla kare (blind kill phir bhi nahi hota).
            return true;
        }
    }

    /// <summary>
    /// Process ka owner user naam (DOMAIN\user ya user) — Windows-only P/Invoke
    /// (OpenProcessToken + GetTokenInformation + LookupAccountSid). Maloom na
    /// ho sake to null — caller fail-closed skip kare.
    /// </summary>
    private static string? GetProcessUserName(Process proc)
    {
        const uint processQueryInformation = 0x0400;
        const uint tokenQuery = 0x0008;
        const int tokenUser = 1;

        IntPtr hProcess = OpenProcess(processQueryInformation, false, proc.Id);
        if (hProcess == IntPtr.Zero) return null;
        try
        {
            if (!OpenProcessToken(hProcess, tokenQuery, out IntPtr hToken) || hToken == IntPtr.Zero) return null;
            try
            {
                // Pehle size lo, phir TOKEN_USER parho (pehla field SID pointer hai).
                GetTokenInformation(hToken, tokenUser, IntPtr.Zero, 0, out uint needed);
                if (needed == 0) return null;
                IntPtr buffer = Marshal.AllocHGlobal((int)needed);
                try
                {
                    if (!GetTokenInformation(hToken, tokenUser, buffer, needed, out _)) return null;
                    IntPtr sid = Marshal.ReadIntPtr(buffer);
                    var name = new StringBuilder(256);
                    var domain = new StringBuilder(256);
                    uint nameLen = (uint)name.Capacity;
                    uint domainLen = (uint)domain.Capacity;
                    if (!LookupAccountSid(null, sid, name, ref nameLen, domain, ref domainLen, out _)) return null;
                    string user = name.ToString();
                    string dom = domain.ToString();
                    return dom.Length > 0 ? $"{dom}\\{user}" : user;
                }
                finally { Marshal.FreeHGlobal(buffer); }
            }
            finally { CloseHandle(hToken); }
        }
        finally { CloseHandle(hProcess); }
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint dwDesiredAccess, bool bInheritHandle, int dwProcessId);

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool OpenProcessToken(IntPtr processHandle, uint desiredAccess, out IntPtr tokenHandle);

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool GetTokenInformation(
        IntPtr tokenHandle, int tokenInformationClass, IntPtr tokenInformation,
        uint tokenInformationLength, out uint returnLength);

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool LookupAccountSid(
        string? lpSystemName, IntPtr sid, StringBuilder lpName, ref uint cchName,
        StringBuilder lpReferencedDomainName, ref uint cchReferencedDomainName, out int peUse);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr hObject);

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        try { _listenCts?.Cancel(); } catch { /* bandish par khamoshi */ }
        try { _listenTask?.Wait(TimeSpan.FromSeconds(2)); } catch { /* bandish par khamoshi */ }
        _listenCts?.Dispose();
        _mutex?.Dispose();
    }
}
