using System.Reflection;
using System.Runtime.InteropServices;
using RSH.Core.Output;
using RSH.Output.VJoy;

namespace RSH.VJoy.Tests;

/// <summary>
/// Ye tests kisi driver ke baghair chalte hain: struct ka layout asal
/// header (BrunnerInnovation inc/public.h, API V3) se milna aur DLL na
/// hone par mehfooz nakami — yahi do cheezen yahan verify ho sakti hain.
/// Asal driver ka test sirf Windows + vJoy par hoga.
///
/// T11 (SW-7): ProcessedState → JoystickPosition mapping bhi yahan hai.
/// Mapping ki asal logic VJoyOutput.Update() mein hai, jo connected
/// driver mangta hai — is liye test reflection se _connected = true
/// karke Update() chalata hai aur staged _position parhta hai. Driver
/// na ho to UpdateVJD ki native call DllNotFoundException deti hai,
/// lekin _position us se PEHLE assign ho chuka hota hai, is liye
/// mapping phir bhi verify hoti hai. Driver wali machine par UpdateVJD
/// kamyab hota hai (ya device-owned na hone par InvalidOperationException)
/// — dono surat mein staged position wahi rehti hai.
/// </summary>
public class VJoyLayoutTests
{
    /// <summary>
    /// Update() ko driver ke baghair chalao aur jo JoystickPosition
    /// us ne stage ki, wo wapas do. _connected ko finally mein wapas
    /// false kar do taake Dispose() koi native call na kare.
    /// </summary>
    private static JoystickPosition StagedPositionAfterUpdate(ProcessedState state)
    {
        using var output = new VJoyOutput();
        var connectedField = typeof(VJoyOutput).GetField("_connected",
            BindingFlags.NonPublic | BindingFlags.Instance)!;
        var positionField = typeof(VJoyOutput).GetField("_position",
            BindingFlags.NonPublic | BindingFlags.Instance)!;
        connectedField.SetValue(output, true);
        try
        {
            output.Update(state);
        }
        catch (Exception ex) when (ex is DllNotFoundException
            or InvalidOperationException       // UpdateVJD nakam (device owned nahi)
            or BadImageFormatException
            or EntryPointNotFoundException)    // DLL maujood lekin ghalat/na-mukammal
        {
            // Driver absent/ghalat: native call throw karti hai, lekin
            // _position pehle hi stage ho chuka hota hai — wahi check karna hai.
        }
        finally
        {
            connectedField.SetValue(output, false);
        }
        return (JoystickPosition)positionField.GetValue(output)!;
    }
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
        Assert.Equal(1, p.WSlider);             // handbrake chhora hua (Slider neutral = min, Phase G C8)
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

    // ------------------------------------------------------------------
    // T11 — ProcessedState → JoystickPosition mapping
    // ------------------------------------------------------------------

    [Fact]
    public void Mapping_SteeringGoesToAxisX_AndV3Steering()
    {
        var p = StagedPositionAfterUpdate(new ProcessedState(
            SteeringAxis: 1000, AccelAxis: 1, BrakeAxis: 1,
            ButtonsMask: 0, SteeringNorm: -0.94, AccelNorm: 0, BrakeNorm: 0));
        Assert.Equal(1000, p.WAxisX);       // steering → X
        Assert.Equal(1000, p.WSteering);    // V3 driving axis par bhi wahi value
        Assert.Equal(1, p.WAxisY);          // pedals chhoo nahi gaye
        Assert.Equal(1, p.WAxisZ);
    }

    [Fact]
    public void Mapping_PedalsGoToAxisY_Z_AndV3AccelBrake()
    {
        var p = StagedPositionAfterUpdate(new ProcessedState(
            SteeringAxis: 16384, AccelAxis: 20000, BrakeAxis: 30000,
            ButtonsMask: 0, SteeringNorm: 0, AccelNorm: 0.61, BrakeNorm: 0.92));
        Assert.Equal(20000, p.WAxisY);      // accel → Y
        Assert.Equal(20000, p.WAccelerator);// V3 driving axis
        Assert.Equal(30000, p.WAxisZ);      // brake → Z
        Assert.Equal(30000, p.WBrake);      // V3 driving axis
        Assert.Equal(16384, p.WAxisX);      // steering center barkarar
        Assert.Equal(16384, p.WSteering);
    }

    [Fact]
    public void Mapping_FullDeflection_EdgesExact()
    {
        var p = StagedPositionAfterUpdate(new ProcessedState(
            SteeringAxis: 32768, AccelAxis: 32768, BrakeAxis: 32768,
            ButtonsMask: 0, SteeringNorm: 1, AccelNorm: 1, BrakeNorm: 1));
        Assert.Equal(32768, p.WAxisX);
        Assert.Equal(32768, p.WAxisY);
        Assert.Equal(32768, p.WAxisZ);
        Assert.Equal(32768, p.WSteering);
        Assert.Equal(32768, p.WAccelerator);
        Assert.Equal(32768, p.WBrake);

        var q = StagedPositionAfterUpdate(new ProcessedState(
            SteeringAxis: 1, AccelAxis: 1, BrakeAxis: 1,
            ButtonsMask: 0, SteeringNorm: -1, AccelNorm: 0, BrakeNorm: 0));
        Assert.Equal(1, q.WAxisX);
        Assert.Equal(1, q.WAxisY);
        Assert.Equal(1, q.WAxisZ);
    }

