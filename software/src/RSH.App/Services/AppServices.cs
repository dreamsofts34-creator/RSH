using System.IO;
using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Profiles;
using RSH.Core.Protocol;
using RSH.Core.Telemetry;
using RSH.Core.Transport;
using RSH.Output.HIDMaestro;
using RSH.Output.VJoy;
using Serilog;

namespace RSH.App.Services;

/// <summary>
/// Poori app ka composition root: tamam services ek jaga bante hain,
/// ViewModels inhe yahin se lete hain. Koi static jaadu nahi — App
/// isay banata hai aur MainWindow ko deta hai.
/// Phase H WS5: <see cref="ITransportSwitchTarget"/> — SwitchToSerialAsync /
/// SwitchToWifi / DetectAndSwitchToSerialAsync pehle se maujood thay,
/// sirf interface declaration naya hai (TransportSwitchController ka target).
/// </summary>
public sealed class AppServices : IDisposable, ITransportSwitchTarget
{
    public AppServices()
    {
        DataDirectory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "RSH");
        Directory.CreateDirectory(DataDirectory);
        Directory.CreateDirectory(LogDirectory);

        SettingsStore = new AppSettingsStore(Path.Combine(DataDirectory, "settings.json"));
        Settings = SettingsStore.Load();

        ProfileStore = new ProfileStore(Path.Combine(DataDirectory, "profiles.json"));
        Profiles = ProfileStore.Load();

        // Phase D (T10 mukammal — V-C1 ka 4211 pin sirf temporary tha):
        // listen port EPHEMERAL (0) hai; HELO data socket se jata hai
        // (SendHelloV2Async — source port = listen port), taake Phase D
        // firmware (W3) data HELO ke remotePort par bheje. HELLO reply
        // (92 B) usi socket par aata hai aur BoardSession ko nonce/key
        // deta hai (GET_CAPS / signed REBOOT ke liye).
        // NOTE (dependency, W3 firmware): firmware v4.0.0 abhi bhi data
        // FIXED 4211 par bhejta hai (net.cpp PORT_DATA) — us board ke
        // saath ephemeral listen par data nahi aayega jab tak firmware
        // remotePort na bhejne lage. Ye switch task ke mutabiq hai;
        // real-board compat firmware side ke saath aayega.
        _udpSource = new UdpWheelSource();
        Source = _udpSource;
        BoardSession = new BoardSession();
        Source.HelloReplyReceived += OnHelloReply;
        Hello = new HelloClient(
            token => _udpSource.SendHelloV2Async(Settings.EspIp, HelloClient.EspHelloPort, newSession: false, token),
            Settings.EspIp);
        Output = CreateOutput();
        ActiveOutputBackend = OutputBackendSelector.Resolve(
            OutputBackendSelector.Parse(Settings.OutputBackend),
            Settings.ExperimentalHidMaestroEnabled);
        Engine = new WheelEngine(Source, Output, ProfileStore.GetActive(Profiles))
        {
            OutputEnabled = Settings.OutputEnabled,
        };

