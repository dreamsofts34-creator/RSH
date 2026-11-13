using System.Diagnostics;
using System.Text;

namespace RSH.Core.Telemetry;

/// <summary>
/// Kunos shared-memory layout (AC + ACC — TELEMETRY-READER.md §4).
/// <para>
/// <b>Structs <c>wchar_t</c> hain (UTF-16LE), <c>char</c> nahi</b> — Kunos
/// docs ke mutabiq `smVersion[15]` / `carModel[33]` wide strings hain.
/// Is liye offsets byte-wise ye hain (har wchar = 2 bytes):
/// smVersion @0 (30 B), acVersion @30 (30 B), numberOfSessions @60,
/// numCars @64, carModel @68 (66 B), track @134 (66 B).
/// ASCII samajh kar offset 38 par parhna ghalat car deta — yehi is
/// reader ka sab se bara layout risk tha, is liye test
/// <c>AcReaderParsesCapturedStatic</c> asal UTF-16 bytes se banta hai.
/// </para>
/// <para>
/// <b>ACC ke map naam AC wale hi hain</b> (<c>acpmf_physics</c> /
/// <c>acpmf_graphics</c> / <c>acpmf_static</c>) — TELEMETRY-READER.md §4
/// ka "alag map naam" ghalat hai (community docs / pyaccsharedmemory /
/// simetry: ACC ne AC ke naam dobara istemal kiye). Is liye AC aur ACC
/// reader map naam se nahi, <b>process</b> se alag hote hain
/// (<c>acs.exe</c> vs <c>acc.exe</c> — GameLauncher catalogue wale naam);
/// map naam ek jaisa hone ke bawajood doosre game ka map kabhi nahi parha jata.
/// </para>
/// <para>
/// Physics map v1 mein khola hi nahi jata (§2 rule 2 — sirf identity,
/// 2 Hz; physics LED/dash ka kaam hai, baad mein). Sirf static
/// (carModel/track) + graphics (AC_STATUS) parhe jate hain.
/// </para>
/// </summary>
public abstract class KunosSharedMemoryReader : ITelemetryReader
{
    // Map naam — AC aur ACC dono ke liye ek jaise (upar dekho).
    public const string PhysicsMapName = @"Local\acpmf_physics";
    public const string GraphicsMapName = @"Local\acpmf_graphics";
    public const string StaticMapName = @"Local\acpmf_static";

    // SPageFileStatic (wchar_t layout): carModel @68, 33 wchar (66 B).
    internal const int StaticCarModelOffset = 68;
    internal const int StaticCarModelChars = 33;
    internal const int StaticTrackOffset = 134;
    internal const int StaticTrackChars = 33;
    // carModel ke aakhir tak kam-az-kam size (134 B) — is se chhota map
    // UnsupportedVersion hai. Track optional hai (200 B ho to parho).
    internal const int MinStaticSize = StaticCarModelOffset + StaticCarModelChars * 2;
    internal const int FullStaticSize = StaticTrackOffset + StaticTrackChars * 2;

    // SPageFileGraphic: packetId @0 (int), status @4 (int).
    internal const int GraphicsStatusOffset = 4;
    internal const int MinGraphicsSize = 8;

    private readonly IMemoryMapAccessor _accessor;
    private readonly Func<string, bool> _isProcessRunning;

