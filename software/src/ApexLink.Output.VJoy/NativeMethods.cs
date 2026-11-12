using System.Runtime.InteropServices;

namespace ApexLink.Output.VJoy;

/// <summary>vJoy device ki halat (vJoyInterface.h ka VjdStat — qeematein header se).</summary>
public enum VjdStatus : int
{
    Own = 0,    // hamare paas hai
    Free = 1,   // khaali hai, acquire ho sakta hai
    Busy = 2,   // kisi aur feeder ke paas hai
    Missing = 3, // device maujood hi nahi (vJoyConf mein enable nahi)
    Unknown = 4,
}

/// <summary>
/// vJoyInterface.dll ke P/Invoke — tamam exports __cdecl hain (SDK header).
/// Khaas ehtiyaat: RelinquishVJD kuch return NAHI karti (void) — bohat se
/// wrappers yahan ghalti karte hain. UpdateVJD 200 Hz ka sahi rasta hai
/// (struct ek sath), SetAxis/SetBtn alag alag calls sust hain.
/// </summary>
internal static partial class NativeMethods
{
    private const string Dll = "vJoyInterface.dll";

    [LibraryImport(Dll)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool vJoyEnabled();

    [LibraryImport(Dll)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    public static partial short GetvJoyVersion();

    [LibraryImport(Dll)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool DriverMatch(ref ushort dllVersion, ref ushort driverVersion);

    [LibraryImport(Dll, EntryPoint = "GetVJDStatus")]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    public static partial VjdStatus GetDeviceStatus(uint deviceId);

    [LibraryImport(Dll)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool AcquireVJD(uint deviceId);

    [LibraryImport(Dll)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    public static partial void RelinquishVJD(uint deviceId);   // VOID — header ke mutabiq

    [LibraryImport(Dll)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool ResetVJD(uint deviceId);

    [LibraryImport(Dll)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool UpdateVJD(uint deviceId, ref JoystickPosition data);

    [LibraryImport(Dll)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool GetVJDAxisExist(uint deviceId, ushort axisUsage);

    [LibraryImport(Dll)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    public static partial int GetVJDButtonNumber(uint deviceId);
}
