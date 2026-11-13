using System.Runtime.InteropServices;
using RSH.Core.Output;
using RSH.Output.VJoy;

namespace RSH.VJoy.Tests;

/// <summary>
/// Ye tests kisi driver ke baghair chalte hain: struct ka layout asal
/// header (BrunnerInnovation inc/public.h, API V3) se milna aur DLL na
/// hone par mehfooz nakami — yahi do cheezen yahan verify ho sakti hain.
/// Asal driver ka test sirf Windows + vJoy par hoga.
/// </summary>
public class VJoyLayoutTests
{
    [Fact]
    public void StructSize_AndOffsets_HeaderSeMilteHain()
    {
        Assert.Equal(124, Marshal.SizeOf<JoystickPosition>());
        Assert.Equal(0, Marshal.OffsetOf<JoystickPosition>("BDevice"));
        Assert.Equal(16, Marshal.OffsetOf<JoystickPosition>("WAxisX"));
        Assert.Equal(20, Marshal.OffsetOf<JoystickPosition>("WAxisY"));
        Assert.Equal(24, Marshal.OffsetOf<JoystickPosition>("WAxisZ"));
        Assert.Equal(52, Marshal.OffsetOf<JoystickPosition>("WAccelerator"));
        Assert.Equal(64, Marshal.OffsetOf<JoystickPosition>("WSteering"));
        Assert.Equal(76, Marshal.OffsetOf<JoystickPosition>("LButtons"));
        Assert.Equal(80, Marshal.OffsetOf<JoystickPosition>("BHats"));
        Assert.Equal(96, Marshal.OffsetOf<JoystickPosition>("LButtonsEx1"));
        Assert.Equal(108, Marshal.OffsetOf<JoystickPosition>("WAxisVZ"));
        Assert.Equal(120, Marshal.OffsetOf<JoystickPosition>("WAxisVBRZ"));
    }

    [Fact]
    public void NeutralPosition_SahiQeematein()
    {
        var p = VJoyOutput.NeutralPosition(1);
        Assert.Equal((byte)1, p.BDevice);
        Assert.Equal(16384, p.WAxisX);          // steering center
        Assert.Equal(1, p.WAxisY);              // pedal chora hua
        Assert.Equal(1, p.WAxisZ);
        Assert.Equal(0, p.LButtons);
        Assert.Equal(0xFFFF_FFFFu, p.BHats);    // HAT neutral
    }

    [Fact]
    public void TryConnect_DllNaHoToMehfoozNakami()
    {
        // Is mahol mein vJoyInterface.dll hai hi nahi — TryConnect ko
        // crash ke baghair wazeh wajah ke sath false dena chahiye.
        if (File.Exists("vJoyInterface.dll") || File.Exists(@"C:\Program Files\vJoy\x64\vJoyInterface.dll"))
            Assert.Skip("Is machine par vJoy DLL mojood hai — ye test sirf DLL-absent mahol ke liye hai.");

        using var output = new VJoyOutput();
        bool ok = output.TryConnect(out string error);
        Assert.False(ok);
        Assert.False(output.IsConnected);
        Assert.Contains("vJoyInterface.dll", error);
    }

    [Fact]
    public void Update_ConnectedNaHoToWazehException()
    {
        using var output = new VJoyOutput();
        Assert.Throws<InvalidOperationException>(() => output.Update(ProcessedState.Neutral));
    }
}
