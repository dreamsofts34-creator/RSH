// RSH HIDMaestro sidecar (G2, production).
//
// JSON-lines over stdin/stdout (dev / when RSH itself runs elevated),
// or over a loopback TCP connect-back when spawned elevated via
// `runas` (handle redirection does not survive a runas launch).
// One command per line, one JSON reply per line. The sidecar owns the
// whole HIDMaestro lifecycle: HMContext -> InstallDriver() -> custom
// profile -> CreateController -> SubmitState / SubmitRawReport ->
// Dispose. Rust never touches the SDK.
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
// Logging goes to stderr only. stdout is the protocol.
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
    static bool exitRequested;
    static string transportName = "stdio";
    static string? dllSha256Cache;
    static bool dllSha256OkCache;
    static readonly Stopwatch sw = Stopwatch.StartNew();

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
            try
            {
                Console.Error.WriteLine($"[sidecar] fatal: {ex.GetType().Name}: {ex.Message}");
            }
            catch
            {
                // Stderr is gone too; nothing left to do.
            }

            return 2;
        }
    }

    static int Run(string[] args)
    {
        var (connectBack, token) = ParseArgs(args);

        TextReader reader;
        TextWriter writer;
        string transport;
        TcpClient? tcp = null;
        if (connectBack is not null)
        {
            var parts = connectBack.Split(':');
            tcp = new TcpClient();
            tcp.Connect(parts[0], int.Parse(parts[1], System.Globalization.CultureInfo.InvariantCulture));
            var stream = tcp.GetStream();
            reader = new StreamReader(stream, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));
            writer = new StreamWriter(stream, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false))
            {
                AutoFlush = true,
            };
            transport = "tcp";
            transportName = "tcp";
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

        Console.CancelKeyPress += (_, _) => DisposeAll();

        var (dllSha256, dllSha256Ok) = CheckDllHash();
        EnsureCtxBestEffort();

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
        WriteReply(writer, hello);

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
        DisposeAll();
        tcp?.Dispose();
        return 0;
    }

    static (string? ConnectBack, string? Token) ParseArgs(string[] args)
    {
        string? connectBack = null;
        string? token = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--connect-back" when i + 1 < args.Length:
                    connectBack = args[++i];
                    break;
                case "--token" when i + 1 < args.Length:
                    token = args[++i];
                    break;
            }
        }

        return (connectBack, token);
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
            "describe-profile" => DescribeProfile(),
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
    };

    static SidecarReply DescribeProfile()
    {
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

    static SidecarReply InstallDriver()
    {
        try
        {
            EnsureCtx();
        }
        catch (Exception ex)
        {
            return Error("driver-install-failed", $"cannot create HIDMaestro context: {ex.Message}");
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

        // Only the custom no-brand profile. Catalog profiles are all
        // brand identities; creating one would spoof a brand.
        if (id != Constants.ProfileId)
        {
            return Error(
                "profile-refused",
                $"profile '{id}' refused: only '{Constants.ProfileId}' is built; catalog brand profiles are never created");
        }

        try
        {
            EnsureCtx();
        }
        catch (Exception ex)
        {
            return Error("create-failed", $"cannot create HIDMaestro context: {ex.Message}");
        }

        if (!ctx!.IsDriverInstalled)
        {
            var install = InstallDriver();
            if (!install.Ok)
            {
                return install;
            }
        }

        var (descriptor, derived) = BuildDescriptor();
        var prof = new HMProfileBuilder()
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
            .InputReportSize(derived)
            .Build();

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
            ctl = ctx.CreateController(prof, Constants.IdentityKey);
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
        // The SDK's own per-submit latency hook — surfaced in
        // version / submit replies for diagnostics.
        ctl.OnSubmitLatencyMicros = us => Interlocked.Exchange(ref lastLatencyUs, us);

        return new SidecarReply
        {
            Ok = true,
            Created = true,
            Profile = id,
            TransportIndex = ctl.TransportIndex,
            Driver = ctl.DriverVersion?.ToString(),
            CreateMs = createMs,
            InputReportSize = derived,
            Capabilities = new CapabilitiesReply
            {
                Axes = 5,
                Buttons = 32,
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

        var axes = new Dictionary<HMAxis, float>();
        if (root.TryGetProperty("axes", out var axesEl) && axesEl.ValueKind == JsonValueKind.Object)
        {
            foreach (var a in axesEl.EnumerateObject())
            {
                if (Enum.TryParse<HMAxis>(a.Name, ignoreCase: true, out var ax))
                {
                    axes[ax] = (float)a.Value.GetDouble();
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
    /// The 15-byte neutral raw report: steering centred (u16 16383),
    /// pedals released (0), no buttons, hat centred (0x08 — the
    /// first out-of-range value for a null-state hat). Same layout
    /// the Rust crate builds in <c>neutral_raw_report</c>.
    /// </summary>
    static byte[] NeutralRawReport()
    {
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
        try
        {
            Console.Error.WriteLine($"[{DateTime.Now:HH:mm:ss.fff}] {message}");
        }
        catch
        {
            // Logging must never break the protocol loop.
        }
    }
}
