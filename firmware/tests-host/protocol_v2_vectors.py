"""RSH protocol v2.2 — reference encoder + golden vectors (no-pairing redesign).
Run:  python3 -I tests-host/protocol_v2_vectors.py
C++ (firmware) and Rust (rsh-protocol) implementations must reproduce these exact bytes.
Contract: docs/PROTOCOL-V2.2.md (authority). v2.1 layouts are superseded.

Conventions: all integers little-endian. CRC16-CCITT-FALSE (poly 0x1021,
init 0xFFFF), always last, over every byte before the CRC field.

v2.2 changes (NO-PAIRING-PLAN.md §3 D1/D2/D13 + coordinator wire decision):
- Stream/status/probe version byte 0x21 -> 0x22; HELLO proto bytes (2,1) -> (2,2).
- HELLO 92 B, but nonce/device_key/wifi_password are always ZERO (reserved);
  flags: only bit2 safe_mode survives (bits 0/1/3 reserved 0).
- Control frames: NO HMAC tag, NO counter field. Layout:
  "RSHC" | cmd u8 | reqId u16 LE | len u16 LE | payload[len] | CRC16.
  Header 9 B, total 11 + len, max frame 1035 B. Duplicate suppression
  keys on reqId alone (monotonic u16 per session).
- SET_PASSWORD payload: [curLen u8][curPw][newLen u8][newPw]; wrong
  current password -> error code 1 (BadPassword).
"""
import struct

def crc16(data: bytes) -> int:           # CRC16-CCITT-FALSE, poly 0x1021, init 0xFFFF
    c = 0xFFFF
    for b in data:
        c ^= b << 8
        for _ in range(8):
            c = ((c << 1) ^ 0x1021) & 0xFFFF if c & 0x8000 else (c << 1) & 0xFFFF
    return c

def with_crc(body: bytes) -> bytes:
    return body + struct.pack("<H", crc16(body))

# ---- v1 sanity: proves this CRC matches the shipped protocol -----------------
V1 = bytes.fromhex("a5013412040302 01fc08e803d007050175bd".replace(" ", ""))
assert crc16(V1[:16]) == struct.unpack("<H", V1[16:])[0], "CRC16 does not match v1 golden packet"
assert crc16(b"123456789") == 0x29B1, "CRC check value failed"

# ---- v2.2 data packet (board -> PC, UDP 4211) --------------------------------
# Layout (PROTOCOL-V2.2.md 1.1): "RSH2" | ver 0x22 | flags | seq u16 | ts u32 |
#   axisMask | nEnc | buttons u32 | rsv u16=0 | axes u16 x N (mV) |
#   encoders i16 x nEnc (cumulative) | CRC16
def data_v2_2(seq, ts_us, axes, buttons, enc, flags):
    """axes: dict {axis_index(0-7): u16 mV}; enc: list of i16 cumulative counts."""
    assert len(enc) <= 4 and len(axes) <= 8
    mask = 0
    for i in axes: mask |= 1 << i
    body = struct.pack("<4sBBHIBBIH", b"RSH2", 0x22, flags, seq, ts_us,
                       mask, len(enc), buttons, 0)
    for i in sorted(axes): body += struct.pack("<H", axes[i])
    for e in enc: body += struct.pack("<h", e)
    return with_crc(body)

# ---- v2.2 status packet (board -> PC, 1 Hz, UDP 4211, magic 0xA6) -----------
# Base 34 B (PROTOCOL-V2.2.md 1.2); 38 B extended when btns_ex is given
# (buttons 32..63). seqGap/txFail sit before the trailing CRC16.
def status_v2_2(seq, uptime_ms, rssi, batt_pct, supply_mv, heap, overruns,
                max_cycle_us, gov, reset_reason, cfg_crc, cfg_ver,
                seq_gap, tx_fail, btns_ex=None):
    body = struct.pack("<BBHIbBHIHHBBIH", 0xA6, 0x22, seq, uptime_ms, rssi, batt_pct,
                       supply_mv, heap, overruns, max_cycle_us, gov, reset_reason,
                       cfg_crc, cfg_ver)
    if btns_ex is not None:
        body += struct.pack("<I", btns_ex)
    body += struct.pack("<HH", seq_gap, tx_fail)
    return with_crc(body)

# ---- HELLO reply (board -> PC, UDP 4211) — 92 B fixed, v2.2 ------------------
# Layout (PROTOCOL-V2.2.md 2.2): "RSHR" | proto 2.2 | flags | sessionEcho |
#   boardMac 6B | fw 4.0.3 | rsv | nonce 16B (=0) | deviceKey 32B (=0) |
#   wifiPassword 24B (=0) | CRC16. Secret fields are ALWAYS zero.
def hello_reply(flags, session_echo, board_mac, fw=(4, 0, 8)):  # fw 4.0.8
    assert len(board_mac) == 6
    body = struct.pack("<4sBBBB", b"RSHR", 2, 2, flags, session_echo) \
        + board_mac + struct.pack("<BBBB", fw[0], fw[1], fw[2], 0) + bytes(72)
    return with_crc(body)

