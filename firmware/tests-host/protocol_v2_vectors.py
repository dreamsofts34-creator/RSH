"""RSH protocol v2.1 — reference encoder + golden vectors (WP3, Phase A).
Run:  python3 -I tests-host/protocol_v2_vectors.py
C++ (firmware) and C# (app) implementations must reproduce these exact bytes.
Contract: docs/PROTOCOL-V2.1.md (authority). v2 draft (PROTOCOL-V2.md) layouts
are superseded — see PROTOCOL-V2.1.md section 6.2.

Conventions: all integers little-endian. CRC16-CCITT-FALSE (poly 0x1021,
init 0xFFFF), always last, over every byte before the CRC field.
"""
import hmac, hashlib, struct

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

# ---- v2.1 data packet (board -> PC, UDP 4211) --------------------------------
# Layout (PROTOCOL-V2.1.md 1.1): "RSH2" | ver 0x21 | flags | seq u16 | ts u32 |
#   axisMask | nEnc | buttons u32 | rsv u16=0 | axes u16 x N (mV) |
#   encoders i16 x nEnc (cumulative) | CRC16
def data_v2_1(seq, ts_us, axes, buttons, enc, flags):
    """axes: dict {axis_index(0-7): u16 mV}; enc: list of i16 cumulative counts."""
    assert len(enc) <= 4 and len(axes) <= 8
    mask = 0
    for i in axes: mask |= 1 << i
    body = struct.pack("<4sBBHIBBIH", b"RSH2", 0x21, flags, seq, ts_us,
                       mask, len(enc), buttons, 0)
    for i in sorted(axes): body += struct.pack("<H", axes[i])
    for e in enc: body += struct.pack("<h", e)
    return with_crc(body)

# ---- v2.1 status packet (board -> PC, 1 Hz, UDP 4211, magic 0xA6) -----------
# Base 34 B (PROTOCOL-V2.1.md 1.2); 38 B extended when btns_ex is given
# (buttons 32..63). seqGap/txFail sit before the trailing CRC16.
def status_v2_1(seq, uptime_ms, rssi, batt_pct, supply_mv, heap, overruns,
                max_cycle_us, gov, reset_reason, cfg_crc, cfg_ver,
                seq_gap, tx_fail, btns_ex=None):
    body = struct.pack("<BBHIbBHIHHBBIH", 0xA6, 0x21, seq, uptime_ms, rssi, batt_pct,
                       supply_mv, heap, overruns, max_cycle_us, gov, reset_reason,
                       cfg_crc, cfg_ver)
    if btns_ex is not None:
        body += struct.pack("<I", btns_ex)
    body += struct.pack("<HH", seq_gap, tx_fail)
    return with_crc(body)

# ---- HELLO reply (board -> PC, UDP 4211) — 92 B fixed -------------------------
# Layout (PROTOCOL-V2.1.md 2.2): "RSHR" | proto 2.1 | flags | sessionEcho |
#   boardMac 6B | fw 4.0.0 | rsv | nonce 16B | deviceKey 32B | wifiPassword 24B | CRC16
# deviceKey/wifiPassword are all-zero unless unpaired AND pairing window open.
def hello_reply(flags, session_echo, board_mac, nonce, device_key=b"\x00" * 32,
                wifi_password=b"\x00" * 24):
    assert len(board_mac) == 6 and len(nonce) == 16
    assert len(device_key) == 32 and len(wifi_password) == 24
    body = struct.pack("<4sBBBB", b"RSHR", 2, 1, flags, session_echo) \
        + board_mac + struct.pack("<BBBB", 4, 0, 0, 0) + nonce + device_key + wifi_password
    return with_crc(body)

# ---- control frame (PC <-> board, UDP 4212) ----------------------------------
# Layout (PROTOCOL-V2.1.md 3.1): "RSHC" | cmd | reqId u16 | counter u32 |
#   len u16 | payload | tag[16]
# tag = HMAC-SHA256(key32, nonce || counter_LE || cmd || payload)[0:16]
def ctrl_frame(key: bytes, nonce: bytes, cmd: int, req_id: int, counter: int,
               payload: bytes = b"") -> bytes:
    assert len(key) == 32 and len(nonce) == 16 and len(payload) <= 1024
    head = struct.pack("<4sBHIH", b"RSHC", cmd, req_id, counter, len(payload))
    tag = hmac.new(key, nonce + struct.pack("<I", counter) + bytes([cmd]) + payload,
                   hashlib.sha256).digest()[:16]
    return head + payload + tag

