using System.Diagnostics;
using System.Text;

namespace RSH.Core.Telemetry;

/// <summary>
/// iRacing reader — official SDK shared memory
/// (<c>Local\IRSDKMemMapFileName</c>, ~1164 KB).
/// <para>
/// Layout self-describing hai (header mein offsets — §2 rule 4 ka sab se
/// mehfooz case): header 112 B (ver @0, status @4, sessionInfoUpdate @12,
/// sessionInfoLen @16, sessionInfoOffset @20, numVars @24,
/// varHeaderOffset @28, numBuf @32, bufLen @36, varBuf[] @48 — har entry
/// 16 B: tickCount @0, bufOffset @4); var header 144 B (type @0, offset
/// @4, count @8, countAsTime @12, name @16 [32 B], desc @48, unit @112).
/// Session info YAML hai (sessionInfoOffset/len); car
/// <c>DriverInfo → Drivers → IsPlayer: 1</c> wale driver ki
/// <c>CarScreenName</c> (fallback <c>CarPath</c>), track
/// <c>WeekendInfo → TrackName</c>. State <c>SessionState</c> var se
/// (0=Invalid, 1=GetInCar, 2=Warmup, 3=ParadeLaps, 4=Racing,
/// 5=Checkered, 6=CoolDown).
/// </para>
/// <para>
/// Triple-buffer: pyirsdk ki tarah <b>second-highest tickCount</b> wala
/// buffer lo — sab se taaza buffer abhi likha ja raha ho sakta hai
/// (aadha), second-highest mukammal hai. Sirf ek buffer ho to wohi.
/// </para>
/// <para>
/// SEC-H-05: (a) TOCTOU — buffer copy se pehle aur baad mein tickCount
/// parho, badla ho to ek dafa dobara (phir bhi badle to reject);
/// (b) size bounds — session YAML zyada se zyada
/// <see cref="MaxSessionInfoBytes"/> (256 KB; asal session info chand
/// das KB hoti hai), numVars ≤ 10000, numBuf 1..4, tamam offsets map ke
/// andar — header ke jhootay offsets se kabhi bahir nahi parha jata;
/// (c) map spoofing — AC/ACC wali process-executable check yahan bhi
/// (<c>iRacingSim64DX11.exe</c> ka MainModule path), residual risk wahi
/// (tuning-only switch).
/// </para>
/// </summary>
public sealed class IRacingSharedMemoryReader : ITelemetryReader
{
    public const string MapName = @"Local\IRSDKMemMapFileName";

    /// <summary>SEC-H-05: session YAML ki max size — is se bari YAML corrupt/malicious hai.</summary>
    public const int MaxSessionInfoBytes = 262_144;

    internal const int HeaderSize = 112;
    internal const int VarBufferEntrySize = 16;
    internal const int VarHeaderSize = 144;
    internal const int MaxVars = 10_000;
    internal const int MaxBuffers = 4;

    private readonly IMemoryMapAccessor _accessor;
    private readonly Func<string, bool> _isProcessRunning;

    public IRacingSharedMemoryReader(
        IMemoryMapAccessor? accessor = null,
        Func<string, bool>? isProcessRunning = null)
    {
        _accessor = accessor ?? new WindowsMemoryMapAccessor();
        _isProcessRunning = isProcessRunning ?? DefaultProcessGate;
    }

    public GameId Game => GameId.IRacing;

    /// <summary>Aakhri TryRead ka nateeja (diagnostics).</summary>
    public ReaderStatus LastStatus { get; private set; } = ReaderStatus.NeverRead;

    /// <summary>SEC-H-05: process naam + executable path dono check karo (dekho Kunos reader).</summary>
    internal static bool DefaultProcessGate(string name)
    {
        try
        {
            foreach (var proc in Process.GetProcessesByName(name))
            {
                using (proc)
                {
                    try
                    {
                        string? exe = proc.MainModule?.FileName;
                        if (exe is not null &&
                            exe.EndsWith(name + ".exe", StringComparison.OrdinalIgnoreCase))
                            return true;
                    }
                    catch (Exception) { /* MainModule na parha jaye — is process ko gin'o mat */ }
                }
            }
        }
        catch (Exception) { /* process list na mile — game nahi samjho */ }
        return false;
    }