# ---- control frame v2.2 (PC <-> board, UDP 4212) -----------------------------
# Layout (PROTOCOL-V2.2.md 3.1): "RSHC" | cmd | reqId u16 | len u16 |
#   payload | CRC16. Header 9 B, total 11 + len, max 1035 B.
def ctrl_frame(cmd: int, req_id: int, payload: bytes = b"") -> bytes:
    assert len(payload) <= 1024
    head = struct.pack("<4sBHH", b"RSHC", cmd, req_id, len(payload))
    return with_crc(head + payload)

def ctrl_crc_ok(frame: bytes) -> bool:
    """Structure + CRC check over a received frame; False on any mismatch/truncation."""
    if len(frame) < 11 or frame[:4] != b"RSHC":
        return False
    cmd, req_id, ln = struct.unpack("<BHH", frame[4:9])
    if ln > 1024 or len(frame) != 9 + ln + 2:
        return False
    return crc16(frame[:-2]) == struct.unpack("<H", frame[-2:])[0]

def ctrl_reply(req_cmd: int, req_id: int, status: int, body: bytes = b"") -> bytes:
    return ctrl_frame(req_cmd | 0x80, req_id, bytes([status]) + body)

# ---- SET_PASSWORD payload v2.2 (D3) ------------------------------------------
# [curLen u8][curPw][newLen u8][newPw]. curLen 0 = no current password.
def set_password_payload(current: bytes, new: bytes) -> bytes:
    assert len(current) <= 63 and 8 <= len(new) <= 63
    return bytes([len(current)]) + current + bytes([len(new)]) + new

# ---- deterministic test fixtures (NOT real secrets) --------------------------
TEST_MAC = bytes.fromhex("aabbccddeeff")

# ---- serial frame (§10, Phase G WS1) -----------------------------------------
# Envelope: "RSHS" | kind u8 | len u16 LE | payload | CRC16 (§0, frame ke
# CRC field se pehle ke tamam bytes par). Kind 1..7, max payload 1035 B
# (v2.2 control max frame).
def serial_frame(kind: int, payload: bytes) -> bytes:
    """Serial frame banao (§10). Payload wahi bytes jo UDP par jate."""
    assert 1 <= kind <= 7 and len(payload) <= 1035
    body = b"RSHS" + struct.pack("<BH", kind, len(payload)) + payload
    return with_crc(body)

VECTORS = {
  # stream v2.2: seq 0x1234, ts 0x01020304, axes 0-4 mV, buttons 0x205,
  # enc cumulative +1000,-250, flags 0x01 (config-changed)
  "data_v2_2": data_v2_2(0x1234, 0x01020304, {0: 2300, 1: 1000, 2: 2000, 3: 0, 4: 512},
                         0x00000205, [1000, -250], 0x01),
  # status v2.2 base 34 B: seqGap 2, txFail 1
  "status_v2_2": status_v2_2(7, 123456, -52, 86, 4012, 187000, 3, 2140, 0, 1,
                             0xDEADBEEF, 5, 2, 1),
  # status v2.2 extended 38 B: btnsEx = buttons 32+5 set (bit 5)
  "status_v2_2_ext": status_v2_2(8, 123457, -50, 86, 4010, 187000, 3, 2140, 0, 1,
                                 0xDEADBEEF, 5, 2, 1, btns_ex=0x00000020),
  # HELLO reply, safe mode (flags 0b00000100), new session echo 1,
  # secret fields all zero, fw 4.0.3
  "hello_safe_mode": hello_reply(0x04, 1, TEST_MAC),
  # HELLO reply, plain (flags 0), echo 0, fw 4.0.3
  "hello_plain": hello_reply(0x00, 0, TEST_MAC),
  # control: GET_CAPS, reqId 1, empty payload
  "ctrl_get_caps": ctrl_frame(0x01, 1, b""),
  # control: SET_CFG chunk, reqId 2 (offset 0, total 64, 16 B data)
  "ctrl_set_cfg": ctrl_frame(0x03, 2,
                             struct.pack("<HHHH", 0, 64, 16, crc16(b"RSHG" + bytes(12)))
                             + b"RSHG" + bytes(12)),
  # control reply: GET_CAPS OK, reqId 1
  "ctrl_get_caps_reply": ctrl_reply(0x01, 1, 0,
                                    TEST_MAC + bytes([4, 0, 8, 2, 2, 8, 64, 4, 6, 0, 0])),  # fw 4.0.8
  # control: SET_PASSWORD, reqId 3, no current password, new 12 chars
  "ctrl_set_password": ctrl_frame(0x0C, 3,
                                  set_password_payload(b"", b"NewPassw0rd!")),
  # serial canonical vectors (§10.1 — C# CanonicalVectorFrame /
  # CanonicalHeloVectorFrame + C++ host tests §22 se byte-exact match):
  # kind=Stream(1), payload 01..08 -> ...d95d (CRC 0x5DD9)
  "serial_stream_0102030405060708": serial_frame(1, bytes(range(1, 9))),
  # kind=Helo(6), payload "HELO" 02 00 -> ...4749 (CRC 0x4947)
  "serial_helo": serial_frame(6, b"HELO" + bytes([2, 0])),
}

