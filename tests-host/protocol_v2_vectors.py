"""RSH protocol v2 — reference encoder + golden vectors (S0/F0 contract).
Run:  python3 -I tests-host/protocol_v2_vectors.py
C++ (firmware) and C# (app) implementations must reproduce these exact bytes.
"""
import hmac, hashlib, struct

def crc16(data: bytes) -> int:           # CRC16-CCITT, poly 0x1021, init 0xFFFF
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

# ---- v2 data packet (board -> PC, UDP 4211) ----------------------------------
def data_v2(seq, ts_us, axes, buttons, enc, flags):
    """axes: dict {axis_index(0-7): u16}; enc: list of i8 deltas."""
    mask = 0
    for i in axes: mask |= 1 << i
    body = struct.pack("<BBHIBBIBB", 0xA5, 2, seq, ts_us, mask, flags, buttons, len(enc), 0)
    for i in sorted(axes): body += struct.pack("<H", axes[i])
    for d in enc: body += struct.pack("<b", d)
    return with_crc(body)

# ---- v2 status packet (board -> PC, 1 Hz, UDP 4211, magic 0xA6) -------------
def status_v2(seq, uptime_ms, rssi, batt_pct, supply_mv, heap, overruns, max_cycle_us,
              gov, reset_reason, cfg_crc, cfg_ver):
    body = struct.pack("<BBHIbBHIHHBBIH", 0xA6, 2, seq, uptime_ms, rssi, batt_pct, supply_mv,
                       heap, overruns, max_cycle_us, gov, reset_reason, cfg_crc, cfg_ver)
    return with_crc(body)

# ---- control frame (PC <-> board, UDP 4212) ----------------------------------
def ctrl(password: str, cmd: int, req_id: int, payload: bytes = b"") -> bytes:
    key = hashlib.sha256(password.encode()).digest()
    head = struct.pack("<BBBHH", 0xC7, 1, cmd, req_id, len(payload))
    tag = hmac.new(key, head + payload, hashlib.sha256).digest()[:8]
    return head + payload + tag

VECTORS = {
  "data_v2": data_v2(0x1234, 0x01020304, {0: 2300, 1: 1000, 2: 2000, 3: 0, 4: 512},
                     0x00000205, [1, -1], 0x01),
  "status_v2": status_v2(7, 123456, -52, 86, 4012, 187000, 3, 2140, 0, 1, 0xDEADBEEF, 5),
  "ctrl_get_caps": ctrl("test", 0x01, 1),
}
if __name__ == "__main__":
    for k, v in VECTORS.items():
        print(f"{k:14s} len={len(v):3d}  {v.hex(' ')}")
    # structural checks
    d = VECTORS["data_v2"]; assert len(d) == 16 + 2*5 + 2 + 2
    assert crc16(d[:-2]) == struct.unpack("<H", d[-2:])[0]
    s = VECTORS["status_v2"]; assert len(s) == 30
    assert crc16(s[:-2]) == struct.unpack("<H", s[-2:])[0]
    assert len(VECTORS["ctrl_get_caps"]) == 7 + 8
    print("OK: v1 golden CRC matches; v2 vectors self-consistent")