    public bool TryRead(out GameTelemetrySnapshot snapshot)
    {
        snapshot = default!;
        try
        {
            return TryReadInner(out snapshot);
        }
        catch (Exception)
        {
            LastStatus = ReaderStatus.Corrupt;
            snapshot = default!;
            return false;
        }
    }

    private bool TryReadInner(out GameTelemetrySnapshot snapshot)
    {
        snapshot = default!;
        if (!_isProcessRunning("iRacingSim64DX11"))
        {
            LastStatus = ReaderStatus.GameNotRunning;
            return false;
        }
        if (!_accessor.TryOpen(MapName, HeaderSize))
        {
            LastStatus = ReaderStatus.MapNotFound;
            return false;
        }

        byte[] header = new byte[HeaderSize];
        if (_accessor.ReadBytes(0, header, header.Length) < HeaderSize)
        {
            LastStatus = ReaderStatus.UnsupportedVersion;
            return false;
        }

        int ver = BitConverter.ToInt32(header, 0);
        int status = BitConverter.ToInt32(header, 4);
        int sessionInfoLen = BitConverter.ToInt32(header, 16);
        int sessionInfoOffset = BitConverter.ToInt32(header, 20);
        int numVars = BitConverter.ToInt32(header, 24);
        int varHeaderOffset = BitConverter.ToInt32(header, 28);
        int numBuf = BitConverter.ToInt32(header, 32);
        int bufLen = BitConverter.ToInt32(header, 36);

        // Header sanity + SEC-H-05 bounds: koi bhi field map se bahir
        // ishara kare to UnsupportedVersion — kabhi ghalat offset se nahi parho.
        int mapSize = _accessor.Size;
        bool headerSane =
            ver == 1 &&
            (status & 1) != 0 && // bit 0 = connected
            numVars >= 0 && numVars <= MaxVars &&
            numBuf >= 1 && numBuf <= MaxBuffers &&
            bufLen > 0 && bufLen <= mapSize &&
            varHeaderOffset >= HeaderSize &&
            (long)varHeaderOffset + (long)numVars * VarHeaderSize <= mapSize &&
            sessionInfoLen >= 0 && sessionInfoLen <= MaxSessionInfoBytes &&
            sessionInfoOffset >= 0 &&
            (long)sessionInfoOffset + sessionInfoLen <= mapSize;
        if (!headerSane)
        {
            LastStatus = ReaderStatus.UnsupportedVersion;
            return false;
        }

        // Var headers parho — sirf SessionState chahiye (int, count 1).
        int sessionStateOffset = -1;
        byte[] varHeader = new byte[VarHeaderSize];
        for (int i = 0; i < numVars; i++)
        {
            if (_accessor.ReadBytes(varHeaderOffset + (long)i * VarHeaderSize, varHeader, VarHeaderSize) < VarHeaderSize)
            {
                LastStatus = ReaderStatus.Corrupt;
                return false;
            }
            string name = ReadAsciiString(varHeader, 16, 32);
            if (name == "SessionState")
            {
                int type = BitConverter.ToInt32(varHeader, 0);
                int count = BitConverter.ToInt32(varHeader, 8);
                if (type != 2 || count < 1) // 2 = int
                {
                    LastStatus = ReaderStatus.UnsupportedVersion;
                    return false;
                }
                sessionStateOffset = BitConverter.ToInt32(varHeader, 4);
                break;
            }
        }

        // Latest buffer (second-highest tickCount — triple-buffer rule).
        if (!TryPickLatestBuffer(header, numBuf, bufLen, mapSize, out int bufOffset, out int tickCount))
        {
            LastStatus = ReaderStatus.Corrupt;
            return false;
        }

        // SEC-H-05 (TOCTOU): buffer copy se pehle aur baad mein tickCount
        // dobara parho — badla ho to game beech mein likh raha tha.
        byte[] buf = new byte[bufLen];
        int sessionStateRaw = 0;
        bool haveState = false;
        for (int attempt = 0; attempt < 2; attempt++)
        {
            int tickBefore = ReadTickCount(bufOffset, header, numBuf);
            if (_accessor.ReadBytes(bufOffset, buf, bufLen) < bufLen) break;
            int tickAfter = ReadTickCount(bufOffset, header, numBuf);
            if (tickBefore == tickCount && tickAfter == tickCount)
            {
                if (sessionStateOffset >= 0 && sessionStateOffset + 4 <= bufLen)
                {
                    sessionStateRaw = BitConverter.ToInt32(buf, sessionStateOffset);
                    haveState = true;
                }
                break;
            }
            // Tick badla — dobara latest buffer chuno aur dobara koshish karo.
            if (attempt == 0 &&
                TryPickLatestBuffer(header, numBuf, bufLen, mapSize, out int nextOffset, out int nextTick))
            {
                bufOffset = nextOffset;
                tickCount = nextTick;
            }
        }
        if (!haveState && sessionStateOffset >= 0)
        {
            // Do koshishon mein bhi stable copy nahi mili — aadha snapshot nahi lo.
            LastStatus = ReaderStatus.Corrupt;
            return false;
        }

        // DATA-AI FIX: SessionState var hi na mile (purana/naya SDK layout)
        // to state Unknown hai — raw 0 ko Menu samajhna khamoshi se ghalat
        // state deta tha (sessionStateOffset -1 par sessionStateRaw 0 rehta
        // tha → Menu). Unknown kabhi switch nahi karta (fail-closed).
        SessionState state = sessionStateOffset < 0 ? SessionState.Unknown : sessionStateRaw switch
        {
            4 => SessionState.OnTrack,  // Racing
            3 => SessionState.OnTrack,  // ParadeLaps — gaari track par hai
            5 => SessionState.OnTrack,  // Checkered — race khatam, abhi track par
            2 => SessionState.Loading,  // Warmup
            1 => SessionState.Loading,  // GetInCar
            0 => SessionState.Menu,     // Invalid
            6 => SessionState.Menu,     // CoolDown — session khatam
            _ => SessionState.Unknown,
        };

        // Session YAML — size pehle hi bound hai (MaxSessionInfoBytes).
        string carModel = "";
        string track = "";
        if (sessionInfoLen > 0)
        {
            byte[] yamlBytes = new byte[sessionInfoLen];
            if (_accessor.ReadBytes(sessionInfoOffset, yamlBytes, yamlBytes.Length) == sessionInfoLen)
            {
                string yaml = Encoding.UTF8.GetString(yamlBytes);
                ParseSessionYaml(yaml, out carModel, out track);
            }
        }

        LastStatus = ReaderStatus.Ok;
        snapshot = new GameTelemetrySnapshot(Game, carModel, track, state, DateTimeOffset.UtcNow);
        return true;
    }