    protected KunosSharedMemoryReader(
        GameId game,
        string processName,
        IMemoryMapAccessor? accessor = null,
        Func<string, bool>? isProcessRunning = null)
    {
        Game = game;
        ProcessName = processName;
        _accessor = accessor ?? new WindowsMemoryMapAccessor();
        // SEC-H-05 (map spoofing, defense-in-depth): sirf process NAAM
        // kaafi nahi — koi bhi local process apna naam `acs.exe` rakh kar
        // `acpmf_static` jaisi map pehle bana sakta hai. Is liye default
        // gate naam ke saath executable path bhi check karta hai
        // (MainModule.FileName `<name>.exe` par khatam ho). Ye poora hal
        // nahi hai — map ke asal owner (creator PID / ACL) ka user-mode
        // se koi seedha saboot nahi milta, aur ek local attacker apni exe
        // ka naam + path bhi naqal kar sakta hai — lekin asar tuning-only
        // switch tak mehdood hai (koi input/output, calibration ya
        // credential is rasta se nahi badalta), is liye Medium par ye
        // mitigation + smVersion sanity check kaafi hai. Tests is gate ko
        // inject karte hain (FakeMemoryMapAccessor ke saath).
        _isProcessRunning = isProcessRunning ?? (name =>
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
        });
    }

    public GameId Game { get; }

    /// <summary>Is reader ka game process (GameLauncher catalogue wala naam, ".exe" ke baghair).</summary>
    public string ProcessName { get; }

    /// <summary>Aakhri TryRead ka nateeja (diagnostics — bool se zyada wajah).</summary>
    public ReaderStatus LastStatus { get; private set; } = ReaderStatus.NeverRead;

    /// <summary>Static block ki smVersion (jaisi parhi gayi — khaali ho to map zeroed hai).</summary>
    public string LastSmVersion { get; private set; } = "";

    public bool TryRead(out GameTelemetrySnapshot snapshot)
    {
        snapshot = default!;
        try
        {
            return TryReadInner(out snapshot);
        }
        catch (Exception)
        {
            // Contract: TryRead kabhi throw nahi karta.
            LastStatus = ReaderStatus.Corrupt;
            snapshot = default!;
            return false;
        }
    }

    private bool TryReadInner(out GameTelemetrySnapshot snapshot)
    {
        snapshot = default!;
        // Rule 3 (§2): pehle game process, phir map — AC chal raha ho to
        // ACC reader us ka (ek jaise naam wala) map kabhi nahi kholta.
        bool running;
        try { running = _isProcessRunning(ProcessName); }
        catch (Exception) { running = false; }
        if (!running)
        {
            LastStatus = ReaderStatus.GameNotRunning;
            return false;
        }

        if (!_accessor.TryOpen(StaticMapName, MinStaticSize))
        {
            // Map na mile (menu / session load nahi hua) ya size chhota ho —
            // dono mein false; farq sirf diagnostics ke liye: size chhota
            // ho to fake/zeroed map UnsupportedVersion hai, warna MapNotFound.
            // Fake accessor mein farq nazar nahi aata (TryOpen ek hi false
            // deta hai), is liye yahan MapNotFound hi rakho — asal
            // UnsupportedVersion ka rasta neeche smVersion check hai.
            LastStatus = ReaderStatus.MapNotFound;
            return false;
        }

        byte[] staticBuf = new byte[FullStaticSize];
        // SEC-H-05 (TOCTOU): game static block ko likh raha ho sakta hai
        // (carModel tabdeel ho raha ho) — ek copy aadhi purani / aadhi
        // nayi ho sakti hai. Do dafa parho aur milao; na mile to ek dafa
        // dobara — teesri bhi na mile to snapshot reject (Corrupt).
        if (!TryReadStableBytes(0, staticBuf, staticBuf.Length, out int read) || read < MinStaticSize)
        {
            LastStatus = ReaderStatus.Corrupt;
            return false;
        }

        // Size/version check (§2 rule 4): smVersion khaali/zeroed ho to ye
        // asal AC/ACC static block nahi — ghalat offset se kabhi nahi parho.
        string smVersion = ReadWideString(staticBuf, 0, 15);
        LastSmVersion = smVersion;
        if (smVersion.Length == 0)
        {
            LastStatus = ReaderStatus.UnsupportedVersion;
            return false;
        }

        string carModel = ReadWideString(staticBuf, StaticCarModelOffset, StaticCarModelChars);
        string track = read >= FullStaticSize
            ? ReadWideString(staticBuf, StaticTrackOffset, StaticTrackChars)
            : "";

        if (!_accessor.TryOpen(GraphicsMapName, MinGraphicsSize))
        {
            LastStatus = ReaderStatus.MapNotFound;
            return false;
        }
        byte[] gfxBuf = new byte[MinGraphicsSize];
        // SEC-H-05 (TOCTOU): status bhi stable copy se — ek int ki torn
        // read x86 par aligned hone se aam tor par nahi hoti, lekin
        // static+graphics ka jora ek hi lamhe ka hona chahiye; do dafa
        // parh kar milana sasta hai aur race ko band karta hai.
        if (!TryReadStableBytes(0, gfxBuf, gfxBuf.Length, out int gfxRead) || gfxRead < MinGraphicsSize)
        {
            LastStatus = ReaderStatus.Corrupt;
            return false;
        }
        int statusRaw = BitConverter.ToInt32(gfxBuf, GraphicsStatusOffset);
        SessionState state = statusRaw switch
        {
            0 => SessionState.Menu,     // AC_OFF / ACC_OFF
            1 => SessionState.Replay,   // AC_REPLAY / ACC_REPLAY
            2 => SessionState.OnTrack,  // AC_LIVE / ACC_LIVE
            3 => SessionState.Paused,   // AC_PAUSE / ACC_PAUSE
            _ => SessionState.Unknown,
        };

        LastStatus = ReaderStatus.Ok;
        snapshot = new GameTelemetrySnapshot(Game, carModel, track, state, DateTimeOffset.UtcNow);
        return true;
    }

    /// <summary>
    /// SEC-H-05 (TOCTOU): <paramref name="offset"/> se
    /// <paramref name="count"/> bytes do dafa parho — dono copy ek jaisi
    /// hon to snapshot atomic hai. Na mile to ek dafa dobara (teesri
    /// copy doosri se mile to woh stable hai); phir bhi na mile to false
    /// — game is waqt block ko likh raha hai, aadha snapshot kabhi nahi
    /// lo. <paramref name="buffer"/> mein stable copy rehti hai.
    /// </summary>
    private bool TryReadStableBytes(long offset, byte[] buffer, int count, out int bytesRead)
    {
        bytesRead = 0;
        byte[] second = new byte[count];
        int first = _accessor.ReadBytes(offset, buffer, count);
        if (first <= 0) return false;
        int secondRead = _accessor.ReadBytes(offset, second, count);
        if (secondRead == first && buffer.AsSpan(0, first).SequenceEqual(second.AsSpan(0, secondRead)))
        {
            bytesRead = first;
            return true;
        }
        int third = _accessor.ReadBytes(offset, buffer, count);
        if (third == secondRead && buffer.AsSpan(0, third).SequenceEqual(second.AsSpan(0, secondRead)))
        {
            bytesRead = third;
            return true;
        }
        return false;
    }

    /// <summary>UTF-16LE null-terminated wide string — pehle `\0` par kaato, trim karo.</summary>
    internal static string ReadWideString(byte[] buffer, int offset, int charCount)
    {
        int byteCount = Math.Min(charCount * 2, buffer.Length - offset);
        if (byteCount <= 0) return "";
        string text = Encoding.Unicode.GetString(buffer, offset, byteCount);
        int nul = text.IndexOf('\0');
        if (nul >= 0) text = text[..nul];
        return text.Trim();
    }
}

/// <summary>Assetto Corsa reader — <c>acs.exe</c> gate + acpmf_* maps.</summary>
public sealed class AcSharedMemoryReader : KunosSharedMemoryReader
{
    public AcSharedMemoryReader(
        IMemoryMapAccessor? accessor = null,
        Func<string, bool>? isProcessRunning = null)
        : base(GameId.AssettoCorsa, "acs", accessor, isProcessRunning)
    {
    }
}
