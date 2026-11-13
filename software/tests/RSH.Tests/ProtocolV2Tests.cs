using RSH.Core.Protocol;

namespace RSH.Tests;

/// <summary>
/// Protocol v2.1 tests (WP3: T1 golden vectors 3-language match, T2 negative,
/// T3 status field-level). Golden bytes tests-host/protocol_v2_vectors.py se
/// liye gaye hain (VECTORS/NEGATIVE). NOTE: ye tests Virtual Test Rig
/// (~/workspace/projects/rsh/test-env/) mein chalenge — yahan sirf likhe gaye hain.
/// </summary>
public class ProtocolV2Tests
{
    // ---- Golden vectors (protocol_v2_vectors.py) ----
    private const string DataV2Hex =
        "5253483221013412040302011f02050200000000fc08e803d00700000002e80306ff69d1";
    private const string StatusBaseHex =
        "a621070040e20100cc56ac0f78da020003005c080001efbeadde0500020001009700";
    private const string StatusExtHex =
        "a621080041e20100ce56aa0f78da020003005c080001efbeadde05002000000002000100e11e";
    private const string HelloUnpairedHex =
        "5253485202010901aabbccddeeff04000000a0a1a2a3a4a5a6a7a8a9aaabacadaeaf" +
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f" +
        "41623364456636684a6b396d4e70327253743576577838595a42";
    private const string HelloPairedHex =
        "5253485202010000aabbccddeeff04000000a0a1a2a3a4a5a6a7a8a9aaabacadaeaf" +
        "0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000" +
        "2445";
    private const string CtrlGetCapsHex =
        "5253484301010000000000000042ee8d10079358d9936fe74368f8b661";
    private const string CtrlSetCfgHex =
        "52534843030200010000001800" +
        "000040001000eb5252534847000000000000000000000000" +
        "5e0c6566c9dd5e791be0ae3e77662d4c";
    private const string CtrlGetCapsReplyHex =
        "52534843810100000000001200" +
        "00aabbccddeeff0400000201084004060000" +
        "04b2212407634ffd8552f491ba7dcbc0";

    // Deterministic fixtures (NOT real secrets) — protocol_v2_vectors.py jaisi.
    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();
    private static readonly byte[] WrongKey = Enumerable.Repeat((byte)0xFF, 32).ToArray();
    private static readonly byte[] TestNonce = Enumerable.Range(0, 16).Select(i => (byte)(0xA0 + i)).ToArray();

    private static byte[] Hex(string hex) => Convert.FromHexString(hex);

    // ================= T1 — golden vectors =================

    [Fact]
    public void DataV2_Golden_Parses_AllFields()
    {
        var result = WheelPacketV2.TryParse(Hex(DataV2Hex), out var p);
        Assert.Equal(WheelPacketV2ParseResult.Ok, result);
        Assert.Equal((ushort)0x1234, p.Sequence);
        Assert.Equal(0x01020304u, p.TimestampUs);
        Assert.True(p.ConfigChanged); // flags 0x01
        Assert.Equal((byte)0x1F, p.AxisMask);
        Assert.Equal(5, p.AxisCount);
        Assert.Equal((ushort)2300, p.Axis(0));
        Assert.Equal((ushort)1000, p.Axis(1));
        Assert.Equal((ushort)2000, p.Axis(2));
        Assert.Equal((ushort)0, p.Axis(3));
        Assert.Equal((ushort)512, p.Axis(4));
        Assert.Equal(0x00000205u, p.Buttons);
        Assert.True(p.HasButton(0));
        Assert.True(p.HasButton(2));
        Assert.True(p.HasButton(9));
        Assert.False(p.HasButton(1));
        Assert.Equal(2, p.EncoderCount);
        Assert.Equal((short)1000, p.Encoders[0]);
        Assert.Equal((short)-250, p.Encoders[1]);
    }