    /// <summary>
    /// VarBuf descriptors se latest (second-highest tickCount) buffer chuno.
    /// Sirf tickCount &gt; 0 aur map ke andar wale buffers gin'e jate hain.
    /// </summary>
    private bool TryPickLatestBuffer(byte[] header, int numBuf, int bufLen, int mapSize, out int bufOffset, out int tickCount)
    {
        bufOffset = 0;
        tickCount = 0;
        var candidates = new List<(int Tick, int Offset)>();
        for (int i = 0; i < numBuf; i++)
        {
            int tick = BitConverter.ToInt32(header, 48 + i * VarBufferEntrySize);
            int offset = BitConverter.ToInt32(header, 48 + i * VarBufferEntrySize + 4);
            if (tick > 0 && offset >= 0 && (long)offset + bufLen <= mapSize)
                candidates.Add((tick, offset));
        }
        if (candidates.Count == 0) return false;
        candidates.Sort(static (a, b) => b.Tick.CompareTo(a.Tick));
        // Second-highest = latest mukammal buffer (pyirsdk rule); ek ho to wohi.
        var chosen = candidates.Count > 1 ? candidates[1] : candidates[0];
        bufOffset = chosen.Offset;
        tickCount = chosen.Tick;
        return true;
    }

    /// <summary>Ek buffer descriptor ka maujooda tickCount dobara parho (TOCTOU check).</summary>
    private int ReadTickCount(int bufOffset, byte[] header, int numBuf)
    {
        // Header ki copy mein tickCount purana ho sakta hai — map se taaza parho.
        byte[] entry = new byte[VarBufferEntrySize];
        for (int i = 0; i < numBuf; i++)
        {
            int offset = BitConverter.ToInt32(header, 48 + i * VarBufferEntrySize + 4);
            if (offset == bufOffset &&
                _accessor.ReadBytes(48 + (long)i * VarBufferEntrySize, entry, entry.Length) == entry.Length)
                return BitConverter.ToInt32(entry, 0);
        }
        return -1;
    }