        // Phase H (TELEMETRY-READER.md §4): per-car telemetry — readers
        // sirf poll par map kholte hain (game na ho to idle, koi error
        // nahi); watcher 2 Hz par identity parhta hai aur 2 s stable ke
        // baad switcher ko snapshot deta hai. Calibration board-bound
        // rehti hai — switch sirf profile ka tuning hissa badalta hai.
        TelemetryWatcher = new TelemetryWatcher([
            new AcSharedMemoryReader(),
            new AccSharedMemoryReader(),
            new IRacingSharedMemoryReader(),
        ]);
        AutoSwitcher = new ProfileAutoSwitcher();
        TelemetryWatcher.SnapshotChanged += OnTelemetrySnapshot;
        Current = this;
    }

    /// <summary>Chal rahi app ka wahid instance — Views yahin se services lete hain (App banata hai).</summary>
    public static AppServices Current { get; set; } = null!;

    public string DataDirectory { get; }
    public string LogDirectory => Path.Combine(DataDirectory, "logs");

    public AppSettingsStore SettingsStore { get; }
    public AppSettings Settings { get; }

    public ProfileStore ProfileStore { get; }
    public ProfileFile Profiles { get; private set; }

    /// <summary>
    /// Active wheel source (Wi-Fi par UdpWheelSource, serial par
    /// SerialWheelSource) — engine aur ViewModels isi ko parhte hain.
    /// Transport switch par naya instance banta hai (SwitchToSerialAsync /
    /// SwitchToWifiAsync); engine bhi dobara banta hai (RestartEngineWithDevice
    /// jaisa pattern). UdpWheelSource wali khasiyat (ProcessDatagram test seam)
    /// ke liye <see cref="UdpSource"/> dekho.
    /// </summary>
    public IWheelSource Source { get; private set; }

    /// <summary>Wi-Fi source (app ki zindagi bhar ek hi instance — switch par Stop hota hai, dispose nahi; tests ka ProcessDatagram seam yahi hai).</summary>
    public UdpWheelSource UdpSource => _udpSource;
    private readonly UdpWheelSource _udpSource;

    /// <summary>Active transport (Wi-Fi ya USB-serial).</summary>
    public TransportKind ActiveTransport { get; private set; } = TransportKind.Wifi;

    /// <summary>Serial link (sirf serial active ho to — control transport bhi isi par hai).</summary>
    public SerialLink? SerialLink { get; private set; }
    /// <summary>Serial source (sirf serial active ho to).</summary>
    public SerialWheelSource? SerialSource { get; private set; }
    /// <summary>Serial port ka naam (sirf serial active ho to).</summary>
    public string? SerialPortName { get; private set; }

    private SerialControlTransport? _serialControlTransport;
    private bool _serialFirstHello = true;

    /// <summary>
    /// Board control commands (GET_CAPS / REBOOT / SET_PASSWORD) ka active
    /// transport: serial par SerialControlTransport, Wi-Fi par test seam
    /// (ControlTransportOverride, production mein null = asal UDP).
    /// BoardSession methods ko yehi dena hai — warna serial mode mein
    /// control galat transport (UDP) par jata hai.
    /// </summary>
    public IControlTransport? ActiveControlTransport =>
        ActiveTransport == TransportKind.UsbSerial ? _serialControlTransport : ControlTransportOverride;

    public HelloClient Hello { get; private set; }
    /// <summary>Board session (HELLO nonce/key) — GET_CAPS / signed REBOOT / SET_PASSWORD ke liye. Key DPAPI store (Phase E WS5, SEC-D-05 CLOSED) mein rehti hai.</summary>
    public BoardSession BoardSession { get; }
    /// <summary>
    /// Active output backend — vJoy (default) ya HIDMaestro (experimental,
    /// sirf toggle on par). Engine isi <see cref="Output"/> ko chalata hai.
    /// </summary>
    public IJoystickOutput Output { get; private set; }
    /// <summary><see cref="Output"/> ka backend — settings se resolve hua (fail-closed vJoy).</summary>
    public OutputBackendKind ActiveOutputBackend { get; private set; }
    public WheelEngine Engine { get; private set; }

    /// <summary>
    /// Settings se output backend banao (fail-closed vJoy). HIDMaestro
    /// sirf jab OutputBackend "HIDMaestro" ho AUR experimental toggle on ho.
    /// </summary>
    private IJoystickOutput CreateOutput() => OutputBackendSelector.Create(
        OutputBackendSelector.Parse(Settings.OutputBackend),
        Settings.ExperimentalHidMaestroEnabled,
        () => new VJoyOutput((uint)Settings.VJoyDeviceId),
        () => new HidMaestroOutput(Settings.HidMaestroProfileId));

    /// <summary>
    /// Output backend badlo (experimental toggle ke saath): engine roko,
    /// purana output dispose (Engine.Dispose output bhi dispose karta hai),
    /// naya output + naya engine, start. Settings in-memory badalti hain —
    /// caller SaveSettingsAsync kare (Output page ka Apply command).
    /// vJoy par wapas aana hamesha mumkin hai (fallback rasta).
    /// </summary>
    public void SwitchOutputBackend(OutputBackendKind kind, bool experimental)
    {
        Settings.OutputBackend = OutputBackendSelector.ToSettingsString(kind);
        Settings.ExperimentalHidMaestroEnabled = experimental;
        Engine.Stop();
        Engine.Dispose();
        Output = CreateOutput();
        ActiveOutputBackend = OutputBackendSelector.Resolve(kind, experimental);
        Engine = new WheelEngine(Source, Output, ActiveProfile) { OutputEnabled = Settings.OutputEnabled };
        Engine.Start();
    }

    /// <summary>vJoy device badalna = naya output + naya engine (purana saaf band karke). HIDMaestro active ho to device id ka koi matlab nahi — wahi backend dobara banta hai.</summary>
    public void RestartEngineWithDevice(int deviceId)
    {
        Engine.Stop();
        Engine.Dispose();
        Output = CreateOutput();
        Engine = new WheelEngine(Source, Output, ActiveProfile) { OutputEnabled = Settings.OutputEnabled };
        Engine.Start();
    }

    /// <summary>HELLO reply: pehle BoardSession update karo (nonce/key),
    /// phir SEC-E-04 migration — purani settings.json / is session mein
    /// darj password KeyStore mein lao. UDP receive thread par hai, is liye
    /// file I/O yahan nahi hota: migrate ho jaye to settings ka dobara save
    /// background par fire-and-forget hai (fail ho to agle HELLO par dobara
    /// koshish hogi — legacy value in-memory rehti hai). Aakhir mein
    /// SEC-E-14: naya session ho to GET_CAPS khud fetch ho (background par).</summary>
    private void OnHelloReply(HelloReply reply)
    {
        BoardSession.UpdateFromHello(reply);
        TryMigrateWifiPassword();
        MaybeRefreshCapsOnHello(reply);
    }

    /// <summary>
    /// SEC-E-14: (re)connect par ek HMAC GET_CAPS — naya session (naya
    /// nonce) ho ya caps abhi tak mile hi na hon to background par fetch
    /// karo. Har HELLO par nahi: wahi nonce dobara aaye to kuch nahi hota
    /// (1 Hz HELO se control channel spam nahi hota); fail hua fetch
    /// 30 s baad dobara koshish karta hai. Kabhi throw nahi karta aur
    /// UDP receive thread ko kabhi block nahi karta.
    /// </summary>
    private void MaybeRefreshCapsOnHello(HelloReply reply)
    {
        try
        {
            bool newSession = _lastHelloNonce is null || !reply.Nonce.AsSpan().SequenceEqual(_lastHelloNonce);
            _lastHelloNonce = reply.Nonce.ToArray();
            if (!BoardSession.CanSendControl) return;
            bool retryDue = LastBoardCaps is null &&
                DateTimeOffset.UtcNow - _lastCapsAttemptUtc >= TimeSpan.FromSeconds(30);
            if (!newSession && !retryDue) return;
            _ = Task.Run(() => RefreshBoardCapsAsync(), CancellationToken.None);
        }
        catch (Exception) { /* caps refresh kabhi HELLO path ko nahi torta */ }
    }

    /// <summary>
    /// SEC-E-04 migration (best-effort, kabhi throw nahi): pehle disk wali
    /// legacy password (purani settings.json se), phir is session mein UI
    /// se darj in-memory password — jo bhi mile, KeyStore mein lao jab
    /// store khaali ho. Migrate ho jaye to source saaf karo aur settings
    /// dobara save karo taake disk par plaintext ka nishan khatam ho.
    /// Board abhi maloom na ho (HELLO nahi aaya) to kuch mat karo — agle
    /// HELLO par dobara koshish hogi.
    /// </summary>
    private void TryMigrateWifiPassword()
    {
        try
        {
            if (BoardSession.BoardMac is null) return;
            bool migrated = false;
            if (!string.IsNullOrEmpty(Settings.LegacyWheelWifiPassword) &&
                BoardSession.MigrateLegacyPassword(Settings.LegacyWheelWifiPassword))
            {
                Settings.LegacyWheelWifiPassword = "";
                migrated = true;
            }
            // UI se darj in-memory password (Connection page / wizard) bhi
            // store mein lao jab store khaali ho — warna har restart par
            // dobara darj karna parta.
            if (!string.IsNullOrEmpty(Settings.WheelWifiPassword) &&
                BoardSession.MigrateLegacyPassword(Settings.WheelWifiPassword))
            {
                Settings.WheelWifiPassword = "";
                migrated = true;
            }
            if (migrated)
            {
                _ = SaveSettingsAsync().ContinueWith(
                    static _ => { /* best-effort — fail ho to agle HELLO par dobara */ },
                    TaskScheduler.Default);
            }
        }
        catch (Exception) { /* migration kabhi HELLO path ko nahi torti */ }
    }

    /// <summary>
    /// SEC-E-04: Wi-Fi join / WebOTA ke liye password ka wahid read path —
    /// pehle DPAPI KeyStore (board jana hua ho to), warna in-memory buffer
    /// (UI se abhi darj hua, HELLO ka intezar). settings.json se kabhi nahi.
    /// Khaali = password kahin nahi — user se darj karwao.
    /// </summary>
    public string GetWheelPassword()
    {
        try
        {
            if (BoardSession.TryGetWifiPassword(out string? stored) && !string.IsNullOrEmpty(stored))
                return stored;
        }
        catch (Exception) { /* store band — in-memory fallback */ }
        return Settings.WheelWifiPassword;
    }

    /// <summary>ESP IP badalna = naya HELO client (wahi data socket, naya destination). Sirf Wi-Fi mode mein — serial HELO ka koi IP nahi hota, is liye serial mode mein kuch nahi hota.</summary>
    public void RestartHello(string espIp)
    {
        if (ActiveTransport != TransportKind.Wifi) return; // serial mode: IP ka koi matlab nahi
        Hello.Dispose();
        Hello = new HelloClient(
            token => _udpSource.SendHelloV2Async(espIp, HelloClient.EspHelloPort, newSession: false, token),
            espIp);
        Hello.Start();
    }

    /// <summary>
    /// USB-serial par switch karo: port kholo, serial source + link lagao,
    /// engine dobara banao (naye source ke saath), HELO serial se bhejo.
    /// Port khul na sake to (false, wajah) aur transport Wi-Fi hi rehta
    /// hai (fail-closed — aadhi switch nahi hoti). Pehla HELO naya session
    /// hai (naya nonce — board ka session shuru hota hai). Kamyabi par
    /// GET_CAPS khud hota hai (OnHelloReply → MaybeRefreshCapsOnHello →
    /// ActiveControlTransport = serial).
    /// </summary>
    public async Task<(bool Ok, string? Error)> SwitchToSerialAsync(string portName, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(portName);
        if (ActiveTransport == TransportKind.UsbSerial && SerialPortName == portName)
            return (true, null);
        SerialLink link;
        try
        {
            link = new SerialLink(new SystemSerialPortAdapter(portName));
            link.Start();
        }
        catch (Exception ex)
        {
            return (false, $"Serial port '{portName}' nahi khul saka: {ex.GetType().Name}: {ex.Message}");
        }
        try
        {
            await SwitchToSerialLinkAsync(link, cancellationToken).ConfigureAwait(false);
            return (true, null);
        }
        catch (Exception ex)
        {
            try { link.Dispose(); } catch { /* switch fail par khamoshi */ }
            return (false, $"Serial par switch nahi ho saka: {ex.Message}");
        }
    }

    /// <summary>
    /// Test seam (internal): pehle se bana link de kar serial par switch
    /// karo (fake port — RSH.App.Tests). Link ki malkiyat AppServices le
    /// leta hai (SwitchToWifiAsync / Dispose par band hota hai).
    /// </summary>
    internal Task SwitchToSerialLinkAsync(SerialLink link, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(link);
        _ = cancellationToken;
        // Engine + purana source roko (UDP source sirf Stop hota hai —
        // dispose nahi, taake wapas Wi-Fi par ja sake).
        Engine.Stop();
        Source.Stop();
        if (Source is SerialWheelSource oldSerial)
        {
            oldSerial.HelloReplyReceived -= OnHelloReply;
            try { oldSerial.Dispose(); } catch { /* switch par khamoshi */ }
        }
        else
        {
            Source.HelloReplyReceived -= OnHelloReply;
        }
        _serialControlTransport?.Dispose();
        _serialControlTransport = null;
        SerialLink?.Dispose();
        SerialLink = null;
        SerialSource = null;

        _serialFirstHello = true;
        SerialLink = link;
        var serialSource = new SerialWheelSource(link, ownsLink: false);
        SerialSource = serialSource;
        serialSource.HelloReplyReceived += OnHelloReply;
        _serialControlTransport = new SerialControlTransport(link);
        SerialPortName = link.PortName;
        Source = serialSource;
        ActiveTransport = TransportKind.UsbSerial;

        // HELO serial se: pehla HELO naya session (naya nonce), baad mein resume.
        Hello.Dispose();
        Hello = new HelloClient(async token =>
        {
            bool newSession = _serialFirstHello;
            _serialFirstHello = false;
            await serialSource.SendHelloV2Async(newSession, token).ConfigureAwait(false);
        });
        RebuildEngine();
        serialSource.Start();
        Hello.Start();
        return Task.CompletedTask;
    }

    /// <summary>Wi-Fi par wapas switch karo: serial link/source band, UDP source + engine dobara.</summary>
    public void SwitchToWifi()
    {
        if (ActiveTransport == TransportKind.Wifi) return;
        Engine.Stop();
        if (SerialSource is not null)
        {
            SerialSource.HelloReplyReceived -= OnHelloReply;
            try { SerialSource.Dispose(); } catch { /* switch par khamoshi */ }
        }
        _serialControlTransport?.Dispose();
        _serialControlTransport = null;
        SerialLink?.Dispose();
        SerialLink = null;
        SerialSource = null;
        SerialPortName = null;

        Source = _udpSource;
        ActiveTransport = TransportKind.Wifi;
        Hello.Dispose();
        Hello = new HelloClient(
            token => _udpSource.SendHelloV2Async(Settings.EspIp, HelloClient.EspHelloPort, newSession: false, token),
            Settings.EspIp);
        Source.HelloReplyReceived += OnHelloReply;
        RebuildEngine();
        Source.Start();
        Hello.Start();
    }

    /// <summary>Engine dobara banao (naye Source ke saath) aur start karo — RestartEngineWithDevice jaisa pattern.</summary>
    private void RebuildEngine()
    {
        try { Engine.Stop(); } catch { /* rebuild par khamoshi */ }
        try { Engine.Dispose(); } catch { /* rebuild par khamoshi */ }
        Engine = new WheelEngine(Source, Output, ActiveProfile) { OutputEnabled = Settings.OutputEnabled };
        Engine.Start();
    }

    /// <summary>
    /// USB-serial board auto-detect karo (SerialDeviceDetector): pehla port
    /// jis se valid HELLO aaye. Kamyabi par us port par switch bhi hota
    /// hai (SwitchToSerialAsync). Koi board na mile to (false, wajah).
    /// </summary>
    public async Task<(bool Ok, string? Error)> DetectAndSwitchToSerialAsync(CancellationToken cancellationToken = default)
    {
        var found = await SerialDeviceDetector.DetectOnSystemPortsAsync(cancellationToken: cancellationToken).ConfigureAwait(false);
        if (found is null)
            return (false, "Koi RSH board USB-serial par nahi mila — cable lagi hai aur board on hai?");
        return await SwitchToSerialAsync(found.PortName, cancellationToken).ConfigureAwait(false);
    }

    /// <summary>
    /// Board ko HMAC-signed REBOOT (0x09) bhejo — SEC-D-01: RSTX hata
    /// diya gaya hai, restart sirf isi se hota hai. Session key na ho
    /// to (false, wajah) — khamoshi se purana rasta kabhi nahi.
    /// Serial mode mein reboot serial transport par jata hai
    /// (<see cref="ActiveControlTransport"/>).
    /// </summary>
    public async Task<(bool Sent, string? Error)> RequestBoardRebootAsync(CancellationToken cancellationToken = default) =>
        await BoardSession.SendRebootAsync(Settings.EspIp, ActiveControlTransport, cancellationToken: cancellationToken).ConfigureAwait(false);

    /// <summary>
    /// Aakhri kamyab GET_CAPS (SEC-E-14): connect/(re)connect par khud
    /// fetch hota hai (dekho <see cref="RefreshBoardCapsAsync"/> aur
    /// OnHelloReply) — board reboot ke baad dashboard khaali nahi rehta.
    /// null = abhi tak caps nahi mile (board nahi mila ya session nahi).
    /// </summary>
    public BoardCaps? LastBoardCaps { get; private set; }

    /// <summary>
    /// <see cref="LastBoardCaps"/> badla — ViewModels (Dashboard/Setup)
    /// yahan subscribe karein. SetupViewModel.LastBoardCaps (static) ka
    /// bridge W2 ki taraf hai — SetupViewModel ctor mein:
    /// <c>services.BoardCapsUpdated += () => { if (services.LastBoardCaps is { } c) LastBoardCaps = c; };</c>
    /// </summary>
    public event Action? BoardCapsUpdated;

    private byte[]? _lastHelloNonce;
    private DateTimeOffset _lastCapsAttemptUtc = DateTimeOffset.MinValue;
    private int _capsRefreshInFlight; // 0/1 — Interlocked (dobara fetch nahi)

    /// <summary>
    /// Test seam (internal): <see cref="RefreshBoardCapsAsync"/> is transport
    /// se GET_CAPS bheje — production mein null (asal UdpControlTransport).
    /// RSH.App.Tests yahan in-memory fake transport deta hai: sandbox mein
    /// UDP loopback sendto EPERM se block hota hai (dekho
    /// test-env/virtual-wheel/wheel.py), is liye auto-fetch ka gate test is
    /// seam se chalta hai; asal UDP rasta Windows-PC gate + rig par verify
    /// hota hai.
    /// </summary>
    internal IControlTransport? ControlTransportOverride { get; set; }

    /// <summary>
    /// HMAC-signed GET_CAPS bhejo (BoardSession public API — SEC-E-14):
    /// board ki capabilities + pin table lao aur <see cref="LastBoardCaps"/>
    /// mein rakho. Ek waqt mein ek hi fetch (doosra caller foran maujooda
    /// value paata hai). Session (nonce/key) na ho to (null, wajah) — koi
    /// unsigned request kabhi nahi. Board 5 s mein jawab na de to timeout.
    /// </summary>
    public async Task<(BoardCaps? Caps, string? Error)> RefreshBoardCapsAsync(CancellationToken cancellationToken = default)
    {
        if (!BoardSession.CanSendControl)
            return (LastBoardCaps, "Board session (nonce/key) abhi nahi mila — HELLO ka intezar hai.");
        if (Interlocked.Exchange(ref _capsRefreshInFlight, 1) != 0)
            return (LastBoardCaps, null); // pehle se chal raha hai — dobara nahi
        try
        {
            _lastCapsAttemptUtc = DateTimeOffset.UtcNow;
            using var cts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            cts.CancelAfter(TimeSpan.FromSeconds(5));
            var (caps, error) = await BoardSession.GetCapsAsync(Settings.EspIp, ActiveControlTransport, cancellationToken: cts.Token).ConfigureAwait(false);
            if (caps is null) return (LastBoardCaps, error);
            LastBoardCaps = caps;
            BoardCapsUpdated?.Invoke();
            return (caps, null);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            return (LastBoardCaps, "Board se 5 s mein GET_CAPS ka jawab nahi aaya.");
        }
        catch (Exception ex)
        {
            return (LastBoardCaps, $"GET_CAPS nahi ho saka: {ex.Message}");
        }
        finally
        {
            Interlocked.Exchange(ref _capsRefreshInFlight, 0);
        }
    }

    /// <summary>
    /// Profiles disk se dobara parho aur engine ko nayi active profile do —
    /// app restart ke baghair (Phase F, SW-14 restore live reload). Restore
    /// (backup/import ya game se wapsi) ke baad kaam aata hai: profiles.json
    /// bahar se badli ho to in-memory <see cref="Profiles"/> stale rehta hai;
    /// ye use taaza karta hai. Disk par kuch nahi likhta (sirf parhta hai),
    /// is liye restore ki hui file dobara save karke kharab nahi hoti.
    /// ViewModels <see cref="ProfilesReloaded"/> par subscribe karke apni
    /// selection dobara parh sakte hain (ProfilesSystem.SelectedProfile).
    /// Engine ka instance wahi rehta hai — restart nahi hota.
    /// </summary>
    public event Action? ProfilesReloaded;

    /// <summary>
    /// <see cref="ProfilesReloaded"/> ka live-reload rasta: disk se
    /// profiles dobara load karo, engine ko nayi active profile do, aur
    /// ViewModels ko khabar karo. Kamyabi par (true, active naam, null);
    /// load kharab ho to ProfileStore khud defaults/backup par girta hai —
    /// is liye ye kabhi throw nahi karta, hamesha koi na koi active
    /// profile deta hai. File I/O background par (U-P1).
    /// </summary>
    public async Task<(bool Ok, string ActiveProfileName, string? Error)> ReloadProfilesAsync(CancellationToken cancellationToken = default)
    {
        try
        {
            Profiles = await Task.Run(() => ProfileStore.Load(), cancellationToken).ConfigureAwait(false);
            Engine.SetProfile(ActiveProfile);
        }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            return (false, Profiles.ActiveProfileName, $"Profiles dobara load nahi hue: {ex.Message}");
        }
        ProfilesReloaded?.Invoke();
        return (true, Profiles.ActiveProfileName, null);
    }

    public WheelProfile ActiveProfile => ProfileStore.GetActive(Profiles);

    /// <summary>
    /// Phase H: per-car telemetry watcher (2 Hz poll, 2 s debounce) —
    /// app start par <see cref="StartRuntime"/> mein start hota hai,
    /// exit par <see cref="Dispose"/> mein band. Game na ho to idle.
    /// </summary>
    public TelemetryWatcher TelemetryWatcher { get; }

    /// <summary>Phase H: auto-switch ka faisla (manual override yahin rehta hai).</summary>
    public ProfileAutoSwitcher AutoSwitcher { get; }

    /// <summary>
    /// Phase H: car profile auto-switch hua — Dashboard toast dikhata hai
    /// ("Profile: Ferrari 488 GT3 (auto)"). UI thread par nahi aata
    /// (watcher timer thread) — subscriber ko dispatcher par marshall karna hai.
    /// </summary>
    public event Action<WheelProfile, string>? CarProfileAutoSwitched;

    /// <summary>Phase H: aakhri auto-switch ka message (toast ke liye) — null = abhi koi switch nahi hua.</summary>
    public string? LastAutoSwitchMessage { get; private set; }

    /// <summary>
    /// Phase H: stable telemetry snapshot par auto-switch — sirf profile
    /// ka tuning hissa badalta hai (calibration board-bound rehti hai,
    /// D21). Switch hone par <see cref="CarProfileAutoSwitched"/> uthta
    /// hai. Kabhi throw nahi karta (watcher timer thread ko nahi maarta).
    /// </summary>
    private void OnTelemetrySnapshot(GameTelemetrySnapshot snap)
    {
        try
        {
            var target = AutoSwitcher.TrySwitch(snap, Profiles.Profiles, Profiles.ActiveProfileName, out string reason);
            if (target is null) return;
            Profiles.ActiveProfileName = target.Name;
            LastAutoSwitchMessage = reason;
            _ = Task.Run(async () =>
            {
                try { await SaveProfilesAndApplyAsync().ConfigureAwait(false); }
                catch (Exception) { /* switch ka save fail — in-memory active phir bhi badla hua hai */ }
            });
            CarProfileAutoSwitched?.Invoke(target, reason);
        }
        catch (Exception) { /* auto-switch kabhi watcher ko nahi torta */ }
    }

    public void StartRuntime()
    {
        Source.Start();
        Hello.Start();
        Engine.Start();
        TelemetryWatcher.Start();
        Log.Information("RSH runtime shuru — profile {Profile}", ActiveProfile.Name);
    }

    /// <summary>Profiles ko mehfooz karo aur engine ko nayi active profile do.</summary>
    public void SaveProfilesAndApply()
    {
        ProfileStore.Save(Profiles);
        Engine.SetProfile(ActiveProfile);
    }

    /// <summary>
    /// <see cref="SaveProfilesAndApply"/> ka async rasta (U-P1 / A9):
    /// file I/O background par — ViewModel commands yahin se save karte
    /// hain, UI thread kabhi block nahi hota. Engine.SetProfile in-memory
    /// hai (lock-protected), is liye us ke liye thread koi farq nahi parta.
    /// </summary>
    public async Task SaveProfilesAndApplyAsync(CancellationToken cancellationToken = default)
    {
        await ProfileStore.SaveAsync(Profiles, cancellationToken).ConfigureAwait(false);
        Engine.SetProfile(ActiveProfile);
    }

    /// <summary>Ek profile ki settings badal kar save + engine par laagu (debounce caller ki zimmedari).</summary>
    public void UpdateActiveProfile(WheelProfile updated)
    {
        int idx = Profiles.Profiles.FindIndex(p => p.Name == Profiles.ActiveProfileName);
        if (idx >= 0) Profiles.Profiles[idx] = updated;
        SaveProfilesAndApply();
    }

    /// <summary>
    /// <see cref="UpdateActiveProfile"/> ka async rasta (U-P1 / A9):
    /// in-memory update foran hota hai (engine ko nayi profile isi lamhe
    /// milti hai), file I/O background par hota hai. Fire-and-forget
    /// caller (debounced slider save) ke liye mehfooz hai: save fail bhi
    /// ho to in-memory state durust rehti hai aur agla save dobara likhta hai.
    /// </summary>
    public async Task UpdateActiveProfileAsync(WheelProfile updated)
    {
        int idx = Profiles.Profiles.FindIndex(p => p.Name == Profiles.ActiveProfileName);
        if (idx >= 0) Profiles.Profiles[idx] = updated;
        await SaveProfilesAndApplyAsync().ConfigureAwait(false);
    }

    public void SaveSettings()
    {
        SettingsStore.Save(Settings);
        Engine.OutputEnabled = Settings.OutputEnabled;
    }

    /// <summary>
    /// <see cref="SaveSettings"/> ka async rasta (U-P1 / A9): file I/O
    /// background par — ViewModel property handlers/commands yahin se
    /// save karte hain, UI thread kabhi block nahi hota.
    /// </summary>
    public async Task SaveSettingsAsync(CancellationToken cancellationToken = default)
    {
        await SettingsStore.SaveAsync(Settings, cancellationToken).ConfigureAwait(false);
        Engine.OutputEnabled = Settings.OutputEnabled;
    }

    public void Dispose()
    {
        try { TelemetryWatcher.Dispose(); } catch { /* bandish par khamoshi */ }
        try { Engine.Stop(); } catch { /* bandish par khamoshi */ }
        Hello.Dispose();
        try { SerialSource?.Dispose(); } catch { /* bandish par khamoshi */ }
        _serialControlTransport?.Dispose();
        SerialLink?.Dispose();
        try { _udpSource.Dispose(); } catch { /* bandish par khamoshi */ }
        Engine.Dispose();
        BoardSession.Dispose();
    }
}