    [Fact]
    public void DataV2_Build_ThenParse_Roundtrips_AndMatchesGolden()
    {
        byte[] built = WheelPacketV2.Build(0x1234, 0x01020304, 0x01, 0x1F, 0x205,
            new ushort[] { 2300, 1000, 2000, 0, 512 }, new short[] { 1000, -250 });
        Assert.Equal(Hex(DataV2Hex), built);
        Assert.Equal(WheelPacketV2ParseResult.Ok, WheelPacketV2.TryParse(built, out var p));
        Assert.Equal((ushort)0x1234, p.Sequence);
        Assert.Equal((short)-250, p.Encoders[1]);
    }

    [Fact]
    public void StatusV2_GoldenBase_Parses_AllFields()
    {
        byte[] bytes = Hex(StatusBaseHex);
        Assert.Equal(34, bytes.Length);
        var result = StatusPacketV2.TryParse(bytes, out var s);
        Assert.Equal(StatusPacketV2ParseResult.Ok, result);
        Assert.False(s.HasBtnsEx);
        Assert.Equal((ushort)7, s.Sequence);
        Assert.Equal(123456u, s.UptimeMs);
        Assert.Equal((sbyte)-52, s.RssiDbm);
        Assert.Equal((byte)86, s.BatteryPercent);
        Assert.Equal((ushort)4012, s.SupplyMv);
        Assert.Equal(187000u, s.FreeHeapBytes);
        Assert.Equal((ushort)3, s.Overruns);
        Assert.Equal((ushort)2140, s.MaxCycleUs);
        Assert.Equal((byte)0, s.Governor);
        Assert.Equal((byte)1, s.ResetReason);
        Assert.Equal(0xDEADBEEFu, s.CfgCrc);
        Assert.Equal((ushort)5, s.CfgVer);
        Assert.Equal((ushort)2, s.SeqGap);   // bytes 28–29 (§1.2)
        Assert.Equal((ushort)1, s.TxFail);   // bytes 30–31 (§1.2)
    }

    [Fact]
    public void StatusV2_GoldenExtended_Parses_BtnsEx_AndShiftedOffsets()
    {
        byte[] bytes = Hex(StatusExtHex);
        Assert.Equal(38, bytes.Length);
        var result = StatusPacketV2.TryParse(bytes, out var s);
        Assert.Equal(StatusPacketV2ParseResult.Ok, result);
        Assert.True(s.HasBtnsEx);
        Assert.Equal(0x00000020u, s.BtnsEx); // button 32+5 set (bit 5)
        Assert.True(s.HasButtonEx(37));
        Assert.False(s.HasButtonEx(32));
        Assert.Equal((ushort)8, s.Sequence);
        Assert.Equal((ushort)2, s.SeqGap);   // extended mein bytes 32–33
        Assert.Equal((ushort)1, s.TxFail);   // extended mein bytes 34–35
    }

    [Fact]
    public void StatusV2_Build_ThenParse_Roundtrips_BaseAndExtended()
    {
        byte[] baseBuilt = StatusPacketV2.Build(7, 123456, -52, 86, 4012, 187000, 3, 2140, 0, 1,
            0xDEADBEEF, 5, 2, 1, null);
        Assert.Equal(Hex(StatusBaseHex), baseBuilt);
        byte[] extBuilt = StatusPacketV2.Build(8, 123457, -50, 86, 4010, 187000, 3, 2140, 0, 1,
            0xDEADBEEF, 5, 2, 1, 0x20);
        Assert.Equal(Hex(StatusExtHex), extBuilt);
    }