def ctrl_verify(key: bytes, nonce: bytes, frame: bytes) -> bool:
    """Recompute the tag over a received frame; False on any mismatch/truncation."""
    if len(frame) < 13 + 16 or frame[:4] != b"RSHC":
        return False
    cmd, req_id, counter, ln = struct.unpack("<BHIH", frame[4:13])
    if len(frame) != 13 + ln + 16:
        return False
    payload = frame[13:13 + ln]
    expect = hmac.new(key, nonce + struct.pack("<I", counter) + bytes([cmd]) + payload,
                      hashlib.sha256).digest()[:16]
    return hmac.compare_digest(expect, frame[13 + ln:])

def ctrl_reply(key: bytes, nonce: bytes, req_cmd: int, req_id: int, counter: int,
               status: int, body: bytes = b"") -> bytes:
    return ctrl_frame(key, nonce, req_cmd | 0x80, req_id, counter,
                      bytes([status]) + body)

# ---- deterministic test fixtures (NOT real secrets) --------------------------
TEST_KEY = bytes(range(32))                    # 00 01 ... 1f
TEST_NONCE = bytes(0xA0 + i for i in range(16))  # a0 a1 ... af
WRONG_KEY = bytes([0xFF]) * 32
TEST_MAC = bytes.fromhex("aabbccddeeff")
TEST_PW = b"Ab3dEf6hJk9mNp2rSt5vWx8Y"          # 24 chars, A-Z a-z 2-9

# ---- serial frame (§10, Phase G WS1) -----------------------------------------
# Envelope: "RSHS" | kind u8 | len u16 LE | payload | CRC16 (§0, frame ke
# CRC field se pehle ke tamam bytes par). Kind 1..7, max payload 1053 B.
def serial_frame(kind: int, payload: bytes) -> bytes:
    """Serial frame banao (§10). Payload wahi bytes jo UDP par jate."""
    assert 1 <= kind <= 7 and len(payload) <= 1053
    body = b"RSHS" + struct.pack("<BH", kind, len(payload)) + payload
    return with_crc(body)

