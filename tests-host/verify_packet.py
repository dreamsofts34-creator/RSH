#!/usr/bin/env python3
"""RSH protocol v2.1 — independent Python cross-check (WP3, Phase A).
Contract: docs/PROTOCOL-V2.1.md (authority). Golden vectors:
tests-host/protocol_v2_vectors.py (VECTORS/NEGATIVE).

Ye script C# (RSH.Core/Protocol) aur C++ (firmware/rsh/protocol.h) parsers
ka teesra, aazaad implementation hai — T1 gate (3-language match) ka Python
hissa. C# / C++ code se kuch share nahi karta, sirf doc + vectors se likha
gaya hai.

NOTE: ye script Virtual Test Rig (~/workspace/projects/rsh/test-env/) mein
chalega — rig tayyar hone tak isay chalana mana hai (standing rule).
"""
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import protocol_v2_vectors as vec  # noqa: E402  (reference encoder + golden vectors)

# ---- CRC16-CCITT-FALSE (poly 0x1021, init 0xFFFF) ---------------------------
def crc16_ccitt_false(data: bytes) -> int:
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc

assert crc16_ccitt_false(b"123456789") == 0x29B1, "CRC self-check failed"

CHECKS = 0


def check(cond, label):
    global CHECKS
    CHECKS += 1
    assert cond, f"FAIL: {label}"
    print(f"  ok: {label}")


def _crc_ok(frame: bytes) -> bool:
    return len(frame) >= 2 and crc16_ccitt_false(frame[:-2]) == struct.unpack("<H", frame[-2:])[0]


def _fix_crc(frame: bytearray) -> bytes:
    frame[-2:] = struct.pack("<H", crc16_ccitt_false(bytes(frame[:-2])))
    return bytes(frame)


# ---- v2.1 stream data packet (§1.1) -----------------------------------------
def parse_data_v2(frame: bytes) -> dict:
    """Reject reasons §1.4 ke mutabiq: badMagic / badVersion / badFrame /
    rsvNonzero / badCrc (ValueError ka message = counter ka naam)."""
    if len(frame) < 2:
        raise ValueError("tooShort")
    if frame[:2] == b"\xa5\x01":
        raise ValueError("legacyV1")
    if len(frame) < 4 or frame[:4] != b"RSH2":
        raise ValueError("badMagic")
    if len(frame) < 5:
        raise ValueError("tooShort")
    if frame[4] != 0x21:
        raise ValueError("badVersion")
    if len(frame) < 20:
        raise ValueError("tooShort")
    flags, axis_mask, n_enc = frame[5], frame[12], frame[13]
    if n_enc > 4:
        raise ValueError("badFrame")
    if len(frame) != 20 + 2 * bin(axis_mask).count("1") + 2 * n_enc + 2:
        raise ValueError("badFrame")
    if struct.unpack("<H", frame[18:20])[0] != 0:
        raise ValueError("rsvNonzero")
    if flags & 0xF0:
        raise ValueError("rsvNonzero")  # flags bits 4-7 reserved 0 (§1.3)
    if not _crc_ok(frame):
        raise ValueError("badCrc")
    off = 20
    axes = {}
    for i in range(8):
        if axis_mask & (1 << i):
            axes[i] = struct.unpack("<H", frame[off:off + 2])[0]
            off += 2
    enc = [struct.unpack("<h", frame[off + 2 * i:off + 2 * i + 2])[0] for i in range(n_enc)]
    return {"seq": struct.unpack("<H", frame[6:8])[0],
            "ts": struct.unpack("<I", frame[8:12])[0],
            "flags": flags, "axis_mask": axis_mask,
            "buttons": struct.unpack("<I", frame[14:18])[0],
            "axes": axes, "enc": enc}