    [Fact]
    public void HelloUnpaired_Golden_Parses_KeyAndPasswordPresent()
    {
        byte[] bytes = Hex(HelloUnpairedHex);
        Assert.Equal(92, bytes.Length);
        var result = HelloReply.TryParse(bytes, out var h);
        Assert.Equal(HelloReplyParseResult.Ok, result);
        Assert.True(h.Unpaired);
        Assert.True(h.PairingWindowOpen);
        Assert.Equal((byte)1, h.SessionEcho);
        Assert.Equal(new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF }, h.BoardMac);
        Assert.Equal((byte)4, h.FwMajor);
        Assert.Equal(TestNonce, h.Nonce);
        Assert.Equal(TestKey, h.DeviceKey);
        Assert.Equal("Ab3dEf6hJk9mNp2rSt5vWx8Y"u8.ToArray(), h.WifiPassword);
    }

    [Fact]
    public void HelloPaired_Golden_Parses_KeyAndPasswordZero()
    {
        var result = HelloReply.TryParse(Hex(HelloPairedHex), out var h);
        Assert.Equal(HelloReplyParseResult.Ok, result);
        Assert.False(h.Unpaired);
        Assert.False(h.PairingWindowOpen);
        Assert.Equal(new byte[32], h.DeviceKey);
        Assert.Equal(new byte[24], h.WifiPassword);
    }

    [Fact]
    public void CtrlGetCaps_Golden_Verifies()
    {
        var result = ControlFrameCodec.TryParse(Hex(CtrlGetCapsHex), TestKey, TestNonce, out var f);
        Assert.Equal(ControlParseResult.Ok, result);
        Assert.Equal((byte)0x01, f.Cmd);
        Assert.Equal((ushort)1, f.ReqId);
        Assert.Equal(0u, f.Counter);
        Assert.Empty(f.Payload);
        Assert.False(f.IsReply);
    }

    [Fact]
    public void CtrlSetCfg_Golden_Verifies_ChunkFields()
    {
        var result = ControlFrameCodec.TryParse(Hex(CtrlSetCfgHex), TestKey, TestNonce, out var f);
        Assert.Equal(ControlParseResult.Ok, result);
        Assert.Equal((byte)0x03, f.Cmd);
        Assert.Equal((ushort)2, f.ReqId);
        Assert.Equal(1u, f.Counter);
        Assert.Equal(24, f.Payload.Length);
        Assert.Equal((ushort)0, BitConverter.ToUInt16(f.Payload, 0));   // offset
        Assert.Equal((ushort)64, BitConverter.ToUInt16(f.Payload, 2));  // total
        Assert.Equal((ushort)16, BitConverter.ToUInt16(f.Payload, 4));  // chunkLen
        Assert.Equal("RSHG"u8.ToArray(), f.Payload.AsSpan(8, 4).ToArray()); // blob magic (§3.6)
    }

    [Fact]
    public void CtrlGetCapsReply_Golden_Verifies_ReplyCmdAndCapsBody()
    {
        var result = ControlFrameCodec.TryParse(Hex(CtrlGetCapsReplyHex), TestKey, TestNonce, out var f);
        Assert.Equal(ControlParseResult.Ok, result);
        Assert.True(f.IsReply);
        Assert.Equal((byte)0x01, f.RequestCmd);
        Assert.Equal(ControlStatus.Ok, f.ReplyStatus);
        Assert.Equal(18, f.Payload.Length);
        Assert.Equal(new byte[] { 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF }, f.Payload.AsSpan(1, 6).ToArray());
        Assert.Equal((byte)4, f.Payload[7]);  // fwMajor
        Assert.Equal((byte)2, f.Payload[10]); // protoMajor
        Assert.Equal((byte)8, f.Payload[12]); // nAxesMax
        Assert.Equal((byte)64, f.Payload[13]); // nBtnMax (wire)
        Assert.Equal((byte)4, f.Payload[14]); // nEncMax
        Assert.Equal((byte)0, f.Payload[17]); // pinCount
    }

    [Fact]
    public void CtrlBuild_ThenParse_Roundtrips()
    {
        byte[] payload = { 0x00, 0x40, 0x00, 0x10, 0x00 };
        byte[] built = ControlFrameCodec.Build(0x03, 2, 1, payload, TestKey, TestNonce);
        var result = ControlFrameCodec.TryParse(built, TestKey, TestNonce, out var f);
        Assert.Equal(ControlParseResult.Ok, result);
        Assert.Equal(payload, f.Payload);
        Assert.Equal(13 + payload.Length + 16, built.Length);
    }

    [Fact]
    public void PacketRouter_Classifies_AllKinds()
    {
        Assert.Equal(PacketKind.StreamV2, PacketRouter.Classify(Hex(DataV2Hex)));
        Assert.Equal(PacketKind.StatusV2, PacketRouter.Classify(Hex(StatusBaseHex)));
        Assert.Equal(PacketKind.HelloV2, PacketRouter.Classify(Hex(HelloUnpairedHex)));
        Assert.Equal(PacketKind.ControlV2, PacketRouter.Classify(Hex(CtrlGetCapsHex)));
        Assert.Equal(PacketKind.LegacyV1, PacketRouter.Classify(new byte[] { 0xA5, 0x01, 0x34, 0x12 }));
        Assert.Equal(PacketKind.Unknown, PacketRouter.Classify(new byte[] { 0x58, 0x58, 0x58, 0x58 }));
    }

    // ================= T2 — negative =================

    [Fact]
    public void DataV2_RejectsBadMagic_Version_Crc_Truncated()
    {
        byte[] golden = Hex(DataV2Hex);

        var badMagic = (byte[])golden.Clone(); badMagic[0] = (byte)'X';
        Assert.Equal(WheelPacketV2ParseResult.BadMagic, WheelPacketV2.TryParse(badMagic, out _));

        var badVersion = (byte[])golden.Clone(); badVersion[4] = 0x20;
        Assert.Equal(WheelPacketV2ParseResult.BadVersion, WheelPacketV2.TryParse(badVersion, out _));

        var badCrc = (byte[])golden.Clone(); badCrc[20] ^= 0xFF; // axis badla, CRC purana
        Assert.Equal(WheelPacketV2ParseResult.BadCrc, WheelPacketV2.TryParse(badCrc, out _));

        // Truncated: aakhri 2 bytes kaat do → length mismatch = BadFrame
        Assert.Equal(WheelPacketV2ParseResult.BadFrame, WheelPacketV2.TryParse(golden.AsSpan(0, golden.Length - 2), out _));
        Assert.Equal(WheelPacketV2ParseResult.TooShort, WheelPacketV2.TryParse(golden.AsSpan(0, 10), out _));
    }

    [Fact]
    public void DataV2_RejectsTooManyEncoders_Reserved_FlagsReserved()
    {
        // nEnc = 5 (header byte 13) — length bhi badal jayegi, pehle BadFrame aana chahiye
        byte[] manyEnc = WheelPacketV2.Build(1, 100, 0, 0x00, 0, ReadOnlySpan<ushort>.Empty, new short[] { 1, 2, 3, 4 });
        manyEnc[13] = 5;
        Assert.Equal(WheelPacketV2ParseResult.BadFrame, WheelPacketV2.TryParse(manyEnc, out _));

        // Reserved bytes 18–19 nonzero (CRC dobara lagao taake sirf reserved fail ho)
        byte[] rsv = WheelPacketV2.Build(1, 100, 0, 0x00, 0, ReadOnlySpan<ushort>.Empty, ReadOnlySpan<short>.Empty);
        rsv[18] = 0x01;
        FixCrc(rsv);
        Assert.Equal(WheelPacketV2ParseResult.ReservedNonzero, WheelPacketV2.TryParse(rsv, out _));

        // Flags bits 4–7 nonzero (bit 7 set) — Build is par throw karta hai,
        // is liye valid packet banao phir flags byte haath se badlo + CRC fix karo.
        byte[] flags = WheelPacketV2.Build(1, 100, 0x00, 0x00, 0, ReadOnlySpan<ushort>.Empty, ReadOnlySpan<short>.Empty);
        flags[5] = 0x80;
        FixCrc(flags);
        Assert.Equal(WheelPacketV2ParseResult.ReservedNonzero, WheelPacketV2.TryParse(flags, out _));
    }

    [Fact]
    public void DataV2_LegacyV1_RoutesToV1Parser()
    {
        byte[] v1Golden =
        {
            0xa5, 0x01, 0x34, 0x12, 0x04, 0x03, 0x02, 0x01,
            0xfc, 0x08, 0xe8, 0x03, 0xd0, 0x07, 0x05, 0x01, 0x75, 0xbd,
        };
        Assert.Equal(WheelPacketV2ParseResult.LegacyV1, WheelPacketV2.TryParse(v1Golden, out _));
        // v1 parser barqarar hai — legacy packet us se parse hota hai.
        Assert.Equal(PacketParseResult.Ok, WheelPacket.TryParse(v1Golden, out var p));
        Assert.Equal((ushort)0x1234, p.Sequence);
    }

    [Fact]
    public void Ctrl_RejectsBadTag_WrongKey_Truncated_BadMagic()
    {
        byte[] good = Hex(CtrlGetCapsHex);

        var badTag = (byte[])good.Clone(); badTag[^1] ^= 0xFF;
        Assert.Equal(ControlParseResult.BadTag, ControlFrameCodec.TryParse(badTag, TestKey, TestNonce, out _));

        Assert.Equal(ControlParseResult.BadTag, ControlFrameCodec.TryParse(good, WrongKey, TestNonce, out _));

        Assert.Equal(ControlParseResult.BadLength, ControlFrameCodec.TryParse(good.AsSpan(0, good.Length - 8), TestKey, TestNonce, out _));

        var badMagic = (byte[])good.Clone();
        badMagic[0] = (byte)'X'; badMagic[1] = (byte)'X'; badMagic[2] = (byte)'X'; badMagic[3] = (byte)'X';
        Assert.Equal(ControlParseResult.BadMagic, ControlFrameCodec.TryParse(badMagic, TestKey, TestNonce, out _));
    }

    [Fact]
    public void Ctrl_RejectsLengthMismatch_AndOversizePayload()
    {
        byte[] good = Hex(CtrlGetCapsHex);
        // len field ko 5 kar do (asal payload 0) → length mismatch
        var lenMismatch = (byte[])good.Clone();
        lenMismatch[11] = 5; lenMismatch[12] = 0;
        Assert.Equal(ControlParseResult.BadLength, ControlFrameCodec.TryParse(lenMismatch, TestKey, TestNonce, out _));

        // 1025 B payload build karna mana hai (§3.1: len > 1024 → TOO_BIG)
        Assert.Throws<ArgumentException>(() =>
            ControlFrameCodec.Build(0x01, 1, 0, new byte[1025], TestKey, TestNonce));
    }

    [Fact]
    public void Ctrl_StructureParse_WorksWithoutValidTag_ForBadTagReplyMatching()
    {
        // BAD_TAG reply ka tag verify nahi ho sakta (key mismatch) — PC sirf
        // reqId/counter match karke use maanta hai (§3.2). Structure parse tag ke baghair chale.
        byte[] good = Hex(CtrlGetCapsHex);
        var badTag = (byte[])good.Clone(); badTag[^1] ^= 0xFF;
        Assert.Equal(ControlParseResult.Ok, ControlFrameCodec.TryParseStructure(badTag, out var f));
        Assert.Equal((ushort)1, f.ReqId);
        Assert.Equal(0u, f.Counter);
    }

    [Fact]
    public void CtrlSession_Classifies_Accept_Duplicate_Replay()
    {
        var session = new ControlSession();
        Assert.Equal(ControlSessionDecision.Accept, session.Classify(1, 0));   // pehla frame, counter 0
        Assert.Equal(ControlSessionDecision.Duplicate, session.Classify(1, 0)); // exact retransmit
        Assert.Equal(ControlSessionDecision.Replay, session.Classify(99, 0));   // naya reqId, counter ≤ max-seen
        Assert.Equal(ControlSessionDecision.Accept, session.Classify(2, 1));    // naya counter
        Assert.Equal(ControlSessionDecision.Accept, session.Classify(3, 5));    // gap accept (khoya packet ok)
        Assert.Equal(ControlSessionDecision.Replay, session.Classify(4, 3));    // purana counter
        Assert.Equal(ControlSessionDecision.Duplicate, session.Classify(3, 5)); // cache mein hai
    }

    [Fact]
    public void CtrlSession_ReplayFrame_VerifiesCryptographically_ButIsRejectedByState()
    {
        // NEGATIVE["neg_replay_same_counter_new_reqid"]: tag valid hai (reqId HMAC mein nahi),
        // reject sirf counter-state rule se hota hai (§3.2).
        byte[] replay = Hex("5253484301630000000000000042ee8d10079358d9936fe74368f8b661");
        Assert.Equal(ControlParseResult.Ok, ControlFrameCodec.TryParse(replay, TestKey, TestNonce, out var f));
        Assert.Equal((ushort)99, f.ReqId);
        var session = new ControlSession();
        Assert.Equal(ControlSessionDecision.Accept, session.Classify(1, 0));
        Assert.Equal(ControlSessionDecision.Replay, session.Classify(f.ReqId, f.Counter));
    }

    [Fact]
    public void ButtonCount_Rule_N1()
    {
        Assert.Equal(ButtonCountResult.Ok, ProtocolV2.CheckButtonCount(0));
        Assert.Equal(ButtonCountResult.Ok, ProtocolV2.CheckButtonCount(32));
        Assert.Equal(ButtonCountResult.Ok, ProtocolV2.CheckButtonCount(64));
        Assert.Equal(ButtonCountResult.Unsupported, ProtocolV2.CheckButtonCount(65));
        Assert.Equal(ButtonCountResult.Unsupported, ProtocolV2.CheckButtonCount(128));
        Assert.Equal(ButtonCountResult.BadFrame, ProtocolV2.CheckButtonCount(129));
    }

    // ================= T3 — status field-level =================

    [Fact]
    public void StatusV2_RejectsBadLength_Magic_Version_Crc()
    {
        byte[] golden = Hex(StatusBaseHex);

        Assert.Equal(StatusPacketV2ParseResult.TooShort, StatusPacketV2.TryParse(golden.AsSpan(0, 33), out _));
        // 35 B: na base na extended
        byte[] len35 = new byte[35];
        golden.AsSpan(0, 34).CopyTo(len35.AsSpan(0, 34));
        Assert.Equal(StatusPacketV2ParseResult.BadLength, StatusPacketV2.TryParse(len35, out _));

        var badMagic = (byte[])golden.Clone(); badMagic[0] = 0xA5;
        Assert.Equal(StatusPacketV2ParseResult.BadMagic, StatusPacketV2.TryParse(badMagic, out _));

        var badVersion = (byte[])golden.Clone(); badVersion[1] = 0x20;
        Assert.Equal(StatusPacketV2ParseResult.BadVersion, StatusPacketV2.TryParse(badVersion, out _));

        var badCrc = (byte[])golden.Clone(); badCrc[12] ^= 0xFF; // heap badla
        Assert.Equal(StatusPacketV2ParseResult.BadCrc, StatusPacketV2.TryParse(badCrc, out _));
    }

    [Fact]
    public void StatusV2_WrapValues_AndEdgeFields()
    {
        // seqGap/txFail wrap allowed; battery 255 = no battery; rssi negative.
        byte[] built = StatusPacketV2.Build(0xFFFF, 0xFFFFFFFF, -128, 255, 0, 0, 0xFFFF, 0xFFFF, 2, 4,
            0, 0xFFFF, 0xFFFF, 0xFFFF, null);
        var result = StatusPacketV2.TryParse(built, out var s);
        Assert.Equal(StatusPacketV2ParseResult.Ok, result);
        Assert.Equal((ushort)0xFFFF, s.Sequence);
        Assert.Equal((ushort)0xFFFF, s.SeqGap);
        Assert.Equal((ushort)0xFFFF, s.TxFail);
        Assert.Equal((sbyte)-128, s.RssiDbm);
        Assert.Equal((byte)255, s.BatteryPercent);
        Assert.Equal((ushort)0, s.SupplyMv); // 0 = not measured
        Assert.Equal((byte)4, s.ResetReason);
    }

    [Fact]
    public void StatusV2_Extended_OnlyBtnsExDiffersFromBase()
    {
        // Aik hi status, base vs extended: seqGap/txFail ki values aik jaisi,
        // sirf offsets badalte hain — dono parse hon aur fields barabar hon.
        byte[] baseBuilt = StatusPacketV2.Build(9, 1000, -60, 50, 4000, 100, 1, 100, 0, 1, 42, 3, 7, 9, null);
        byte[] extBuilt = StatusPacketV2.Build(9, 1000, -60, 50, 4000, 100, 1, 100, 0, 1, 42, 3, 7, 9, 0x00000001);
        Assert.Equal(StatusPacketV2ParseResult.Ok, StatusPacketV2.TryParse(baseBuilt, out var b));
        Assert.Equal(StatusPacketV2ParseResult.Ok, StatusPacketV2.TryParse(extBuilt, out var e));
        Assert.Equal(b.SeqGap, e.SeqGap);
        Assert.Equal(b.TxFail, e.TxFail);
        Assert.Equal((ushort)7, e.SeqGap);
        Assert.Equal((ushort)9, e.TxFail);
        Assert.True(e.HasButtonEx(32));
        Assert.False(b.HasBtnsEx);
    }

    [Fact]
    public void ProbeFrame_Parses_AdcSamples_AndRejectsBadFrames()
    {
        byte[] probe = BuildProbeFrame(seq: 3, pin: 21, mode: 1, samplesMv: new ushort[] { 1200, 3300, 0 });
        var result = ProbeFrame.TryParse(probe, out var f);
        Assert.Equal(ProbeFrameParseResult.Ok, result);
        Assert.Equal((ushort)3, f.Sequence);
        Assert.Equal((byte)21, f.Pin);
        Assert.Equal((byte)1, f.Mode);
        Assert.Equal(3, f.SampleCount);
        Assert.Equal((ushort)1200, f.SamplesMv[0]);
        Assert.Equal((ushort)3300, f.SamplesMv[1]);
        Assert.Equal((ushort)0, f.SamplesMv[2]);

        var badCrc = (byte[])probe.Clone(); badCrc[11] ^= 0xFF;
        Assert.Equal(ProbeFrameParseResult.BadCrc, ProbeFrame.TryParse(badCrc, out _));

        // sampleCount 0 → BadFrame
        byte[] zeroSamples = BuildProbeFrame(seq: 0, pin: 21, mode: 0, samplesLevel: Array.Empty<byte>());
        Assert.Equal(ProbeFrameParseResult.BadFrame, ProbeFrame.TryParse(zeroSamples, out _));
    }

    // ---- helpers ----

    /// <summary>Packet ke aakhri 2 bytes mein CRC dobara lagao (negative tests ke liye).</summary>
    private static void FixCrc(byte[] packet)
    {
        ushort crc = Crc16.Compute(packet.AsSpan(0, packet.Length - 2));
        packet[^2] = (byte)(crc & 0xFF);
        packet[^1] = (byte)(crc >> 8);
    }

    /// <summary>§3.8 ke mutabiq probe frame banao (test helper — board ka aaina).</summary>
    private static byte[] BuildProbeFrame(ushort seq, byte pin, byte mode,
        ushort[]? samplesMv = null, byte[]? samplesLevel = null)
    {
        int n = mode == 1 ? samplesMv!.Length : samplesLevel!.Length;
        var bytes = new byte[11 + n * (mode == 1 ? 2 : 1) + 2];
        bytes[0] = (byte)'R'; bytes[1] = (byte)'S'; bytes[2] = (byte)'H'; bytes[3] = (byte)'P';
        bytes[4] = ProtocolV2.Version;
        bytes[5] = (byte)(seq & 0xFF); bytes[6] = (byte)(seq >> 8);
        bytes[7] = pin;
        bytes[8] = mode;
        bytes[9] = (byte)(n & 0xFF); bytes[10] = (byte)(n >> 8);
        if (mode == 1)
        {
            for (int i = 0; i < n; i++)
            {
                bytes[11 + 2 * i] = (byte)(samplesMv![i] & 0xFF);
                bytes[12 + 2 * i] = (byte)(samplesMv[i] >> 8);
            }
        }
        else
        {
            samplesLevel!.CopyTo(bytes.AsSpan(11, n));
        }
        FixCrc(bytes);
        return bytes;
    }
}
