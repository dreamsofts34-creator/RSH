using RSH.Core.Protocol;

namespace RSH.Tests;

/// <summary>
/// GET_CAPS naye modules tests — FW-11 (Phase D): Handbrake, Shifter, Horn
/// + LED/encoders ki pin capability rows BoardCaps parser se guzrein.
/// Body bytes firmware/rsh/modules.h (rsh_caps_build_body) wale layout se
/// hain — C++ host test wahi body banata hai, ye parser usay parhta hai.
/// (Parser pehle se maujood hai — BoardCaps.cs, Phase C; ye file sirf
/// FW-11 rows ki coverage deti hai.)
/// </summary>
public class BoardCapsModuleTests
{
    // Firmware default table (modules.h rsh_caps_default_table):
    // steering 34, accel 35, brake 36, handbrake 32, battery 39,
    // shifter 27/33, horn 4, buttons 25/26, encoders 23/19/16/17, LED 21.
    private static byte[] BuildDefaultBody(byte wifiChannel = 6, byte capsFlags = 0)
    {
        var entries = new (byte pin, byte kind, byte module, byte caps)[]
        {
            (34, 1, 1, 0x05), // steering ADC_NATIVE
            (35, 1, 2, 0x05), // accelerator
            (36, 1, 3, 0x05), // brake
            (32, 1, 5, 0x15), // handbrake ADC_NATIVE + pullup
            (39, 1, 11, 0x05), // battery
            (27, 3, 6, 0x11), // shifter up DIGITAL_IN + pullup
            (33, 3, 6, 0x15), // shifter down (ADC-capable pin)
            (4, 3, 7, 0x11),  // horn DIGITAL_IN + pullup
            (25, 3, 8, 0x11), // button
            (26, 3, 8, 0x11), // button
            (23, 3, 9, 0x11), // encoder 1A
            (19, 3, 9, 0x11), // encoder 1B
            (16, 3, 9, 0x11), // encoder 2A
            (17, 3, 9, 0x11), // encoder 2B
            (21, 5, 10, 0x02), // LED strip LED_STRIP output
        };
        var body = new byte[18 + 8 * entries.Length];
        body[0] = 0; // OK
        new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF }.CopyTo(body, 1);
        body[7] = 4; body[8] = 0; body[9] = 0;     // fw 4.0.0
        body[10] = 2; body[11] = 1;                 // proto 2.1
        body[12] = 8; body[13] = 64; body[14] = 4; // limits
        body[15] = wifiChannel;
        body[16] = capsFlags;
        body[17] = (byte)entries.Length;
        for (int i = 0; i < entries.Length; i++)
        {
            body[18 + i * 8 + 0] = entries[i].pin;
            body[18 + i * 8 + 1] = entries[i].kind;
            body[18 + i * 8 + 2] = entries[i].module;
            body[18 + i * 8 + 3] = entries[i].caps;
        }
        return body;
    }

    [Fact]
    public void DefaultBody_Parses_NewModuleRows()
    {
        Assert.True(BoardCaps.TryParse(BuildDefaultBody(), out var caps, out var error));
        Assert.Null(error);
        Assert.NotNull(caps);
        Assert.Equal(new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF }, caps!.BoardMac);
        Assert.Equal(new Version(4, 0, 0), caps.FirmwareVersion);
        Assert.Equal(new Version(2, 1), caps.ProtocolVersion);
        Assert.Equal(8, caps.NAxesMax);
        Assert.Equal(64, caps.NBtnMax);
        Assert.Equal(4, caps.NEncMax);
        Assert.Equal(6, caps.WifiChannel);
        Assert.Equal(15, caps.PinTable.Count);

        // Handbrake: analog pin 32, ADC_NATIVE, caps adc+input+pullup.
        var hb = caps.PinTable.FirstOrDefault(p => p.Module == CapsPinModule.Handbrake);
        Assert.Equal(32, hb.Pin);
        Assert.Equal(CapsPinKind.AdcNative, hb.Kind);
        Assert.True(hb.IsAdc && hb.IsInput && hb.HasPullup);

        // Shifter: sequential up/down, 2 rows.
        var shifter = caps.PinTable.Where(p => p.Module == CapsPinModule.Shifter).ToArray();
        Assert.Equal(2, shifter.Length);
        Assert.Contains(shifter, p => p.Pin == 27);
        Assert.Contains(shifter, p => p.Pin == 33);

        // Horn: button pin 4.
        var horn = caps.PinTable.FirstOrDefault(p => p.Module == CapsPinModule.Horn);
        Assert.Equal(4, horn.Pin);
        Assert.Equal(CapsPinKind.DigitalIn, horn.Kind);

        // LED: strip GPIO21 (R-L9), LED_STRIP kind, output cap.
        var led = caps.PinTable.FirstOrDefault(p => p.Module == CapsPinModule.Led);
        Assert.Equal(21, led.Pin);
        Assert.Equal(CapsPinKind.LedStrip, led.Kind);
        Assert.True(led.IsOutput);
        Assert.False(led.IsInput);

        // Encoders: 4 pins, 2 encoders. Clutch v3.3 map mein nahi.
        Assert.Equal(4, caps.PinTable.Count(p => p.Module == CapsPinModule.Encoders));
        Assert.DoesNotContain(caps.PinTable, p => p.Module == CapsPinModule.Clutch);
        // Koi pin do dafa nahi.
        Assert.Equal(caps.PinTable.Count, caps.PinTable.Select(p => p.Pin).Distinct().Count());
    }

    [Fact]
    public void SlowAxisFlag_Parses()
    {
        Assert.True(BoardCaps.TryParse(BuildDefaultBody(capsFlags: 0x01), out var caps, out _));
        Assert.NotNull(caps);
        Assert.True(caps!.SlowAxis);
        Assert.False(caps.SafeMode);
    }

    [Fact]
    public void EmptyPinTable_Parses()
    {
        var body = new byte[18];
        body[12] = 8; body[13] = 64; body[14] = 4;
        Assert.True(BoardCaps.TryParse(body, out var caps, out _));
        Assert.NotNull(caps);
        Assert.Empty(caps!.PinTable);
    }

    [Fact]
    public void Negatives_Rejected()
    {
        // Too short.
        Assert.False(BoardCaps.TryParse(new byte[17], out _, out _));
        // pinCount/length mismatch (pinCount 15 lekin body 18 B).
        var shortBody = BuildDefaultBody();
        Array.Resize(ref shortBody, 18);
        Assert.False(BoardCaps.TryParse(shortBody, out _, out var e1));
        Assert.NotNull(e1);
        // pinCount > 40.
        var big = BuildDefaultBody();
        big[17] = 41;
        Assert.False(BoardCaps.TryParse(big, out _, out _));
        // Status != OK.
        var badStatus = BuildDefaultBody();
        badStatus[0] = 6; // UNSUPPORTED
        Assert.False(BoardCaps.TryParse(badStatus, out _, out _));
        // capsFlags reserved bits.
        var badFlags = BuildDefaultBody(capsFlags: 0x10);
        Assert.False(BoardCaps.TryParse(badFlags, out _, out _));
        // Unknown kind.
        var badKind = BuildDefaultBody();
        badKind[18 + 1] = 12;
        Assert.False(BoardCaps.TryParse(badKind, out _, out _));
        // Unknown module.
        var badModule = BuildDefaultBody();
        badModule[18 + 2] = 12;
        Assert.False(BoardCaps.TryParse(badModule, out _, out _));
        // Caps reserved bits.
        var badCaps = BuildDefaultBody();
        badCaps[18 + 3] = 0x20;
        Assert.False(BoardCaps.TryParse(badCaps, out _, out _));
        // Entry reserved nonzero.
        var badRsv = BuildDefaultBody();
        badRsv[18 + 4] = 1;
        Assert.False(BoardCaps.TryParse(badRsv, out _, out _));
    }
}