    /// <summary>
    /// Session YAML se car + track nikalo (chhota line scanner — Core mein
    /// YAML library nahi hai, aur poora YAML parser is kaam ke liye zyada
    /// hai). Sections: <c>WeekendInfo: TrackName:</c>,
    /// <c>DriverInfo: Drivers:</c> list — <c>IsPlayer: 1</c> wala driver,
    /// us ki <c>CarScreenName</c> (fallback <c>CarPath</c>); player na mile
    /// to pehla driver. Khaali/kharab YAML → dono khaali (kabhi throw nahi).
    /// </summary>
    internal static void ParseSessionYaml(string yaml, out string carModel, out string trackName)
    {
        carModel = "";
        trackName = "";
        if (string.IsNullOrEmpty(yaml)) return;
        try
        {
            string section = "";
            string? playerCar = null;
            string? playerPath = null;
            string? firstCar = null;
            string? firstPath = null;
            bool inDrivers = false;
            bool driverIsPlayer = false;
            string? curCar = null;
            string? curPath = null;

            void FlushDriver()
            {
                if (curCar is null && curPath is null) return;
                firstCar ??= curCar;
                firstPath ??= curPath;
                if (driverIsPlayer)
                {
                    playerCar ??= curCar;
                    playerPath ??= curPath;
                }
                curCar = null;
                curPath = null;
                driverIsPlayer = false;
            }

            foreach (string rawLine in yaml.Split('\n'))
            {
                string line = rawLine.TrimEnd('\r');
                if (line.Length == 0 || line.TrimStart().StartsWith('#')) continue;
                int indent = 0;
                while (indent < line.Length && line[indent] == ' ') indent++;
                string trimmed = line.Trim();

                if (indent == 0 && trimmed.EndsWith(':'))
                {
                    FlushDriver();
                    section = trimmed[..^1];
                    inDrivers = false;
                    continue;
                }
                if (section == "WeekendInfo" && indent == 2 && trimmed.StartsWith("TrackName:"))
                {
                    trackName = Unquote(trimmed["TrackName:".Length..].Trim());
                    continue;
                }
                if (section == "DriverInfo" && trimmed == "Drivers:")
                {
                    inDrivers = true;
                    continue;
                }
                if (!inDrivers || section != "DriverInfo") continue;

                if (trimmed.StartsWith("- "))
                {
                    FlushDriver();
                    string rest = trimmed[2..].Trim();
                    if (rest.StartsWith("CarIdx:")) continue; // naya driver block shuru
                }
                if (trimmed.StartsWith("IsPlayer:"))
                {
                    driverIsPlayer = trimmed["IsPlayer:".Length..].Trim() == "1";
                    continue;
                }
                if (trimmed.StartsWith("CarScreenName:"))
                {
                    curCar = Unquote(trimmed["CarScreenName:".Length..].Trim());
                    continue;
                }
                if (trimmed.StartsWith("CarPath:"))
                {
                    curPath = Unquote(trimmed["CarPath:".Length..].Trim());
                }
            }
            FlushDriver();
            carModel = playerCar ?? playerPath ?? firstCar ?? firstPath ?? "";
        }
        catch (Exception)
        {
            carModel = "";
            trackName = "";
        }
    }

    private static string Unquote(string value)
    {
        value = value.Trim();
        if (value.Length >= 2 && value[0] == '"' && value[^1] == '"')
            return value[1..^1];
        if (value.Length >= 2 && value[0] == '\'' && value[^1] == '\'')
            return value[1..^1];
        return value;
    }

    internal static string ReadAsciiString(byte[] buffer, int offset, int maxChars)
    {
        int byteCount = Math.Min(maxChars, buffer.Length - offset);
        if (byteCount <= 0) return "";
        string text = Encoding.ASCII.GetString(buffer, offset, byteCount);
        int nul = text.IndexOf('\0');
        if (nul >= 0) text = text[..nul];
        return text.Trim();
    }
}
