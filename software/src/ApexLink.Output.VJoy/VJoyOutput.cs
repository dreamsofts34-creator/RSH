using System.Runtime.InteropServices;
using ApexLink.Core.Output;
using ApexLink.Core.Processing;

namespace ApexLink.Output.VJoy;

/// <summary>
/// vJoy backend — IJoystickOutput ki asal implementation.
/// Driver: BrunnerInnovation v2.2.2.0 (EV-signed, Win10/Win11) aur usi
/// release ki vJoyInterface.dll app ke sath ship hogi. DLL na mile ya
/// driver band ho to TryConnect wazeh wajah ke sath false deta hai —
/// app kabhi crash nahi hoti, Output page wajah dikhati hai.
///
/// Har native call exception se mehfooz hai: DLL ki ghair-maujudgi
/// DllNotFoundException deti hai, jo yahan pakri jati hai.
/// </summary>
public sealed class VJoyOutput : IJoystickOutput
{
    // HID usages (public.h — HID_USAGE asal mein USHORT hai)
    public const ushort UsageX = 0x30;
    public const ushort UsageY = 0x31;
    public const ushort UsageZ = 0x32;

    private readonly uint _deviceId;
    private JoystickPosition _position;
    private bool _connected;

    static VJoyOutput()
    {
        // DLL ki talash: pehle app ke sath wali copy, phir vJoy ki apni
        // install jagah (C:\Program Files\vJoy\x64) — taake sirf driver
        // install karne par bhi sab chale, bina DLL copy kiye.
        NativeLibrary.SetDllImportResolver(typeof(VJoyOutput).Assembly, (name, assembly, path) =>
        {
            if (!string.Equals(name, "vJoyInterface.dll", StringComparison.OrdinalIgnoreCase))
                return IntPtr.Zero;
            // AUDIT FIX: pehle APP ke sath wali copy (comment bhi yehi kehta
            // tha, code ulta tha) — Program Files wali purani/mukhtalif
            // release ki DLL khamoshi se ship-shuda DLL par jeet jati thi.
            string bundled = Path.Combine(AppContext.BaseDirectory, "vJoyInterface.dll");
            if (File.Exists(bundled) && NativeLibrary.TryLoad(bundled, out var bundledHandle))
                return bundledHandle;
            string installed = @"C:\Program Files\vJoy\x64\vJoyInterface.dll";
            if (File.Exists(installed) && NativeLibrary.TryLoad(installed, out var handle))
                return handle;
            return IntPtr.Zero;   // default talash par chor do
        });
    }

    public VJoyOutput(uint deviceId = 1)
    {
        _deviceId = deviceId;
        _position = NeutralPosition(deviceId);
    }

    public string Name => "vJoy";
    public bool IsConnected => _connected;
    public uint DeviceId => _deviceId;

    /// <summary>Driver/DLL versions (DriverMatch se) — Output page ke liye.</summary>
    public string? DllVersionText { get; private set; }
    public string? DriverVersionText { get; private set; }
    public int ButtonCount { get; private set; }
    public bool HasAxisX { get; private set; }
    public bool HasAxisY { get; private set; }
    public bool HasAxisZ { get; private set; }

    private static string FormatVersion(ushort v) =>
        $"{(v >> 8) & 0xF}.{(v >> 4) & 0xF}.{v & 0xF}";

