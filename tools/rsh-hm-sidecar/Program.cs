// RSH HIDMaestro sidecar (G2, production).
//
// JSON-lines over stdin/stdout (dev / when RSH itself runs elevated),
// or over a loopback TCP connect-back when spawned elevated via
// `runas` (handle redirection does not survive a runas launch).
// One command per line, one JSON reply per line. The sidecar owns the
// whole HIDMaestro lifecycle: HMContext -> InstallDriver() -> custom
// profile (or the G29 catalog profile, v2.0.16) -> CreateController ->
// SubmitState / SubmitRawReport -> Dispose. Rust never touches the SDK.
//
// Commands: ping, version, describe-profile, install-driver, create,
// submit, submit-raw, neutral, dispose, exit. See PROTOCOL.md.
//
// AOT-safe: replies go through the source-generated context
// (SidecarJson.cs); requests are parsed with JsonDocument. No
// anonymous types, no Assembly.GetName().Version (the SDK version is
// the pinned constant), no reflection — that is what the spike's
// IL3050/IL2026 warnings came from.
//
// Logging goes to stderr AND the file log
// (%PROGRAMDATA%\RSH\logs\rsh-sidecar-YYYYMMDD.log, v2.0.11 P0 —
// stderr is discarded on the TCP path and when elevated).
// stdout is the protocol.
using System.Diagnostics;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using HIDMaestro;

namespace RshHmSidecar;

internal static class Program
{
    static HMContext? ctx;
    static HMController? ctl;
    static string? profileId;
    static int inputReportSize;
    static long submitCount;
    static long lastLatencyUs;
    // G29 spoof (v2.0.16): output/FFB packets received on a G29
    // controller and DISCARDED (the wheel has no motor). A counter
    // only — the payload is never logged, never surfaced to the user.
    // Logged every 1000th packet (see the `OutputReceived`
    // subscription in `Create`) so the file log shows the sink is
    // alive without spamming it at game FFB rates.
    static long outputReceivedCount;
    static volatile bool exitRequested;
    static string transportName = "stdio";
    static string? dllSha256Cache;
    static bool dllSha256OkCache;
    static readonly Stopwatch sw = Stopwatch.StartNew();
    // P3-2: serialises in-process file-log writers (Log is also
    // called from the submit-latency callback thread).
    static readonly object logLock = new();
    /// P2-4 (update4-fix): bound for the TCP connect-back (see
    /// `Run` — below the daemon's 30 s accept bound, far above a
    /// normal loopback connect).
    const int ConnectBackTimeoutSeconds = 15;
    // P0 (v2.0.11): the sidecar's file log. Log() used to write to
    // stderr only, which the daemon never captures on the TCP path
    // (and which is discarded when the sidecar runs elevated) — so a
    // sidecar that died before hello left no trace anywhere. The log
    // lives in %PROGRAMDATA%\RSH\logs (all-users, writable from an
    // elevated process — %APPDATA% would resolve under the admin
    // token, not the user's), one file per local day, mirroring the
    // daemon's rsh-YYYYMMDD.log naming. Best-effort: logging must
    // never break the protocol loop.
    static string? logFilePath;
    static bool logFilePathResolved;
    // P3-3 (update4-fix): the day the cached log path was resolved
    // for. The path is date-based, so a sidecar that runs past
    // midnight must re-resolve (a new day's file) instead of
    // writing into yesterday's file forever.
    static DateTime logFileDate;

    static string? LogFilePath()
    {
        // P3-3: re-resolve when the day rolls over. A failed
        // resolution is NOT latched — the next Log() retries
        // (the old code resolved once and never retried, so a
        // transient %PROGRAMDATA% failure silenced the file log
        // for the whole session).
        if (logFilePathResolved && logFileDate == DateTime.Now.Date)
        {
            return logFilePath;
        }

        try
        {
            var dir = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
                "RSH",
                "logs");
            Directory.CreateDirectory(dir);
            logFilePath = Path.Combine(dir, $"rsh-sidecar-{DateTime.Now:yyyyMMdd}.log");
            logFilePathResolved = true;
            logFileDate = DateTime.Now.Date;
        }
        catch
        {
            // Unresolved: the next Log() retries. stderr (written
            // first, in Log()) still carries the line.
            logFilePath = null;
            logFilePathResolved = false;
        }