    [Fact]
    public void Mapping_NeutralState_MatchesNeutralPosition()
    {
        var p = StagedPositionAfterUpdate(ProcessedState.Neutral);
        var n = VJoyOutput.NeutralPosition(1);
        Assert.Equal(n.WAxisX, p.WAxisX);
        Assert.Equal(n.WAxisY, p.WAxisY);
        Assert.Equal(n.WAxisZ, p.WAxisZ);
        Assert.Equal(n.WSlider, p.WSlider);
        Assert.Equal(n.WSteering, p.WSteering);
        Assert.Equal(n.WAccelerator, p.WAccelerator);
        Assert.Equal(n.WBrake, p.WBrake);
        Assert.Equal(0, p.LButtons);
        Assert.Equal(0xFFFF_FFFFu, p.BHats);
    }

    [Theory]
    [InlineData(1, 0x0000_0001)]    // button 1  → bit 0
    [InlineData(2, 0x0000_0002)]    // button 2  → bit 1
    [InlineData(8, 0x0000_0080)]    // button 8  → bit 7
    [InlineData(32, unchecked((int)0x8000_0000))] // button 32 → bit 31 (aakhri bit)
    public void Mapping_Buttons1To32_GoToLButtons(int button, int expectedLButtons)
    {
        uint mask = 1u << (button - 1);
        var p = StagedPositionAfterUpdate(new ProcessedState(
            SteeringAxis: 16384, AccelAxis: 1, BrakeAxis: 1,
            ButtonsMask: mask, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0));
        Assert.Equal(expectedLButtons, p.LButtons);
    }

    [Fact]
    public void Mapping_AllButtons1To32_SetTogether()
    {
        var p = StagedPositionAfterUpdate(new ProcessedState(
            SteeringAxis: 16384, AccelAxis: 1, BrakeAxis: 1,
            ButtonsMask: 0xFFFF_FFFF, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0));
        Assert.Equal(-1, p.LButtons); // tamam 32 bits set
    }

    [Fact]
    public void Buttons33Plus_ExFields_SetFromButtonsMaskEx()
    {
        // T11/Ex fix (Backend, SW-7/P12): pehle ye test gap document karta
        // tha — ab Ex fields set hote hain:
        //  - StatusPacketV2.BtnsEx (0-based buttons 32..63) → pipeline
        //    (Process buttonsEx param / WheelEngine.LatestButtonsEx) →
        //    ProcessedState.ButtonsMaskEx (ulong, bit (n-33) = vJoy button n)
        //    → Update()/BuildPosition mein Ex1 = bits 33..64 (neeche 32),
        //    Ex2 = bits 65..96 (oopar 32).
        //  - Ex3 (97..128) ka abhi bhi koi source nahi — wire max 64
        //    buttons hai (stream 0..31 + status BtnsEx 32..63) — is liye
        //    Ex3 hamesha 0 rehta hai (baaqi gap, protocol extension mangta hai).
        var plain = StagedPositionAfterUpdate(new ProcessedState(
            SteeringAxis: 16384, AccelAxis: 1, BrakeAxis: 1,
            ButtonsMask: 0xFFFF_FFFF, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0));
        Assert.Equal(-1, plain.LButtons);   // buttons 1..32 waisi ki waisi
        Assert.Equal(0, plain.LButtonsEx1); // Ex bits na hon to Ex fields 0
        Assert.Equal(0, plain.LButtonsEx2);
        Assert.Equal(0, plain.LButtonsEx3);

        var ex = StagedPositionAfterUpdate(new ProcessedState(
            SteeringAxis: 16384, AccelAxis: 1, BrakeAxis: 1,
            ButtonsMask: 0, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0,
            ButtonsMaskEx: 0xFFFF_FFFF_FFFF_FFFF));
        Assert.Equal(-1, ex.LButtonsEx1);   // buttons 33..64 — tamam bits set
        Assert.Equal(-1, ex.LButtonsEx2);   // buttons 65..96 — tamam bits set
        Assert.Equal(0, ex.LButtonsEx3);    // buttons 97..128 — koi source nahi
    }

    [Fact]
    public void Mapping_ClutchGoesToWClutch()
    {
        // Phase F WS5: clutch channel ProcessedState.ClutchAxis → WClutch.
        var p = StagedPositionAfterUpdate(new ProcessedState(
            SteeringAxis: 16384, AccelAxis: 1, BrakeAxis: 1,
            ButtonsMask: 0, SteeringNorm: 0, AccelNorm: 0, BrakeNorm: 0,
            ClutchNorm: 1.0, ClutchAxis: 32768));
        Assert.Equal(32768, p.WClutch);
        Assert.Equal(1, p.WAxisY); // baaqi axes chhoo nahi gaye

        var neutral = StagedPositionAfterUpdate(ProcessedState.Neutral);
        Assert.Equal(1, neutral.WClutch); // neutral = pedal chora hua
    }
}