    public bool TryConnect(out string error)
    {
        error = "";
        if (_connected) return true;

        try
        {
            if (!NativeMethods.vJoyEnabled())
            {
                error = "vJoy driver enabled nahi hai. Pehle vJoy (BrunnerInnovation v2.2.2.0) install karo.";
                return false;
            }

            ushort dll = 0, drv = 0;
            bool match = NativeMethods.DriverMatch(ref dll, ref drv);
            DllVersionText = FormatVersion(dll);
            DriverVersionText = FormatVersion(drv);
            if (!match)
            {
                error = $"vJoy DLL (v{DllVersionText}) aur driver (v{DriverVersionText}) ke versions nahi milte. Ek hi release se dono install karo.";
                return false;
            }

            var status = NativeMethods.GetDeviceStatus(_deviceId);
            switch (status)
            {
                case VjdStatus.Own:
                    _connected = true;   // pehle se hamare paas hai
                    break;
                case VjdStatus.Free:
                    if (!NativeMethods.AcquireVJD(_deviceId))
                    {
                        error = $"vJoy device {_deviceId} acquire nahi ho saka.";
                        return false;
                    }
                    _connected = true;
                    break;
                case VjdStatus.Busy:
                    error = $"vJoy device {_deviceId} kisi aur app ke paas hai (BUSY). Wo app band karke dobara koshish karo.";
                    return false;
                case VjdStatus.Missing:
                    error = $"vJoy device {_deviceId} maujood nahi. vJoyConf mein Device {_deviceId} enable karo (X, Y, Z axes ke sath).";
                    return false;
                default:
                    error = $"vJoy device {_deviceId} ki halat maloom nahi ho saki (Unknown).";
                    return false;
            }

            // Salahiyatein (Output page ke liye)
            HasAxisX = NativeMethods.GetVJDAxisExist(_deviceId, UsageX);
            HasAxisY = NativeMethods.GetVJDAxisExist(_deviceId, UsageY);
            HasAxisZ = NativeMethods.GetVJDAxisExist(_deviceId, UsageZ);
            ButtonCount = NativeMethods.GetVJDButtonNumber(_deviceId);

            NativeMethods.ResetVJD(_deviceId);
            SendNeutral();
            return true;
        }
        catch (DllNotFoundException)
        {
            // AUDIT FIX: device acquire ho chuka ho aur baad wali native
            // call par exception aye to _connected jhoota true reh jata
            // tha — device wapas kar ke halat saaf karo.
            RelinquishIfConnected();
            error = "vJoyInterface.dll nahi mili. vJoy (BrunnerInnovation v2.2.2.0) install karo — DLL app ke sath bhi ship hoti hai.";
            return false;
        }
        catch (Exception ex) when (ex is BadImageFormatException or EntryPointNotFoundException)
        {
            RelinquishIfConnected();
            error = $"vJoyInterface.dll theek nahi lagi ({ex.GetType().Name}). Sahi (x64) DLL ek hi release se lo.";
            return false;
        }
    }

    private void RelinquishIfConnected()
    {
        if (!_connected) return;
        _connected = false;
        try { NativeMethods.RelinquishVJD(_deviceId); } catch { /* safai ki koshish hi maqsad hai */ }
    }

    public void Disconnect()
    {
        if (!_connected) return;
        _connected = false;
        // AUDIT FIX: neutral bhejna aur device chorna alag alag mehfooz
        // hison mein — neutral ki koi bhi kharabi relinquish ko NAHI rok
        // sakti (pehle ek hi try mein honay se device acquire hi reh
        // jata tha aur exception Dispose se bahir nikal jata tha).
        try { SendNeutralCore(); } catch { /* neutral best-effort hai */ }
        try { NativeMethods.RelinquishVJD(_deviceId); }
        catch (DllNotFoundException) { /* DLL hi nahi — kuch karna nahi */ }
    }

    public void Update(in ProcessedState state)
    {
        if (!_connected) throw new InvalidOperationException("vJoy connected nahi hai.");

        var p = NeutralPosition(_deviceId);
        p.WAxisX = state.SteeringAxis;
        p.WAxisY = state.AccelAxis;
        p.WAxisZ = state.BrakeAxis;
        // V3 ke khaas driving axes par bhi wahi values — jo game inhe
        // parhta hai, use bhi durust data mile.
        p.WSteering = state.SteeringAxis;
        p.WAccelerator = state.AccelAxis;
        p.WBrake = state.BrakeAxis;
        p.LButtons = (int)(state.ButtonsMask & 0xFFFF_FFFF);
        _position = p;

        if (!NativeMethods.UpdateVJD(_deviceId, ref _position))
            throw new InvalidOperationException("UpdateVJD nakam — device haath se nikal gaya lagta hai.");
    }

    public void SendNeutral()
    {
        if (!_connected) return;
        SendNeutralCore();
    }

    private void SendNeutralCore()
    {
        _position = NeutralPosition(_deviceId);
        NativeMethods.UpdateVJD(_deviceId, ref _position);
    }

    internal static JoystickPosition NeutralPosition(uint deviceId)
    {
        int c = InputMapper.AxisCenter;
        int min = InputMapper.AxisMin;
        return new JoystickPosition
        {
            BDevice = (byte)deviceId,
            WThrottle = c, WRudder = c, WAileron = c,
            WAxisX = c, WAxisY = min, WAxisZ = min,
            WAxisXRot = c, WAxisYRot = c, WAxisZRot = c,
            WSlider = c, WDial = c, WWheel = c,
            WAccelerator = min, WBrake = min, WClutch = min, WSteering = c,
            WAxisVX = c, WAxisVY = c,
            LButtons = 0,
            BHats = 0xFFFF_FFFF, BHatsEx1 = 0xFFFF_FFFF, BHatsEx2 = 0xFFFF_FFFF, BHatsEx3 = 0xFFFF_FFFF,
            LButtonsEx1 = 0, LButtonsEx2 = 0, LButtonsEx3 = 0,
            WAxisVZ = c, WAxisVBRX = c, WAxisVBRY = c, WAxisVBRZ = c,
        };
    }

    public void Dispose() => Disconnect();
}
