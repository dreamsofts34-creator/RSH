#!/usr/bin/env python3
"""Generate the recorded shared-memory fixtures for rsh-telemetry tests.

Regenerate with:  python3 generate_fixtures.py   (run in this directory)

Format (full layout reference — offsets are listed below):
- ac_static.bin (1024 B): Kunos static block; carModel "ferrari_488_gt3"
  (UTF-16LE @ 68), track "monza" (@ 134), smVersion "1.4", acVersion
  "1.16.4", numberOfSessions 1 (@ 60), numCars 20 (@ 64).
- ac_graphics.bin (512 B): Kunos graphics block; packetId 1 (@ 0),
  status LIVE (2) (@ 4), session RACE (2) (@ 8).
- ac_graphics_menu.bin (512 B): same, but status OFF (0) — menu state.
- ac_static_short.bin (100 B): truncated static block — the reader must
  reject it (minimum size is 200 B).
- acc_static.bin (1024 B): same layout as AC; carModel
  "Ferrari 488 GT3 Evo" (@ 68), track "spa" (@ 134), smVersion "1.8",
  acVersion "1.10.4".
- acc_graphics.bin (512 B): status LIVE (2).
- iracing_shm.bin (~1 KB, NOT the real 780 KiB map): minimal valid
  iRacing image — 112 B header (ver 2, status connected, 1 var,
  1 buffer) + 144 B var header (SessionState, int, offset 0) + 64 B
  data buffer (SessionState = 4 racing, tickCount 1234) + session YAML
  (DriverCarIdx 0 -> Ferrari 488 GT3 Evo 2020, track Spa-Francorchamps,
  plus a second driver as a distractor).

These are synthetic fixtures built from the published layouts, not
captures from a live game (no game was running in the sandbox). A live
capture on the owner PC should replace/extend them in Phase F live
verification.
"""

import struct
from pathlib import Path

HERE = Path(__file__).parent


def put_wstr(buf: bytearray, offset: int, text: str) -> None:
    raw = text.encode("utf-16-le")
    buf[offset:offset + len(raw)] = raw


def put_i32(buf: bytearray, offset: int, value: int) -> None:
    buf[offset:offset + 4] = struct.pack("<i", value)


def kunos_static(car: str, track: str, sm_version: str, ac_version: str) -> bytearray:
    buf = bytearray(1024)
    put_wstr(buf, 0, sm_version)
    put_wstr(buf, 30, ac_version)
    put_i32(buf, 60, 1)   # numberOfSessions
    put_i32(buf, 64, 20)  # numCars
    put_wstr(buf, 68, car)
    put_wstr(buf, 134, track)
    return buf


def kunos_graphics(status: int) -> bytearray:
    buf = bytearray(512)
    put_i32(buf, 0, 1)       # packetId
    put_i32(buf, 4, status)  # AC_STATUS
    put_i32(buf, 8, 2)       # AC_SESSION_TYPE: RACE
    return buf


IRACING_YAML = (
    "WeekendInfo:\n"
    "  TrackName: Spa-Francorchamps\n"
    "DriverInfo:\n"
    "  DriverCarIdx: 0\n"
    "  Drivers:\n"
    "  - CarIdx: 0\n"
    "    UserName: Test Driver\n"
    "    CarPath: ferrari488gt3\n"
    "    CarScreenName: Ferrari 488 GT3 Evo 2020\n"
    "  - CarIdx: 1\n"
    "    UserName: Rival Driver\n"
    "    CarPath: porsche991gt3\n"
    "    CarScreenName: Porsche 911 GT3 R\n"
)


def iracing_image(session_state: int) -> bytearray:
    yaml = IRACING_YAML.encode("ascii")
    var_header_offset = 112
    buf_offset = var_header_offset + 144
    buf_len = 64
    yaml_offset = buf_offset + buf_len
    buf = bytearray(yaml_offset + len(yaml))
    put_i32(buf, 0, 2)                    # ver
    put_i32(buf, 4, 1)                    # status: connected
    put_i32(buf, 8, 60)                   # tickRate
    put_i32(buf, 12, 7)                   # sessionInfoUpdate
    put_i32(buf, 16, len(yaml))           # sessionInfoLen
    put_i32(buf, 20, yaml_offset)         # sessionInfoOffset
    put_i32(buf, 24, 1)                   # numVars
    put_i32(buf, 28, var_header_offset)   # varHeaderOffset
    put_i32(buf, 32, 1)                   # numBuf
    put_i32(buf, 36, buf_len)             # bufLen
    put_i32(buf, 48, 1234)                # varBuf[0].tickCount
    put_i32(buf, 52, buf_offset)          # varBuf[0].bufOffset
    vh = var_header_offset
    put_i32(buf, vh, 2)                   # type: int
    put_i32(buf, vh + 4, 0)               # offset
    put_i32(buf, vh + 8, 1)               # count
    buf[vh + 16:vh + 16 + len(b"SessionState")] = b"SessionState"
    put_i32(buf, buf_offset, session_state)
    buf[yaml_offset:] = yaml
    return buf


def main() -> None:
    (HERE / "ac_static.bin").write_bytes(kunos_static("ferrari_488_gt3", "monza", "1.4", "1.16.4"))
    (HERE / "ac_graphics.bin").write_bytes(kunos_graphics(2))
    (HERE / "ac_graphics_menu.bin").write_bytes(kunos_graphics(0))
    (HERE / "ac_static_short.bin").write_bytes(kunos_static("ferrari_488_gt3", "monza", "1.4", "1.16.4")[:100])
    (HERE / "acc_static.bin").write_bytes(kunos_static("Ferrari 488 GT3 Evo", "spa", "1.8", "1.10.4"))
    (HERE / "acc_graphics.bin").write_bytes(kunos_graphics(2))
    (HERE / "iracing_shm.bin").write_bytes(iracing_image(4))
    print("fixtures written to", HERE)


if __name__ == "__main__":
    main()
