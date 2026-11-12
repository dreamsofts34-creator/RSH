using System.Runtime.InteropServices;

namespace ApexLink.Output.VJoy;

/// <summary>
/// JOYSTICK_POSITION — BrunnerInnovation vJoy (2.2.2.0) ke asal
/// inc/public.h se byte-for-byte milaya gaya layout. Is fork ka driver
/// API version 3 istemal karta hai (V3 layout: wAccelerator/wBrake/
/// wClutch/wSteering beech mein, wAxisVZ/VBR tail par).
///
/// Achi khabar: hamare istemal ke fields (X/Y/Z axes, buttons, hats)
/// ke offsets V2 aur V3 dono mein EK hi hain, aur V3 ka size (124 B)
/// V2 (108 B) se bara hai — to ye struct dono DLLs ke sath mehfooz hai.
/// StructLayout Sequential + default packing C header jaisi hai
/// (pshpack1 sirf FFB structs ke liye tha, is ke liye nahi).
/// Layout ki tasdeeq tests mein hai (size + offsets) — guess nahi.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public struct JoystickPosition
{
    public byte BDevice;        // offset 0 (1-based device index)
    public int WThrottle;       // 4
    public int WRudder;         // 8
    public int WAileron;        // 12
    public int WAxisX;          // 16 — steering
    public int WAxisY;          // 20 — accelerator
    public int WAxisZ;          // 24 — brake
    public int WAxisXRot;       // 28
    public int WAxisYRot;       // 32
    public int WAxisZRot;       // 36
    public int WSlider;         // 40
    public int WDial;           // 44
    public int WWheel;          // 48
    public int WAccelerator;    // 52 (sirf V3)
    public int WBrake;          // 56 (sirf V3)
    public int WClutch;         // 60 (sirf V3)
    public int WSteering;       // 64 (sirf V3)
    public int WAxisVX;         // 68
    public int WAxisVY;         // 72
    public int LButtons;        // 76 — buttons 1..32 (bit 0 = button 1)
    public uint BHats;          // 80 — 0xFFFFFFFF = neutral
    public uint BHatsEx1;       // 84
    public uint BHatsEx2;       // 88
    public uint BHatsEx3;       // 92
    public int LButtonsEx1;     // 96 — buttons 33..64
    public int LButtonsEx2;     // 100 — buttons 65..96
    public int LButtonsEx3;     // 104 — buttons 97..128
    public int WAxisVZ;         // 108 (V3 mein tail par)
    public int WAxisVBRX;       // 112
    public int WAxisVBRY;       // 116
    public int WAxisVBRZ;       // 120 — kul size 124
}