# ---- v2.1 status packet (§1.2) ----------------------------------------------
def parse_status_v2(frame: bytes) -> dict:
    if len(frame) < 2:
        raise ValueError("tooShort")
    if frame[0] != 0xA6:
        raise ValueError("badMagic")
    if frame[1] != 0x21:
        raise ValueError("badVersion")
    if len(frame) < 34:
        raise ValueError("tooShort")
    if len(frame) not in (34, 38):
        raise ValueError("badLength")
    if not _crc_ok(frame):
        raise ValueError("badCrc")
    ext = len(frame) == 38
    gap_off = 32 if ext else 28
    base = {"seq": struct.unpack("<H", frame[2:4])[0],
            "uptime": struct.unpack("<I", frame[4:8])[0],
            "rssi": struct.unpack("<b", frame[8:9])[0],
            "battery": frame[9],
            "supply": struct.unpack("<H", frame[10:12])[0],
            "heap": struct.unpack("<I", frame[12:16])[0],
            "overruns": struct.unpack("<H", frame[16:18])[0],
            "max_cycle": struct.unpack("<H", frame[18:20])[0],
            "governor": frame[20], "reset_reason": frame[21],
            "cfg_crc": struct.unpack("<I", frame[22:26])[0],
            "cfg_ver": struct.unpack("<H", frame[26:28])[0],
            "seq_gap": struct.unpack("<H", frame[gap_off:gap_off + 2])[0],
            "tx_fail": struct.unpack("<H", frame[gap_off + 2:gap_off + 4])[0],
            "btns_ex": struct.unpack("<I", frame[28:32])[0] if ext else None,
            "has_btns_ex": ext}
    return base


# ---- HELLO reply (§2.2, 92 B fixed) -----------------------------------------
def parse_hello(frame: bytes) -> dict:
    if len(frame) < 6:
        raise ValueError("tooShort")
    if frame[:4] != b"RSHR":
        raise ValueError("badMagic")
    if frame[4] != 2 or frame[5] != 1:
        raise ValueError("badVersion")
    if len(frame) < 92:
        raise ValueError("tooShort")
    if len(frame) != 92:
        raise ValueError("badLength")
    if frame[17] != 0 or (frame[6] & 0xF0):
        raise ValueError("rsvNonzero")
    if not _crc_ok(frame):
        raise ValueError("badCrc")
    return {"flags": frame[6], "session_echo": frame[7],
            "board_mac": frame[8:14], "fw": (frame[14], frame[15], frame[16]),
            "nonce": frame[18:34], "device_key": frame[34:66],
            "wifi_password": frame[66:90]}


# ---- PIN_PROBE frame (§3.8) -------------------------------------------------
def build_probe(seq, pin, mode, samples):
    body = struct.pack("<4sBHB BH".replace(" ", ""), b"RSHP", 0x21, seq, pin, mode,
                       len(samples))
    if mode == 1:
        body += b"".join(struct.pack("<H", s) for s in samples)
    else:
        body += bytes(samples)
    return body + struct.pack("<H", crc16_ccitt_false(body))


def parse_probe(frame: bytes) -> dict:
    if len(frame) < 6:
        raise ValueError("tooShort")
    if frame[:4] != b"RSHP":
        raise ValueError("badMagic")
    if frame[4] != 0x21:
        raise ValueError("badVersion")
    if len(frame) < 11:
        raise ValueError("tooShort")
    mode = frame[8]
    n = struct.unpack("<H", frame[9:11])[0]
    if mode > 1 or n < 1 or n > 64:
        raise ValueError("badFrame")
    if len(frame) != 11 + n * (2 if mode == 1 else 1) + 2:
        raise ValueError("badFrame")
    if not _crc_ok(frame):
        raise ValueError("badCrc")
    if mode == 0:
        samples = list(frame[11:11 + n])
    else:
        samples = [struct.unpack("<H", frame[11 + 2 * i:13 + 2 * i])[0] for i in range(n)]
    return {"seq": struct.unpack("<H", frame[5:7])[0], "pin": frame[7],
            "mode": mode, "samples": samples}


