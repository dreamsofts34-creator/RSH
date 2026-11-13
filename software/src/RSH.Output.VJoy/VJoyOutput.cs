using System.Runtime.InteropServices;
using RSH.Core.Output;
using RSH.Core.Pipeline;
using RSH.Core.Processing;

namespace RSH.Output.VJoy;

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
    // Phase G C8: Slider (HID Generic Desktop usage 0x36 — X 0x30, Y 0x31,
    // Z 0x32, Rx 0x33, Ry 0x34, Rz 0x35, Slider 0x36, Dial 0x37, Wheel
    // 0x38) — handbrake isi axis par jata hai (docs/HANDBRAKE-MAPPING.md).
    public const ushort UsageSlider = 0x36;

    private readonly uint _deviceId;
    private JoystickPosition _position;
    private bool _connected;
    private bool _capsKnown;   // P12: TryConnect ne caps parhe hon to true — warna Unknown

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
    /// <summary>Phase G C8: device mein Slider axis hai ya nahi (handbrake).</summary>
    public bool HasAxisSlider { get; private set; }

    private static string FormatVersion(ushort v) =>
        $"{(v >> 8) & 0xF}.{(v >> 4) & 0xF}.{v & 0xF}";

    /// <summary>
    /// P12 (D11): device ki salahiyatein configured modules ke khilaf
    /// validate karo — sirf enabled modules ki zarooratein lagti hain.
    /// Steering-only wheel valid hai (sirf X axis chahiye, buttons ki
    /// shart nahi); band module ke liye axis/button ki shart nahi.
    /// Clutch ka vJoy mapping hai (WClutch — BuildPosition, Phase F WS5);
    /// Encoders ka mapping hai (pulse buttons 65..72 — EncoderPulseMask,
    /// BuildPosition, Phase F WS5 — device caps se mask, koi shart nahi);
    /// Phase G C8: Handbrake → WSlider (analog). Slider ki HID usage
    /// (0x36) hoti hai, is liye driver se poocha ja sakta hai — Handbrake
    /// enabled ho aur device mein Slider na ho to error hai (khamoshi se
    /// drop nahi — D11). Shifter/Horn/LED ka is app path mein koi vJoy
    /// axis/button mapping nahi, is liye un ki koi shart nahi.
    /// Khaali list = valid.
    /// </summary>
    public static IReadOnlyList<string> ValidateCapabilities(PipelinePlan plan, VJoyCapabilities caps)
    {
        ArgumentNullException.ThrowIfNull(plan);
        var errors = new List<string>();
        if (caps.ButtonCount is < 0 or > VJoyCapabilities.MaxButtons)
            errors.Add($"ButtonCount {caps.ButtonCount} hadd se bahar hai (0..{VJoyCapabilities.MaxButtons}).");
        if (plan.IsEnabled(ModuleKind.Steering) && !caps.HasAxisX)
            errors.Add("Steering module on hai lekin vJoy device mein X axis nahi hai.");
        if (plan.IsEnabled(ModuleKind.Accelerator) && !caps.HasAxisY)
            errors.Add("Accelerator module on hai lekin vJoy device mein Y axis nahi hai.");
        if (plan.IsEnabled(ModuleKind.Brake) && !caps.HasAxisZ)
            errors.Add("Brake module on hai lekin vJoy device mein Z axis nahi hai.");
        if (plan.IsEnabled(ModuleKind.Handbrake) && !caps.HasAxisSlider)
            errors.Add("Handbrake module on hai lekin vJoy device mein Slider axis nahi hai.");
        if (plan.IsEnabled(ModuleKind.Buttons) && caps.ButtonCount < 1)
            errors.Add("Buttons module on hai lekin vJoy device mein koi button nahi hai.");
        return errors;
    }

    /// <summary>
    /// P12: maujooda (connected) device ki salahiyatein plan ke khilaf
    /// validate karo. Connect se pehle caps (false/0) hain — is liye
    /// ye sirf connected device par maani rakhta hai.
    /// </summary>
    public IReadOnlyList<string> ValidateCurrentCapabilities(PipelinePlan plan) =>
        ValidateCapabilities(plan, new VJoyCapabilities(HasAxisX, HasAxisY, HasAxisZ, ButtonCount, HasAxisSlider));
    /// <summary>Is device ki salahiyatein (TryConnect ke baad sahi hoti hain).</summary>
    public VJoyCapabilities Capabilities => new(HasAxisX, HasAxisY, HasAxisZ, ButtonCount, HasAxisSlider);

    public bool TryConnect(out string error) => TryConnect(out error, plan: null);

    /// <summary>
    /// P12: plan diya jaye to connect ke baad capabilities us ke khilaf
    /// validate hoti hain — fail ho to device wapas kar diya jata hai
    /// (relinquish) aur wajah ke saath false milta hai.
    /// </summary>
    public bool TryConnect(out string error, PipelinePlan? plan)
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
                    error = $"vJoy device {_deviceId} maujood nahi. vJoyConf mein Device {_deviceId} enable karo (X, Y, Z + Slider axes ke sath).";
                    return false;
                default:
                    error = $"vJoy device {_deviceId} ki halat maloom nahi ho saki (Unknown).";
                    return false;
            }

            // Salahiyatein (Output page ke liye)
            HasAxisX = NativeMethods.GetVJDAxisExist(_deviceId, UsageX);
            HasAxisY = NativeMethods.GetVJDAxisExist(_deviceId, UsageY);
            HasAxisZ = NativeMethods.GetVJDAxisExist(_deviceId, UsageZ);
            HasAxisSlider = NativeMethods.GetVJDAxisExist(_deviceId, UsageSlider);
            ButtonCount = NativeMethods.GetVJDButtonNumber(_deviceId);
            _capsKnown = true;

            // P12 (D11): plan diya ho to capabilities us ke khilaf
            // validate karo — fail ho to device wapas karo, connect nahi.
            if (plan is not null)
            {
                var errors = ValidateCapabilities(plan, Capabilities);
                if (errors.Count > 0)
                {
                    RelinquishIfConnected();
                    error = "vJoy device is plan ke liye kaafi nahi hai: " + string.Join(" ", errors);
                    return false;
                }
            }

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

        // P12: position har Update par neutral se dobara banti hai
        // (rebuild) — caps maloom hon to sirf wohi axes likhe jate hain
        // jo device mein hain (baqi neutral rehte hain), aur buttons
        // device ke ButtonCount se mask hote hain (count se zyada bits
        // kabhi nahi). Caps maloom na hon (kabhi connect nahi hua) to
        // Unknown = sab kuch likho (purana behaviour; T11 mapping tests
        // isi par hain — default false/0 ko "device mein kuch nahi"
        // samajhna ghalat tha, wo "maloom nahi" hai).
        _position = BuildPosition(state, _capsKnown ? Capabilities : VJoyCapabilities.Unknown, _deviceId);

        if (!NativeMethods.UpdateVJD(_deviceId, ref _position))
            throw new InvalidOperationException("UpdateVJD nakam — device haath se nikal gaya lagta hai.");
    }

    /// <summary>
    /// P12: ProcessedState → JoystickPosition, device ki salahiyaton ke
    /// khilaf. Pure function — baghair driver ke test ho sakta hai.
    /// Na-mojood axis neutral rehta hai; buttons ButtonCount se mask hote hain.
    /// T11/Ex: ButtonsMaskEx (33..96) → LButtonsEx1 (33..64, neeche 32 bits),
    /// LButtonsEx2 (65..96, oopar 32 bits); LButtonsEx3 (97..128) ka koi
    /// source nahi (wire max 64 buttons) — hamesha 0.
    /// Phase F WS5: encoder pulses (ProcessedState.EncoderPulseMask, bit
    /// (n-65) = vJoy button n, 65..72) LButtonsEx2 mein OR hote hain
    /// (caps.Ex2Mask se mask — 72 se kam buttons wale device par pulses
    /// khamoshi se mask ho jati hain, koi error nahi).
    /// Phase G C8: handbrake → WSlider (analog, caps-gated — device mein
    /// Slider na ho to neutral rehta hai; validation mein ye error hai,
    /// dekho ValidateCapabilities). Faisla: docs/HANDBRAKE-MAPPING.md.
    /// </summary>
    internal static JoystickPosition BuildPosition(in ProcessedState state, VJoyCapabilities caps, uint deviceId)
    {
        var p = NeutralPosition(deviceId);
        if (caps.HasAxisX)
        {
            p.WAxisX = state.SteeringAxis;
            p.WSteering = state.SteeringAxis;   // V3 khaas driving axis
        }
        if (caps.HasAxisY)
        {
            p.WAxisY = state.AccelAxis;
            p.WAccelerator = state.AccelAxis;
        }
        if (caps.HasAxisZ)
        {
            p.WAxisZ = state.BrakeAxis;
            p.WBrake = state.BrakeAxis;
        }
        // Phase F WS5: clutch → WClutch (vJoy V3 driving axis). Is ke liye
        // caps mein koi flag nahi (HasAxisX/Y/Z sirf X/Y/Z hain) — field
        // NeutralPosition mein bhi hamesha likha jata hai (min), aur
        // device mein axis na ho to driver is field ko ignore karta hai.
        p.WClutch = state.ClutchAxis;
        // Phase G C8: handbrake → WSlider (analog). vJoy V3 mein handbrake
        // ka koi dedicated axis nahi (sirf Steering/Accel/Brake/Clutch);
        // Slider generic 1-D axis hai — games isay handbrake ke tor par
        // bind kar sakte hain. Neutral = AxisMin (chhora hua), pedals jaisa.
        if (caps.HasAxisSlider)
            p.WSlider = state.HandbrakeAxis;
        p.LButtons = (int)(state.ButtonsMask & caps.ButtonMask);
        p.LButtonsEx1 = (int)(uint)(state.ButtonsMaskEx & caps.Ex1Mask);
        p.LButtonsEx2 = (int)(uint)(((state.ButtonsMaskEx >> 32) | state.EncoderPulseMask) & caps.Ex2Mask);
        p.LButtonsEx3 = 0;   // 97..128: koi wire source nahi (StatusPacketV2.BtnsEx sirf 32..63 deta hai)
        return p;
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
            // Phase G C8: Slider handbrake hai — neutral = min (chhora hua),
            // center nahi (center aadha handbrake hota). Slider pehle kisi
            // module mein istemal nahi hota tha, is liye koi aur asar nahi.
            WSlider = min, WDial = c, WWheel = c,
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