        return logFilePath;
    }

    static int Main(string[] args)
    {
        try
        {
            return Run(args);
        }
        catch (Exception ex)
        {
            // Last resort: the process cannot even start the loop
            // (missing DLL, no listener). Never crash silently.
            // Log() also writes the file log (P0, v2.0.11).
            Log($"fatal: {ex.GetType().Name}: {ex.Message}");
            return 2;
        }
    }

    static int Run(string[] args)
    {
        var (connectBack, token) = ParseArgs(args);
        // v2.0.12 (H1 — PROVEN on the owner's PC): the loopback token
        // comes from argv (`--token <nonce>`), NOT the environment.
        // The old env transport (`RSH_HM_SIDECAR_TOKEN`) is dead: a
        // non-elevated daemon launches this sidecar elevated via
        // `runas`, and AIS builds a FRESH environment for the
        // elevated child, dropping the runtime-set variable — so the
        // sidecar exited 2 here before connecting. The nonce is
        // single-use random and briefly visible in the process list /
        // WMI — accepted; it is NEVER logged (presence only, below).
        if (connectBack is not null && string.IsNullOrEmpty(token))
        {
            // P2-7 (update4-fix): this used to bypass the file log
            // (`Console.Error` + return 2) — a token-missing
            // connect-back failure left no trace in the file log,
            // the one place the daemon-side diagnostics look.
            // Log() writes stderr AND the file log.
            Log("bad-args: --connect-back needs --token <nonce> (the one-time loopback token is required as a --token argument — it is no longer read from the environment)");
            return 2;
        }

        TextReader reader;
        TextWriter writer;
        string transport;
        TcpClient? tcp = null;
        if (connectBack is not null)
        {
            var parts = connectBack.Split(':');
            tcp = new TcpClient();
            Log($"connect-back {connectBack} (token present={!string.IsNullOrEmpty(token)})");
            // P2-4 (update4-fix): the connect is BOUNDED (15 s).
            // `TcpClient.Connect` blocks with no timeout — a
            // loopback listener that never accepts (daemon gone
            // between spawn and connect) used to hang the sidecar
            // forever, invisible to every timeout on the daemon
            // side. 15 s is far above a loopback connect (< 1 ms
            // normally, seconds on a loaded machine) and below the
            // daemon's 30 s accept bound, so a sidecar-side
            // timeout surfaces here (exit 2 + log line) instead of
            // the daemon timing out the accept with no trace.
            var connectTask = tcp.ConnectAsync(parts[0], int.Parse(parts[1], System.Globalization.CultureInfo.InvariantCulture));
            if (!connectTask.Wait(TimeSpan.FromSeconds(ConnectBackTimeoutSeconds)))
            {
                Log($"tcp connect-back timed out after {ConnectBackTimeoutSeconds} s");
                tcp.Dispose();
                return 2;
            }

            connectTask.GetAwaiter().GetResult();
            Log("tcp connected");
            var stream = tcp.GetStream();
            reader = new StreamReader(stream, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));
            writer = new StreamWriter(stream, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false))
            {
                AutoFlush = true,
            };
            // P3-6 (v2.0.13 polish): the Rust contract documents
            // "tcp-loopback" (protocol.rs, manager.rs) — the sidecar
            // sent "tcp", so the hello/reply transport never matched
            // the documented value.
            transport = "tcp-loopback";
            transportName = "tcp-loopback";
        }
        else
        {
            reader = new StreamReader(
                Console.OpenStandardInput(), new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));
            writer = new StreamWriter(
                Console.OpenStandardOutput(), new UTF8Encoding(encoderShouldEmitUTF8Identifier: false))
            {
                AutoFlush = true,
            };
            transport = "stdio";
        }

        // P3-1 (v2.0.13 polish): Ctrl+C used to call DisposeAll()
        // straight from the console signal thread while the main
        // loop could be mid-submit — DisposeAll racing the loop's
        // own dispose. Now the handler only sets the flag (and
        // cancels the default terminate so the loop can exit
        // cleanly); the loop breaks after the current command and
        // disposes on its own thread (finally block below).
        Console.CancelKeyPress += (_, e) => { e.Cancel = true; exitRequested = true; };

        var (dllSha256, dllSha256Ok) = CheckDllHash();

        // P1 (v2.0.11): the hello goes out BEFORE the HIDMaestro
        // context is created. `new HMContext()` is the prime suspect
        // for the v2.0.10 native crash (30/30 deaths between connect
        // and hello on Win10 22H2) — a native crash there is invisible
        // (no managed exception, no stderr on the TCP path) and used
        // to kill the sidecar before it ever said hello. The hello
        // already tolerates a null context (`DriverInstalled` is
        // `ctx?.IsDriverInstalled`), so sending it first turns a
        // context-creation failure into a diagnosable, recoverable
        // protocol state: the daemon gets a live sidecar, and
        // install-driver / create then report `context-unavailable`
        // instead of the sidecar vanishing.
        Log($"start transport={transport} sdk={Constants.PinnedVersion} admin={IsAdmin()} dll_ok={dllSha256Ok}");

        var hello = new SidecarReply
        {
            Ok = true,
            Hello = "rsh-hm-sidecar",
            Sidecar = "rsh-hm-sidecar",
            Sdk = Constants.PinnedVersion,
            Protocol = Constants.ProtocolVersion,
            Pid = Environment.ProcessId,
            Token = token,
            Transport = transport,
            Os = Environment.OSVersion.VersionString,
            Admin = IsAdmin(),
            DriverInstalled = ctx?.IsDriverInstalled,
            DllSha256 = dllSha256,
            DllSha256Ok = dllSha256Ok,
        };
        // P3-2 (update4-fix, verified harmless, LEFT as-is): the
        // hello WriteReply below sits OUTSIDE the try/finally that
        // disposes on every exit path — moving it inside would turn
        // a hello-write failure (daemon already gone) from a fatal
        // exit 2 into a clean exit 0, and nothing needs disposing
        // yet at this point (`ctx`/`ctl` are still null; the
        // process is exiting, so the OS reclaims the socket).
        // Deliberately not moved.
        WriteReply(writer, hello);
        Log("hello sent");

        EnsureCtxBestEffort();
        Log($"context ready={ctx is not null} driver_installed={ctx?.IsDriverInstalled}");

        try
        {
            string? line;
            while ((line = reader.ReadLine()) is not null)
            {
                SidecarReply reply;
                try
                {
                    reply = Handle(line);
                }
                catch (Exception ex)
                {
                    Log($"command failed: {ex.GetType().Name}: {ex.Message}");
                    reply = new SidecarReply
                    {
                        Ok = false,
                        Code = "internal-error",
                        Error = $"{ex.GetType().Name}: {ex.Message}",
                    };
                }

                WriteReply(writer, reply);
                if (exitRequested)
                {
                    break;
                }
            }

            Log("stdin EOF / exit — disposing");
        }
        catch (IOException ex)
        {
            // A socket reset / broken pipe while reading or writing:
            // the daemon went away. Fall through to the cleanup below
            // instead of skipping it (the virtual controller must be
            // disposed on EVERY exit path, not only a clean EOF).
            Log($"transport error: {ex.GetType().Name}: {ex.Message}");
        }
        finally
        {
            DisposeAll();
            tcp?.Dispose();
        }

        return 0;
    }

    /// <summary>
    /// Parse the command line (M5 — strict). Only
    /// `--connect-back 127.0.0.1:&lt;port&gt;` and `--token &lt;nonce&gt;`
    /// are accepted. Anything else — an unknown flag, a missing
    /// value, a non-loopback host, a bad port, or a duplicate flag —
    /// is a distinctive stderr message + fast exit 2, never a silent
    /// fallback to stdio with a half-parsed address.
    /// The token travels on argv (v2.0.12, H1): AIS drops
    /// environment variables set by the non-elevated daemon when it
    /// launches this sidecar elevated via `runas`, so the old
    /// `RSH_HM_SIDECAR_TOKEN` environment transport is dead — a
    /// connect-back with no `--token` is `bad-args`, exit 2 (see
    /// `Run`). `--token` with no value, and a duplicate `--token`,
    /// are `bad-args` too.
    /// </summary>
    static (string? ConnectBack, string? Token) ParseArgs(string[] args)
    {
        string? connectBack = null;
        string? token = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--connect-back" when i + 1 < args.Length:
                    if (connectBack is not null)
                    {
                        throw new InvalidOperationException(
                            "[sidecar] bad-args: --connect-back given twice");
                    }
                    connectBack = ValidateConnectBack(args[++i]);
                    break;
                case "--connect-back":
                    throw new InvalidOperationException(
                        "[sidecar] bad-args: --connect-back needs a value (127.0.0.1:<port>)");
                case "--token" when i + 1 < args.Length:
                    if (token is not null)
                    {
                        throw new InvalidOperationException(
                            "[sidecar] bad-args: --token given twice");
                    }
                    token = args[++i];
                    break;
                case "--token":
                    throw new InvalidOperationException(
                        "[sidecar] bad-args: --token needs a value (the one-time loopback nonce)");
                default:
                    // P3-10 (v2.0.12 scan): name the POSITION, never
                    // the value — a nonce passed as a bare argument
                    // (no `--token`) would otherwise land in the
                    // file log verbatim, breaking the "never logged
                    // anywhere" rule for the one-time token (it is
                    // single-use and dead by then, but the log must
                    // not be the place a secret ever appears).
                    throw new InvalidOperationException(
                        $"[sidecar] bad-args: unknown argument at position {i} (only --connect-back 127.0.0.1:<port> and --token <nonce> are accepted; argument values are never logged)");
            }
        }

        return (connectBack, token);
    }

    static string ValidateConnectBack(string value)
    {
        var parts = value.Split(':');
        if (parts.Length != 2 || parts[0] != "127.0.0.1"
            || !int.TryParse(parts[1], System.Globalization.NumberStyles.None,
                System.Globalization.CultureInfo.InvariantCulture, out var port)
            || port < 1 || port > 65535)
        {
            // P3-10 (v2.0.12 scan): do not echo the supplied value
            // — a malformed launch could pass the nonce here, and
            // argument values are never logged.
            throw new InvalidOperationException(
                "[sidecar] bad-args: --connect-back must be 127.0.0.1:<port> (1-65535)");
        }

        return value;
    }

    static void WriteReply(TextWriter writer, SidecarReply reply)
    {
        writer.WriteLine(JsonSerializer.Serialize(reply, SidecarJsonContext.Default.SidecarReply));
    }

    static SidecarReply Handle(string line)
    {
        using var doc = JsonDocument.Parse(line);
        var root = doc.RootElement;
        if (!root.TryGetProperty("cmd", out var cmdEl) || cmdEl.ValueKind != JsonValueKind.String)
        {
            return Error("bad-command", "missing string 'cmd'");
        }

        var cmd = cmdEl.GetString()!;
        return cmd switch
        {
            "ping" => new SidecarReply
            {
                Ok = true,
                Pong = true,
                TUs = sw.ElapsedTicks * 1_000_000.0 / Stopwatch.Frequency,
            },
            "version" => Version(),
            "describe-profile" => DescribeProfile(root),
            "install-driver" => InstallDriver(),
            "create" => Create(root),
            "submit" => Submit(root),
            "submit-raw" => SubmitRaw(root),
            "neutral" => Neutral(root),
            "dispose" => Dispose(),
            "exit" => Exit(),
            _ => Error("unknown-command", $"unknown cmd '{cmd}'"),
        };
    }

    static SidecarReply Version() => new()
    {
        Ok = true,
        Sdk = Constants.PinnedVersion,
        DriverInstalled = ctx?.IsDriverInstalled,
        Driver = ctl?.DriverVersion?.ToString(),
        Profile = profileId,
        TransportIndex = ctl?.TransportIndex,
        Os = Environment.OSVersion.VersionString,
        Admin = IsAdmin(),
        DllSha256 = dllSha256Cache,
        DllSha256Ok = dllSha256OkCache,
        Transport = transportName,
        SubmitCount = submitCount,
        OnSubmitLatencyUs = submitCount > 0 ? lastLatencyUs : null,
        OutputReceivedCount = outputReceivedCount,
    };

    /// <summary>
    /// Describes a profile: the requested one (`{"profile": ...}`),
    /// else the ACTIVE profile once a controller exists, else the
    /// custom profile. G29 spoof (v2.0.16): describing
    /// <c>logitech-g29</c> loads the catalog profile from the SDK
    /// (needs the HIDMaestro context + <c>LoadDefaultProfiles</c> —
    /// it is NOT pure like the custom descriptor build) and reports
    /// its VID/PID/descriptor/derived size (123 bytes / 12 bytes).
    /// Any other id is refused — only the two built profiles are
    /// ever described.
    /// </summary>
    static SidecarReply DescribeProfile(JsonElement root)
    {
        string? asked = null;
        if (root.TryGetProperty("profile", out var profileEl)
            && profileEl.ValueKind == JsonValueKind.String)
        {
            asked = profileEl.GetString();
        }

        var id = asked ?? profileId ?? Constants.ProfileId;
        if (id != Constants.ProfileId && id != Constants.G29ProfileId)
        {
            return Error(
                "profile-refused",
                $"profile '{id}' refused: only '{Constants.ProfileId}' and '{Constants.G29ProfileId}' are described");
        }

        if (id == Constants.G29ProfileId)
        {
            return DescribeG29Profile();
        }

        var (descriptor, derived) = BuildDescriptor();
        return new SidecarReply
        {
            Ok = true,
            Profile = Constants.ProfileId,
            Vid = Constants.Vid,
            PidValue = Constants.Pid,
            VidPidProvisional = Constants.VidPidProvisional,
            DescriptorLen = descriptor.Length,
            DescriptorHex = Convert.ToHexString(descriptor).ToLowerInvariant(),
            InputReportSize = derived,
            InputReportSizeLegacyAssumption = Constants.LegacyReportSizeAssumption,
            ButtonCount = 32,
            AxisCount = 5,
            HasHat = true,
            Backend = "hidmaestro",
        };
    }

    /// <summary>
    /// Describes the catalog <c>logitech-g29</c> profile, loaded from
    /// the SDK. <c>HMProfile.InputReportSize</c> is 0 for catalog
    /// profiles (verified against the real v2.1.0 DLL) and is never
    /// trusted — the size is derived from the descriptor (12 bytes).
    /// </summary>
    static SidecarReply DescribeG29Profile()
    {
        try
        {
            EnsureCtx();
        }
        catch (Exception ex)
        {
            // P1 (v2.0.11): see InstallDriver — a context that cannot
            // be created is `context-unavailable`, not a generic failure.
            return Error("context-unavailable", $"cannot create HIDMaestro context: {ex.Message}");
        }

        HMProfile prof;
        try
        {
            prof = LoadG29CatalogProfile(ctx!);
        }
        catch (Exception ex)
        {
            return Error(
                "profile-refused",
                $"catalog profile '{Constants.G29ProfileId}' could not be loaded: {ex.GetType().Name}: {ex.Message}");
        }

        var descriptor = prof.GetDescriptorBytes();
        if (descriptor is null)
        {
            return Error(
                "profile-refused",
                $"catalog profile '{Constants.G29ProfileId}' has no descriptor bytes — refusing to describe an unverified device");
        }

        var derived = HidReportSize.DeriveInputReportSize(descriptor);
        if (derived <= 0)
        {
            return Error(
                "create-failed",
                "could not derive the input report size from the G29 profile descriptor");
        }

        return new SidecarReply
        {
            Ok = true,
            Profile = Constants.G29ProfileId,
            Vid = prof.VendorId,
            PidValue = prof.ProductId,
            VidPidProvisional = false,
            DescriptorLen = descriptor.Length,
            DescriptorHex = Convert.ToHexString(descriptor).ToLowerInvariant(),
            InputReportSize = derived,
            InputReportSizeLegacyAssumption = Constants.LegacyReportSizeAssumption,
            ButtonCount = prof.ButtonCount,
            AxisCount = prof.AxisCount,
            HasHat = prof.HasHat,
            Backend = "hidmaestro",
        };
    }

    /// <summary>
    /// Loads the catalog <c>logitech-g29</c> profile from the SDK.
    /// <c>LoadDefaultProfiles</c> is REQUIRED before
    /// <c>GetProfile</c> — verified against the real v2.1.0 DLL:
    /// <c>GetProfile</c> with no load throws
    /// <c>NullReferenceException</c>.
    /// </summary>
    static HMProfile LoadG29CatalogProfile(HMContext context)
    {
        context.LoadDefaultProfiles();
        var prof = context.GetProfile(Constants.G29ProfileId);
        if (prof is null)
        {
            // The SDK returns null instead of throwing when the
            // catalog has no such profile — fail closed.
            throw new InvalidOperationException(
                $"catalog profile '{Constants.G29ProfileId}' not found after LoadDefaultProfiles");
        }

        return prof;
    }

    static SidecarReply InstallDriver()
    {
        try
        {
            EnsureCtx();
        }
        catch (Exception ex)
        {
            // P1 (v2.0.11): a context that cannot be created is a
            // distinct, diagnosable state — the sidecar is alive, the
            // HIDMaestro context is not. The Rust side maps this code
            // to `ContextUnavailable`, never to a generic failure.
            return Error("context-unavailable", $"cannot create HIDMaestro context: {ex.Message}");
        }

        if (ctx!.IsDriverInstalled)
        {
            // Fast path: skip the install entirely.
            return new SidecarReply
            {
                Ok = true,
                WasInstalled = true,
                NowInstalled = true,
                Outcome = "AlreadyCurrent",
            };
        }

        if (!IsAdmin())
        {
            return Error(
                "not-admin",
                "HIDMaestro driver install needs elevation; the sidecar was not started as administrator");
        }

        try
        {
            ctx.InstallDriver();
        }
        catch (UnauthorizedAccessException)
        {
            return Error(
                "not-admin",
                "driver install was refused (not administrator)");
        }
        catch (Exception ex)
        {
            return Error("driver-install-failed", $"{ex.GetType().Name}: {ex.Message}");
        }

        return new SidecarReply
        {
            Ok = true,
            WasInstalled = false,
            NowInstalled = ctx.IsDriverInstalled,
            Outcome = ctx.LastDriverInstall?.ToString(),
        };
    }

    static SidecarReply Create(JsonElement root)
    {
        var id = "rsh-wheel-custom";
        if (root.TryGetProperty("profile", out var idEl) && idEl.ValueKind == JsonValueKind.String)
        {
            id = idEl.GetString()!;
        }

        // Only the custom no-brand profile and the G29 catalog
        // profile. Every OTHER catalog profile is still refused:
        // creating one would spoof a brand. The G29 exception is
        // explicit and user-gated (default-off `g29_spoof_mode`,
        // HIDMaestro-only) — see Constants.G29ProfileId for the
        // risk disclosure. The pid.codes 0x1209/0x5253 claim is
        // unaffected: that identity is simply unused while G29 is
        // active.
        if (id != Constants.ProfileId && id != Constants.G29ProfileId)
        {
            return Error(
                "profile-refused",
                $"profile '{id}' refused: only '{Constants.ProfileId}' and '{Constants.G29ProfileId}' are built; catalog brand profiles are otherwise never created");
        }

        try
        {
            EnsureCtx();
        }
        catch (Exception ex)
        {
            // P1 (v2.0.11): see InstallDriver — a context that cannot
            // be created is `context-unavailable`, not `create-failed`.
            return Error("context-unavailable", $"cannot create HIDMaestro context: {ex.Message}");
        }

        if (!ctx!.IsDriverInstalled)
        {
            var install = InstallDriver();
            if (!install.Ok)
            {
                return install;
            }
        }

        HMProfile prof;
        int derived;
        string identityKey;
        if (id == Constants.G29ProfileId)
        {
            // The catalog profile, loaded from the SDK — never
            // hand-built. Its descriptor bytes must equal the
            // pinned 123-byte G29 descriptor exactly; a different
            // descriptor under this identity would be an unverified
            // device, so create fails instead of shipping it.
            try
            {
                prof = LoadG29CatalogProfile(ctx!);
            }
            catch (Exception ex)
            {
                return Error(
                    "profile-refused",
                    $"catalog profile '{Constants.G29ProfileId}' could not be loaded: {ex.GetType().Name}: {ex.Message}");
            }

            var descriptor = prof.GetDescriptorBytes();
            var expected = Convert.FromHexString(Constants.G29DescriptorHex);
            if (prof.Id != Constants.G29ProfileId
                || prof.VendorId != Constants.G29Vid
                || prof.ProductId != Constants.G29Pid
                || !descriptor.SequenceEqual(expected))
            {
                return Error(
                    "profile-refused",
                    $"catalog profile '{Constants.G29ProfileId}' does not match the pinned G29 identity (VID 0x046D / PID 0xC24F, 123-byte descriptor) — refusing to create an unverified device");
            }

            // The SDK's own InputReportSize is 0 for catalog profiles
            // (verified against the real v2.1.0 DLL) — derive the
            // size from the descriptor (12 bytes), never trust it.
            derived = HidReportSize.DeriveInputReportSize(descriptor);
            if (derived != Constants.G29InputReportSize)
            {
                return Error(
                    "create-failed",
                    $"G29 descriptor derives input report size {derived}, expected {Constants.G29InputReportSize}");
            }

            identityKey = Constants.G29IdentityKey;
        }
        else
        {
            var (descriptor, derivedCustom) = BuildDescriptor();
            prof = new HMProfileBuilder()
                .Id(Constants.ProfileId)
                .Name("RSH Wheel (custom)")
                .Vendor(Constants.Vendor)
                .Vid(Constants.Vid)
                .Pid(Constants.Pid)
                .ProductString(Constants.Product)
                .ManufacturerString(Constants.Vendor)
                .Type("wheel")
                .Connection("usb")
                .Descriptor(descriptor)
                .InputReportSize(derivedCustom)
                .Build();
            derived = derivedCustom;
            identityKey = Constants.IdentityKey;
        }

        // SEC-H-06 refusal rules (mirrors the C# app's adapter):
        // usbip-backed or self-reporting or non-deployable profiles
        // must never be created.
        if (prof.RequiresUsbipBackend || prof.BuildsItsOwnReports || !prof.IsDeployable)
        {
            return Error(
                "profile-refused",
                $"profile '{id}' refused (usbip={prof.RequiresUsbipBackend} ownReports={prof.BuildsItsOwnReports} deployable={prof.IsDeployable})");
        }

        ctl?.Dispose();
        ctl = null;
        var t0 = Stopwatch.GetTimestamp();
        try
        {
            ctl = ctx.CreateController(prof, identityKey);
        }
        catch (Exception ex)
        {
            return Error("create-failed", $"{ex.GetType().Name}: {ex.Message}");
        }

        var createMs = (Stopwatch.GetTimestamp() - t0) * 1000.0 / Stopwatch.Frequency;
        profileId = id;
        inputReportSize = derived;
        submitCount = 0;
        lastLatencyUs = 0;
        outputReceivedCount = 0;
        // The SDK's own per-submit latency hook — surfaced in
        // version / submit replies for diagnostics.
        ctl.OnSubmitLatencyMicros = us => Interlocked.Exchange(ref lastLatencyUs, us);

        if (id == Constants.G29ProfileId)
        {
            // FFB SINK (research §5): the G29 profile keeps its
            // 16-byte 0xFF00 Output report (the Driving Force
            // force-feedback command channel), so FH4's FFB writes
            // have somewhere to go instead of failing against a
            // missing endpoint. The wheel has no motor, so the
            // writes are DISCARDED here — a counter only, never
            // payload data, never an error to the user.
            // NOTE: `HapticReceived` does NOT exist on HMController
            // (verified against the real v2.1.0 DLL — it lives on
            // HMVRController, a VR type); `OutputReceived` is the
            // channel that carries the game's output writes.
            ctl.OutputReceived += (_, _) =>
            {
                var n = Interlocked.Increment(ref outputReceivedCount);
                if (n % 1000 == 0)
                {
                    Log($"g29 output received x{n} (discarded — no FFB motor; payload never logged)");
                }
            };
        }

        var isG29 = id == Constants.G29ProfileId;
        return new SidecarReply
        {
            Ok = true,
            Created = true,
            Profile = id,
            TransportIndex = ctl.TransportIndex,
            Driver = ctl.DriverVersion?.ToString(),
            CreateMs = createMs,
            InputReportSize = derived,
            OutputReceivedCount = outputReceivedCount,
            Capabilities = new CapabilitiesReply
            {
                Axes = isG29 ? 4 : 5,
                Buttons = isG29 ? 25 : 32,
                HasHat = true,
                InputReportSize = derived,
                SubmitStateMaxButton = 25,
                SubmitStateButtonMask = Constants.SubmitStateButtonMask,
            },
        };
    }

    static SidecarReply Submit(JsonElement root)
    {
        if (ctl is null)
        {
            return Error("no-controller", "no controller — call create first");
        }

        // G29 spoof (v2.0.16): the SubmitState frame layout does not
        // match the G29 profile — only the 12-byte raw report may be
        // submitted for it (the Rust side forces the raw path; this
        // is defence in depth, so a mismatched frame can never reach
        // the driver even if the Rust side regresses).
        if (profileId == Constants.G29ProfileId)
        {
            return Error(
                "bad-command",
                "submit (SubmitState) is not supported for the logitech-g29 profile — use submit-raw");
        }

        var axes = new Dictionary<HMAxis, float>();
        if (root.TryGetProperty("axes", out var axesEl) && axesEl.ValueKind == JsonValueKind.Object)
        {
            foreach (var a in axesEl.EnumerateObject())
            {
                if (Enum.TryParse<HMAxis>(a.Name, ignoreCase: true, out var ax))
                {
                    // L3: clamp every axis to 0..1 — the Rust side
                    // already clamps, but the sidecar is the trust
                    // boundary for the driver: a NaN / out-of-range
                    // float must never reach SubmitState. Non-finite
                    // input maps to the axis neutral (X = 0.5 centre,
                    // everything else = 0.0 released).
                    var v = (float)a.Value.GetDouble();
                    if (!float.IsFinite(v))
                    {
                        v = ax == HMAxis.X ? 0.5f : 0.0f;
                    }
                    axes[ax] = Math.Clamp(v, 0.0f, 1.0f);
                }
            }
        }

        if (!root.TryGetProperty("buttons", out var buttonsEl) || !buttonsEl.TryGetUInt32(out var buttonsRaw))
        {
            return Error("bad-command", "submit needs uint 'buttons'");
        }

        var buttons = (HMButton)(buttonsRaw & Constants.SubmitStateButtonMask);
        var t0 = Stopwatch.GetTimestamp();
        var st = new HMGamepadState { Axes = axes, Buttons = buttons, Hat = HMHat.None };
        ctl.SubmitState(in st);
        var us = (Stopwatch.GetTimestamp() - t0) * 1_000_000.0 / Stopwatch.Frequency;
        submitCount++;
        return new SidecarReply
        {
            Ok = true,
            SubmitUs = us,
            OnSubmitLatencyUs = lastLatencyUs,
            SubmitCount = submitCount,
        };
    }

    static SidecarReply SubmitRaw(JsonElement root)
    {
        if (ctl is null)
        {
            return Error("no-controller", "no controller — call create first");
        }

        if (!root.TryGetProperty("bytes", out var bytesEl) || bytesEl.ValueKind != JsonValueKind.String)
        {
            return Error("bad-command", "submit-raw needs hex string 'bytes'");
        }

        byte[] bytes;
        try
        {
            bytes = Convert.FromHexString(bytesEl.GetString()!);
        }
        catch (FormatException)
        {
            return Error("bad-command", "'bytes' is not valid hex");
        }

        if (bytes.Length != inputReportSize)
        {
            return Error(
                "bad-report-size",
                $"raw report is {bytes.Length} bytes, profile expects {inputReportSize}");
        }

        var t0 = Stopwatch.GetTimestamp();
        ctl.SubmitRawReport(bytes);
        var us = (Stopwatch.GetTimestamp() - t0) * 1_000_000.0 / Stopwatch.Frequency;
        submitCount++;
        return new SidecarReply
        {
            Ok = true,
            SubmitUs = us,
            OnSubmitLatencyUs = lastLatencyUs,
            SubmitCount = submitCount,
        };
    }

    static SidecarReply Neutral(JsonElement root)
    {
        if (ctl is null)
        {
            return Error("no-controller", "no controller — call create first");
        }

        var raw = root.TryGetProperty("raw", out var rawEl) && rawEl.ValueKind == JsonValueKind.True;
        if (raw)
        {
            var bytes = NeutralRawReport();
            ctl.SubmitRawReport(bytes);
        }
        else
        {
            // G29 spoof (v2.0.16): the SubmitState neutral frame does
            // not match the G29 profile — only the raw neutral may be
            // sent for it (the Rust side always passes raw:true
            // under G29; this is defence in depth).
            if (profileId == Constants.G29ProfileId)
            {
                return Error(
                    "bad-command",
                    "SubmitState neutral is not supported for the logitech-g29 profile — use raw:true");
            }

            ctl.SubmitState(new HMGamepadState
            {
                Axes = new Dictionary<HMAxis, float> { [HMAxis.X] = 0.5f },
                Buttons = 0,
                Hat = HMHat.None,
            });
        }

        submitCount++;
        return new SidecarReply { Ok = true, SubmitCount = submitCount };
    }

    static SidecarReply Dispose()
    {
        DisposeAll();
        return new SidecarReply { Ok = true, Disposed = true };
    }

    static SidecarReply Exit()
    {
        exitRequested = true;
        return new SidecarReply { Ok = true, Bye = true };
    }

    static SidecarReply Error(string code, string message) => new()
    {
        Ok = false,
        Code = code,
        Error = message,
    };

    /// <summary>
    /// Builds the custom profile descriptor and derives its input
    /// report size from the descriptor itself — never a hardcoded
    /// assumption. The descriptor is 124 bytes; the derived size is
    /// 15 bytes (5×16-bit axes + 32×1-bit buttons + 8-bit hat).
    /// </summary>
    static (byte[] Descriptor, int DerivedSize) BuildDescriptor()
    {
        var descriptor = new HidDescriptorBuilder().Joystick()
            .AddAxis(HMAxis.X, 16)
            .AddAxis(HMAxis.Y, 16)
            .AddAxis(HMAxis.Z, 16)
            .AddAxis(HMAxis.Rx, 16)
            .AddAxis(HMAxis.Slider, 16)
            .AddButtons(32)
            .AddHat(4)
            .Build();
        var derived = HidReportSize.DeriveInputReportSize(descriptor);
        if (derived <= 0)
        {
            throw new InvalidOperationException("could not derive the input report size from the profile descriptor");
        }

        return (descriptor, derived);
    }

    /// <summary>
    /// The neutral raw report for the ACTIVE profile: the 15-byte
    /// custom report (steering centred at u16 16383, pedals released,
    /// no buttons, hat centred 0x08 — the first out-of-range value
    /// for a null-state hat), or the 12-byte G29 report (X = 32768,
    /// pedals 0, hat neutral 0x08 low nibble, buttons 0, vendor bytes
    /// 0). Same layout the Rust crate builds in
    /// <c>neutral_raw_report</c> / <c>neutral_g29_raw_report</c>.
    /// </summary>
    static byte[] NeutralRawReport()
    {
        if (profileId == Constants.G29ProfileId)
        {
            var g29 = new byte[Constants.G29InputReportSize];
            // Hat = 0x08 (centred) in the low nibble; buttons = 0.
            g29[0] = 0x08;
            // X (steering) = 32768, little-endian.
            g29[4] = 0x00;
            g29[5] = 0x80;
            // Pedals (6-8) and vendor bytes (9-11) = 0 (already zeroed).
            return g29;
        }

        var bytes = new byte[15];
        // X (steering) = 16383, little-endian.
        bytes[0] = 0xFF;
        bytes[1] = 0x3F;
        // Y, Z, Rx, Slider = 0 (already zeroed).
        // Buttons = 0 (already zeroed).
        // Hat = 0x08 (centred).
        bytes[14] = 0x08;
        return bytes;
    }

    static void EnsureCtx() => ctx ??= new HMContext();

    static void EnsureCtxBestEffort()
    {
        try
        {
            EnsureCtx();
        }
        catch (Exception ex)
        {
            Log($"context create failed at startup (will retry on demand): {ex.GetType().Name}: {ex.Message}");
        }
    }

    static void DisposeAll()
    {
        try
        {
            ctl?.Dispose();
        }
        catch (Exception ex)
        {
            Log($"controller dispose failed: {ex.Message}");
        }

        ctl = null;
        profileId = null;
        inputReportSize = 0;
        outputReceivedCount = 0;
        try
        {
            ctx?.Dispose();
        }
        catch (Exception ex)
        {
            Log($"context dispose failed: {ex.Message}");
        }

        ctx = null;
    }

    /// <summary>
    /// SHA-256 of the HIDMaestro.Core.dll actually loaded, compared
    /// against the pinned v2.1.0 hash. Location first (the assembly
    /// as loaded), falling back to the DLL beside the exe.
    /// </summary>
    static (string? Sha256, bool Ok) CheckDllHash()
    {
        string? path = null;
        try
        {
            var location = typeof(HMContext).Assembly.Location;
            if (!string.IsNullOrEmpty(location) && File.Exists(location))
            {
                path = location;
            }
        }
        catch (Exception ex)
        {
            Log($"assembly location lookup failed: {ex.Message}");
        }

        path ??= Path.Combine(AppContext.BaseDirectory, "HIDMaestro.Core.dll");
        if (!File.Exists(path))
        {
            Log($"HIDMaestro.Core.dll not found at {path}");
            dllSha256Cache = null;
            dllSha256OkCache = false;
            return (null, false);
        }

        try
        {
            var hash = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(path))).ToLowerInvariant();
            var ok = string.Equals(hash, Constants.PinnedDllSha256, StringComparison.OrdinalIgnoreCase);
            dllSha256Cache = hash;
            dllSha256OkCache = ok;
            if (!ok)
            {
                Log($"DLL hash mismatch: got {hash}, expected {Constants.PinnedDllSha256}");
            }

            return (hash, ok);
        }
        catch (Exception ex)
        {
            Log($"DLL hash failed: {ex.Message}");
            dllSha256Cache = null;
            dllSha256OkCache = false;
            return (null, false);
        }
    }

    static bool IsAdmin()
    {
        try
        {
            using var id = System.Security.Principal.WindowsIdentity.GetCurrent();
            return new System.Security.Principal.WindowsPrincipal(id)
                .IsInRole(System.Security.Principal.WindowsBuiltInRole.Administrator);
        }
        catch
        {
            return false;
        }
    }

    static void Log(string message)
    {
        // P0 (v2.0.11): stderr AND the file log
        // (%PROGRAMDATA%\RSH\logs\rsh-sidecar-YYYYMMDD.log) — stderr
        // is discarded on the TCP path and when elevated, so the file
        // is the only trace a pre-hello death leaves. Best-effort:
        // logging must never break the protocol loop.
        // P3-3 (update4-fix): the PID rides in every log line.
        // Concurrent sidecars share one daily file and interleave
        // lines — without the PID a line cannot be attributed to
        // its process, and a pre-hello death is indistinguishable
        // from another sidecar's milestones.
        var line = $"[sidecar] [pid {Environment.ProcessId}] [{DateTime.Now:HH:mm:ss.fff}] {message}";
        try
        {
            Console.Error.WriteLine(line);
        }
        catch
        {
            // Stderr is gone; the file log below may still work.
        }

        // P3-2 (v2.0.13 polish): the file log had no locking —
        // two sidecars share one daily file, and concurrent
        // AppendAllText calls (same process via the submit-latency
        // callback thread, or a second sidecar process) could
        // collide on open and silently drop the line. A static
        // lock serialises in-process writers; FileShare.ReadWrite
        // + a short retry absorbs a cross-process open collision.
        // Logging must still never break the protocol loop.
        lock (logLock)
        {
            for (var attempt = 0; attempt < 3; attempt++)
            {
                try
                {
                    var path = LogFilePath();
                    if (path is null) break;
                    using var fs = new FileStream(path, FileMode.Append, FileAccess.Write, FileShare.ReadWrite);
                    var bytes = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false).GetBytes(line + Environment.NewLine);
                    fs.Write(bytes, 0, bytes.Length);
                    break;
                }
                catch (IOException) when (attempt < 2)
                {
                    Thread.Sleep(10);
                }
                catch
                {
                    // File log unwritable; stderr already carried the line.
                    break;
                }
            }
        }
    }
}