# ---- control frame (§3.1/§3.2) ----------------------------------------------
def parse_ctrl_structure(frame: bytes) -> dict:
    if len(frame) < 29:
        raise ValueError("tooShort")
    if frame[:4] != b"RSHC":
        raise ValueError("badMagic")
    ln = struct.unpack("<H", frame[11:13])[0]
    if ln > 1024 or len(frame) != 13 + ln + 16:
        raise ValueError("badLength")
    return {"cmd": frame[4], "req_id": struct.unpack("<H", frame[5:7])[0],
            "counter": struct.unpack("<I", frame[7:11])[0],
            "payload": frame[13:13 + ln], "tag": frame[13 + ln:]}


class ControlSession:
    """Duplicate/replay state (§3.2): pichle 32 (reqId, counter) pairs cache."""

    def __init__(self):
        self.max_seen = None
        self.recent = []  # list of (req_id, counter), max 32

    def classify(self, req_id, counter):
        if (req_id, counter) in self.recent:
            return "duplicate"
        if self.max_seen is not None and counter <= self.max_seen:
            return "replay"
        self.max_seen = counter
        self.recent.append((req_id, counter))
        self.recent = self.recent[-32:]
        return "accept"


# ---- v1 sanity (unchanged, Appendix A) --------------------------------------
V1 = bytes.fromhex("a5013412040302 01fc08e803d007050175bd".replace(" ", ""))
assert crc16_ccitt_false(V1[:16]) == struct.unpack("<H", V1[16:])[0]
assert V1[:2] == b"\xa5\x01", "legacy v1 pehchan"


