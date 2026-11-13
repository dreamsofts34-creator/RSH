namespace RSH.Core.Telemetry;

/// <summary>
/// Assetto Corsa Competizione reader — <c>acc.exe</c> gate + acpmf_* maps.
/// <para>
/// Layout AC jaisa hi hai (wchar_t static: carModel @68; graphics status
/// @4; ACC_STATUS values AC_STATUS jaisi: 0=OFF, 1=REPLAY, 2=LIVE,
/// 3=PAUSE) — farq sirf <see cref="KunosSharedMemoryReader.ProcessName"/>
/// gate aur <see cref="GameId.Acc"/> hai. Map naam AC wale hi hain
/// (dekho <see cref="KunosSharedMemoryReader"/> — ACC ne AC ke naam
/// dobara istemal kiye), is liye process gate hi dono readers ko alag
/// karta hai.
/// </para>
/// </summary>
public sealed class AccSharedMemoryReader : KunosSharedMemoryReader
{
    public AccSharedMemoryReader(
        IMemoryMapAccessor? accessor = null,
        Func<string, bool>? isProcessRunning = null)
        : base(GameId.Acc, "acc", accessor, isProcessRunning)
    {
    }
}