VECTORS = {
  # stream v2.1: seq 0x1234, ts 0x01020304, axes 0-4 mV, buttons 0x205,
  # enc cumulative +1000,-250, flags 0x01 (config-changed)
  "data_v2_1": data_v2_1(0x1234, 0x01020304, {0: 2300, 1: 1000, 2: 2000, 3: 0, 4: 512},
                         0x00000205, [1000, -250], 0x01),
  # status v2.1 base 34 B: seqGap 2, txFail 1
  "status_v2_1": status_v2_1(7, 123456, -52, 86, 4012, 187000, 3, 2140, 0, 1,
                             0xDEADBEEF, 5, 2, 1),
  # status v2.1 extended 38 B: btnsEx = buttons 32+5 set (bit 5)
  "status_v2_1_ext": status_v2_1(8, 123457, -50, 86, 4010, 187000, 3, 2140, 0, 1,
                                 0xDEADBEEF, 5, 2, 1, btns_ex=0x00000020),
  # HELLO reply, unpaired + window open (flags 0b00001001 = unpaired|window),
  # new session echo 1, key + password present
  "hello_unpaired": hello_reply(0x09, 1, TEST_MAC, TEST_NONCE, TEST_KEY, TEST_PW),
  # HELLO reply, paired (flags 0), key/password zero
  "hello_paired": hello_reply(0x00, 0, TEST_MAC, TEST_NONCE),
  # control: GET_CAPS, reqId 1, counter 0, empty payload
  "ctrl_get_caps": ctrl_frame(TEST_KEY, TEST_NONCE, 0x01, 1, 0, b""),
  # control: SET_CFG chunk, reqId 2, counter 1 (offset 0, total 64, 16 B data)
  "ctrl_set_cfg": ctrl_frame(TEST_KEY, TEST_NONCE, 0x03, 2, 1,
                             struct.pack("<HHHH", 0, 64, 16, crc16(b"RSHG" + bytes(12)))
                             + b"RSHG" + bytes(12)),
  # control reply: GET_CAPS OK, reqId 1, counter 0
  "ctrl_get_caps_reply": ctrl_reply(TEST_KEY, TEST_NONCE, 0x01, 1, 0, 0,
                                    TEST_MAC + bytes([4, 0, 0, 2, 1, 8, 64, 4, 6, 0, 0])),
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
  # tag byte flipped -> BAD_TAG
  "neg_bad_tag": _flip(_good, len(_good) - 1),
  # same frame bytes, verified against a different key -> BAD_TAG (wrong key)
  "neg_wrong_key_frame": _good,
  # truncated: last 8 bytes cut off
  "neg_truncated": _good[:-8],
  # replay: counter 0 reused with a NEW reqId (counter <= max-seen) — the
  # frame itself verifies (tag is valid); the REPLAY reject is a
  # counter-state rule, checked by the receiver, not by the tag.
  "neg_replay_same_counter_new_reqid": ctrl_frame(TEST_KEY, TEST_NONCE, 0x01, 99, 0, b""),
  # duplicate: byte-identical retransmit of the good frame (same reqId+counter)
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
    d = VECTORS["data_v2_1"]; assert len(d) == 20 + 2*5 + 2*2 + 2
    assert crc16(d[:-2]) == struct.unpack("<H", d[-2:])[0]
    assert d[:4] == b"RSH2" and d[4] == 0x21
    s = VECTORS["status_v2_1"]; assert len(s) == 34
    assert crc16(s[:-2]) == struct.unpack("<H", s[-2:])[0]
    assert struct.unpack("<H", s[28:30])[0] == 2 and struct.unpack("<H", s[30:32])[0] == 1
    sx = VECTORS["status_v2_1_ext"]; assert len(sx) == 38
    assert struct.unpack("<I", sx[28:32])[0] == 0x20
    assert crc16(sx[:-2]) == struct.unpack("<H", sx[-2:])[0]
    h = VECTORS["hello_unpaired"]; assert len(h) == 92
    assert crc16(h[:-2]) == struct.unpack("<H", h[-2:])[0]
    assert h[34:66] == TEST_KEY and h[66:90] == TEST_PW
    hp = VECTORS["hello_paired"]; assert len(hp) == 92 and hp[34:90] == b"\x00" * 56
    c = VECTORS["ctrl_get_caps"]; assert len(c) == 13 + 16
    assert ctrl_verify(TEST_KEY, TEST_NONCE, c), "good control frame must verify"
    r = VECTORS["ctrl_get_caps_reply"]
    assert ctrl_verify(TEST_KEY, TEST_NONCE, r) and r[4] == (0x01 | 0x80)
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
    assert not ctrl_verify(TEST_KEY, TEST_NONCE, NEGATIVE["neg_bad_tag"]), "bad tag must fail"
    assert not ctrl_verify(WRONG_KEY, TEST_NONCE, NEGATIVE["neg_wrong_key_frame"]), \
        "wrong key must fail"
    assert not ctrl_verify(TEST_KEY, TEST_NONCE, NEGATIVE["neg_truncated"]), \
        "truncated must fail"
    assert not ctrl_verify(TEST_KEY, TEST_NONCE, NEGATIVE["neg_bad_magic"]), \
        "bad magic must fail"
    # replay + duplicate are state rules, not tag rules: the frames verify
    # cryptographically, the receiver must reject/replay them by counter state.
    assert ctrl_verify(TEST_KEY, TEST_NONCE, NEGATIVE["neg_replay_same_counter_new_reqid"])
    assert ctrl_verify(TEST_KEY, TEST_NONCE, NEGATIVE["neg_duplicate_exact"])
    print("OK: v1 golden CRC matches; v2.1 vectors self-consistent; "
          "negative vectors fail as expected (tag/state rules)")
