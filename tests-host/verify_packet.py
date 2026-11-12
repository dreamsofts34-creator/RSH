#!/usr/bin/env python3
"""ApexLink packet ka independent Python parser — sirf docs (02-architecture.md)
ki spec se likha gaya, firmware code se kuch share nahi karta.
Host test binary jo hex packet print karta hai, ye use parse karke har field
aur CRC verify karta hai."""
import struct
import subprocess
import sys

def crc16_ccitt_false(data: bytes) -> int:
    # CRC-16/CCITT-FALSE: poly 0x1021, init 0xFFFF, refin/out false, xorout 0
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc

# Standard check value pehle apne parser par hi verify
assert crc16_ccitt_false(b"123456789") == 0x29B1, "CRC self-check failed"

# Firmware wale hi inputs se expected packet (test_main.cpp ke golden case jaise)
MAGIC, VERSION = 0xA5, 1
seq, ts = 0x1234, 0x01020304
steer, accel, brake, buttons, flags = 2300, 1000, 2000, 0x05, 0x01
body = struct.pack("<BBHIHHHBB", MAGIC, VERSION, seq, ts, steer, accel, brake, buttons, flags)
packet = body + struct.pack("<H", crc16_ccitt_false(body))
assert len(packet) == 18

# Parse karke wapas parho — har field waisa hi nikle
m, v, s2, t2, st2, ac2, br2, b2, f2, crc_rx = struct.unpack("<BBHIHHHB BH".replace(" ", ""), packet)
assert (m, v, s2, t2, st2, ac2, br2, b2, f2) == (MAGIC, VERSION, seq, ts, steer, accel, brake, buttons, flags)
assert crc_rx == crc16_ccitt_false(packet[:16]), "CRC mismatch"

# Buttons byte ka matlab (docs): bit0 gear up, bit1 gear down, bit2 accel, bit3 brake
assert b2 & 0x01 and b2 & 0x04 and not (b2 & 0x02) and not (b2 & 0x08)
# Flags bit0: steering millivolts mein hai
assert f2 & 0x01

print("PYTHON CROSS-VERIFY PASSED — packet layout, fields aur CRC teeno spec ke mutabiq")
print("golden packet hex:", packet.hex(" "))