def main():
    print("== T1: golden vectors (protocol_v2_vectors.py) ==")
    d = parse_data_v2(vec.VECTORS["data_v2_1"])
    check(d["seq"] == 0x1234 and d["ts"] == 0x01020304, "data: seq/ts")
    check(d["flags"] == 0x01 and d["axis_mask"] == 0x1F, "data: flags/axisMask")
    check(d["axes"] == {0: 2300, 1: 1000, 2: 2000, 3: 0, 4: 512}, "data: axes mV")
    check(d["buttons"] == 0x205 and d["enc"] == [1000, -250], "data: buttons u32 + enc cumulative")

    s = parse_status_v2(vec.VECTORS["status_v2_1"])
    check(len(vec.VECTORS["status_v2_1"]) == 34 and not s["has_btns_ex"], "status base: 34 B")
    check(s["seq"] == 7 and s["uptime"] == 123456 and s["rssi"] == -52, "status base: seq/uptime/rssi")
    check(s["battery"] == 86 and s["supply"] == 4012 and s["heap"] == 187000, "status base: battery/supply/heap")
    check(s["overruns"] == 3 and s["max_cycle"] == 2140, "status base: overruns/maxCycle")
    check(s["cfg_crc"] == 0xDEADBEEF and s["cfg_ver"] == 5, "status base: cfgCrc/cfgVer")
    check(s["seq_gap"] == 2 and s["tx_fail"] == 1, "status base: seqGap@28-29 / txFail@30-31")

    sx = parse_status_v2(vec.VECTORS["status_v2_1_ext"])
    check(len(vec.VECTORS["status_v2_1_ext"]) == 38 and sx["has_btns_ex"], "status ext: 38 B")
    check(sx["btns_ex"] == 0x20 and sx["seq_gap"] == 2 and sx["tx_fail"] == 1,
          "status ext: btnsEx@28-31 / seqGap@32-33 / txFail@34-35")

    h = parse_hello(vec.VECTORS["hello_unpaired"])
    check(h["flags"] == 0x09 and h["session_echo"] == 1, "hello unpaired: flags/sessionEcho")
    check(h["board_mac"] == vec.TEST_MAC and h["fw"] == (4, 0, 0), "hello unpaired: mac/fw 4.0.0")
    check(h["nonce"] == vec.TEST_NONCE and h["device_key"] == vec.TEST_KEY, "hello unpaired: nonce/key")
    check(h["wifi_password"] == vec.TEST_PW, "hello unpaired: wifi password 24 chars")
    hp = parse_hello(vec.VECTORS["hello_paired"])
    check(hp["flags"] == 0 and hp["device_key"] == b"\x00" * 32
          and hp["wifi_password"] == b"\x00" * 24, "hello paired: key/password zero")

    c = parse_ctrl_structure(vec.VECTORS["ctrl_get_caps"])
    check(c["cmd"] == 0x01 and c["req_id"] == 1 and c["counter"] == 0 and c["payload"] == b"",
          "ctrl GET_CAPS: fields")
    check(vec.ctrl_verify(vec.TEST_KEY, vec.TEST_NONCE, vec.VECTORS["ctrl_get_caps"]),
          "ctrl GET_CAPS: HMAC tag verifies")
    c2 = parse_ctrl_structure(vec.VECTORS["ctrl_set_cfg"])
    check(c2["cmd"] == 0x03 and c2["req_id"] == 2 and c2["counter"] == 1, "ctrl SET_CFG: fields")
    check(struct.unpack("<HHHH", c2["payload"][:8]) == (0, 64, 16, crc16_ccitt_false(b"RSHG" + bytes(12))),
          "ctrl SET_CFG: chunk offset/total/len/crc")
    check(c2["payload"][8:12] == b"RSHG", "ctrl SET_CFG: blob magic RSHG")
    check(vec.ctrl_verify(vec.TEST_KEY, vec.TEST_NONCE, vec.VECTORS["ctrl_set_cfg"]),
          "ctrl SET_CFG: HMAC tag verifies")
    r = parse_ctrl_structure(vec.VECTORS["ctrl_get_caps_reply"])
    check(r["cmd"] == 0x81 and r["payload"][0] == 0 and len(r["payload"]) == 18,
          "ctrl reply: cmd 0x81, status OK")
    check(r["payload"][1:7] == vec.TEST_MAC and r["payload"][13] == 64,
          "ctrl reply: GET_CAPS body (mac, nBtnMax 64)")
    check(vec.ctrl_verify(vec.TEST_KEY, vec.TEST_NONCE, vec.VECTORS["ctrl_get_caps_reply"]),
          "ctrl reply: HMAC tag verifies")

    print("== T2: negative vectors ==")
    neg = vec.NEGATIVE
    check(not vec.ctrl_verify(vec.TEST_KEY, vec.TEST_NONCE, neg["neg_bad_tag"]), "neg: bad tag fails")
    check(not vec.ctrl_verify(vec.WRONG_KEY, vec.TEST_NONCE, neg["neg_wrong_key_frame"]),
          "neg: wrong key fails")
    check(not vec.ctrl_verify(vec.TEST_KEY, vec.TEST_NONCE, neg["neg_truncated"]), "neg: truncated fails")
    check(not vec.ctrl_verify(vec.TEST_KEY, vec.TEST_NONCE, neg["neg_bad_magic"]), "neg: bad magic fails")
    try:
        parse_ctrl_structure(struct.pack("<4sBHIH", b"RSHC", 1, 1, 0, 1025) + bytes(1025) + bytes(16))
        raise AssertionError("oversize len 1025 must be rejected")
    except ValueError as e:
        check(str(e) == "badLength", "neg: control len 1025 → badLength (TOO_BIG)")
    # replay + duplicate: tag valid hai, reject sirf counter-state se hota hai (§3.2)
    check(vec.ctrl_verify(vec.TEST_KEY, vec.TEST_NONCE, neg["neg_replay_same_counter_new_reqid"]),
          "neg replay: tag valid (reqId HMAC mein nahi)")
    check(vec.ctrl_verify(vec.TEST_KEY, vec.TEST_NONCE, neg["neg_duplicate_exact"]),
          "neg duplicate: tag valid (exact retransmit)")
    sess = ControlSession()
    check(sess.classify(1, 0) == "accept", "session: pehla frame accept (counter 0)")
    check(sess.classify(1, 0) == "duplicate", "session: exact retransmit → duplicate")
    check(sess.classify(99, 0) == "replay", "session: naya reqId, counter<=max → replay")
    check(sess.classify(2, 1) == "accept" and sess.classify(3, 5) == "accept",
          "session: naya counter / gap accept")
    # stream negatives
    good = vec.VECTORS["data_v2_1"]
    for label, mutate in [
        ("badMagic", lambda b: b.__setitem__(0, ord("X"))),
        ("badVersion", lambda b: b.__setitem__(4, 0x20)),
        ("badCrc", lambda b: b.__setitem__(20, b[20] ^ 0xFF)),
    ]:
        b = bytearray(good)
        mutate(b)
        try:
            parse_data_v2(bytes(b))
            raise AssertionError(f"stream {label} must be rejected")
        except ValueError as e:
            check(str(e) == label, f"neg stream: {label}")
    try:
        parse_data_v2(good[:-2])
        raise AssertionError("truncated stream must be rejected")
    except ValueError as e:
        check(str(e) == "badFrame", "neg stream: truncated → badFrame")
    b = bytearray(good)
    b[13] = 5
    try:
        parse_data_v2(bytes(b))
        raise AssertionError("nEnc 5 must be rejected")
    except ValueError as e:
        check(str(e) == "badFrame", "neg stream: nEnc 5 → badFrame")
    b = bytearray(vec.data_v2_1(1, 100, {}, 0, [], 0))
    b[18] = 1
    try:
        parse_data_v2(_fix_crc(b))
        raise AssertionError("reserved nonzero must be rejected")
    except ValueError as e:
        check(str(e) == "rsvNonzero", "neg stream: reserved 18-19 → rsvNonzero")
    b = bytearray(vec.data_v2_1(1, 100, {}, 0, [], 0))
    b[5] = 0x80
    try:
        parse_data_v2(_fix_crc(b))
        raise AssertionError("flags reserved must be rejected")
    except ValueError as e:
        check(str(e) == "rsvNonzero", "neg stream: flags bits 4-7 → rsvNonzero")
    try:
        parse_data_v2(V1)
        raise AssertionError("v1 packet must route to legacy")
    except ValueError as e:
        check(str(e) == "legacyV1", "legacy: v1 packet (A5 01) → legacy path")

    print("== T3: status field-level + probe ==")
    try:
        parse_status_v2(vec.VECTORS["status_v2_1"] + b"\x00")
        raise AssertionError("35 B status must be rejected")
    except ValueError as e:
        check(str(e) == "badLength", "status: 35 B → badLength")
    b = bytearray(vec.VECTORS["status_v2_1"])
    b[12] ^= 0xFF
    try:
        parse_status_v2(bytes(b))
        raise AssertionError("bad status CRC must be rejected")
    except ValueError as e:
        check(str(e) == "badCrc", "status: bad CRC → badCrc")
    # base vs extended: aik hi values, sirf offsets badalte hain
    sb = vec.status_v2_1(9, 1000, -60, 50, 4000, 100, 1, 100, 0, 1, 42, 3, 7, 9)
    se = vec.status_v2_1(9, 1000, -60, 50, 4000, 100, 1, 100, 0, 1, 42, 3, 7, 9, btns_ex=1)
    pb, pe = parse_status_v2(sb), parse_status_v2(se)
    check(pb["seq_gap"] == pe["seq_gap"] == 7 and pb["tx_fail"] == pe["tx_fail"] == 9,
          "status: base/ext seqGap/txFail barabar")
    check(pe["btns_ex"] == 1 and not pb["has_btns_ex"], "status: ext btnsEx, base mein nahi")
    # probe frame (§3.8)
    probe = build_probe(3, 21, 1, [1200, 3300, 0])
    pf = parse_probe(probe)
    check(pf["seq"] == 3 and pf["pin"] == 21 and pf["mode"] == 1
          and pf["samples"] == [1200, 3300, 0], "probe: ADC samples mV")
    probe0 = build_probe(0, 5, 0, [1, 0, 1])
    check(parse_probe(probe0)["samples"] == [1, 0, 1], "probe: digital levels")

    print(f"PYTHON CROSS-VERIFY PASSED — {CHECKS} checks, "
          "v2.1 stream/status/hello/control/probe vectors + negatives match")


if __name__ == "__main__":
    main()