# ---- negative vectors (T2): each must FAIL verification / be rejected ---------
def _flip(frame: bytes, idx: int) -> bytes:
    b = bytearray(frame); b[idx] ^= 0xFF; return bytes(b)

_good = VECTORS["ctrl_get_caps"]
NEGATIVE = {
  # CRC byte flipped -> BAD_CRC (dispatcher silent drop)
  "neg_bad_crc": _flip(_good, len(_good) - 1),
  # truncated: last 8 bytes cut off
  "neg_truncated": _good[:-8],
  # len field tampered (0 -> 5) with CRC fixed -> BAD_LENGTH
  "neg_bad_len": with_crc(_good[:7] + struct.pack("<H", 5) + _good[9:-2]),
  # duplicate: byte-identical retransmit of the good frame (same reqId)
  "neg_duplicate_exact": _good,
  # bad magic
  "neg_bad_magic": b"XXXX" + _good[4:],
}

if __name__ == "__main__":
    for k, v in VECTORS.items():
        print(f"{k:22s} len={len(v):3d}  {v.hex(' ')}")
    for k, v in NEGATIVE.items():
        print(f"{k:22s} len={len(v):3d}  {v.hex(' ')}")
    # structural checks
    d = VECTORS["data_v2_2"]; assert len(d) == 20 + 2*5 + 2*2 + 2
    assert crc16(d[:-2]) == struct.unpack("<H", d[-2:])[0]
    assert d[:4] == b"RSH2" and d[4] == 0x22
    s = VECTORS["status_v2_2"]; assert len(s) == 34
    assert crc16(s[:-2]) == struct.unpack("<H", s[-2:])[0]
    assert struct.unpack("<H", s[28:30])[0] == 2 and struct.unpack("<H", s[30:32])[0] == 1
    sx = VECTORS["status_v2_2_ext"]; assert len(sx) == 38
    assert struct.unpack("<I", sx[28:32])[0] == 0x20
    assert crc16(sx[:-2]) == struct.unpack("<H", sx[-2:])[0]
    h = VECTORS["hello_safe_mode"]; assert len(h) == 92
    assert crc16(h[:-2]) == struct.unpack("<H", h[-2:])[0]
    assert h[4] == 2 and h[5] == 2 and h[6] == 0x04
    assert h[14:17] == bytes([4, 0, 8]) and h[18:90] == b"\x00" * 72  # fw 4.0.8
    hp = VECTORS["hello_plain"]; assert len(hp) == 92 and hp[6] == 0 and hp[18:90] == b"\x00" * 72
    c = VECTORS["ctrl_get_caps"]; assert len(c) == 11
    assert ctrl_crc_ok(c), "good control frame must pass CRC"
    r = VECTORS["ctrl_get_caps_reply"]
    assert ctrl_crc_ok(r) and r[4] == (0x01 | 0x80) and len(r) == 11 + 18
    sp = VECTORS["ctrl_set_password"]
    assert ctrl_crc_ok(sp) and sp[4] == 0x0C
    assert sp[9] == 0 and sp[10] == 12 and sp[11:23] == b"NewPassw0rd!"
    # serial canonical vectors (§10.1 — C# + C++ host tests §22 se byte-exact)
    sv = VECTORS["serial_stream_0102030405060708"]
    assert len(sv) == 17 and sv.hex() == "525348530108000102030405060708d95d"
    assert crc16(sv[:-2]) == struct.unpack("<H", sv[-2:])[0]
    sh = VECTORS["serial_helo"]
    assert len(sh) == 15 and sh.hex() == "5253485306060048454c4f02004749"
    assert crc16(sh[:-2]) == struct.unpack("<H", sh[-2:])[0]
    # serial negative: CRC byte flip -> CRC fail (receiver drop + count)
    assert crc16(_flip(sv, len(sv) - 1)[:-2]) != struct.unpack("<H", _flip(sv, len(sv) - 1)[-2:])[0]
    # negative checks (T2)
    assert not ctrl_crc_ok(NEGATIVE["neg_bad_crc"]), "bad CRC must fail"
    assert not ctrl_crc_ok(NEGATIVE["neg_truncated"]), "truncated must fail"
    assert not ctrl_crc_ok(NEGATIVE["neg_bad_len"]), "bad len must fail"
    assert not ctrl_crc_ok(NEGATIVE["neg_bad_magic"]), "bad magic must fail"
    # duplicate is a state rule, not a CRC rule: the frame passes CRC,
    # the receiver must replay the cached reply by reqId.
    assert ctrl_crc_ok(NEGATIVE["neg_duplicate_exact"])
    print("OK: v1 golden CRC matches; v2.2 vectors self-consistent; "
          "negative vectors fail as expected (CRC/length/state rules)")
