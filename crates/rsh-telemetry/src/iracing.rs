//! iRacing shared-memory reader (official SDK layout).
//!
//! Layout source: the iRacing SDK (`irsdk_defines.h`) as implemented by
//! the community `pyirsdk` 1.1.7 port, cross-checked against
//! `TELEMETRY-READER.md` §2:
//!
//! * Map name: `Local\IRSDKMemMapFileName`, fixed size 780 KiB.
//! * Header (112 B): `ver` @ 0, `status` @ 4 (bit 0 = connected),
//!   `tickRate` @ 8, `sessionInfoUpdate` @ 12, `sessionInfoLen` @ 16,
//!   `sessionInfoOffset` @ 20, `numVars` @ 24, `varHeaderOffset` @ 28,
//!   `numBuf` @ 32, `bufLen` @ 36, `pad[2]` @ 40, `varBuf[4]` @ 48 —
//!   each `varBuf` 16 B (`tickCount` @ +0, `bufOffset` @ +4, pad @ +8).
//! * Var header (144 B): `type` @ 0, `offset` @ 4, `count` @ 8,
//!   `countAsTime` @ 12, `name[32]` @ 16, `desc[64]` @ 48, `unit[32]` @
//!   112. Var types: 0 char, 1 bool, 2 int, 3 bitfield, 4 float,
//!   5 double.
//! * Session state comes from the `SessionState` telemetry var
//!   (0 invalid, 1 get-in-car, 2 warmup, 3 parade laps, 4 racing,
//!   5 checkered, 6 cool down). Car identity comes from the session
//!   YAML: `DriverInfo → DriverCarIdx` selects the entry in
//!   `DriverInfo → Drivers` (matched on `CarIdx`); the car name is that
//!   entry's `CarScreenName`, falling back to `CarPath`. Track is
//!   `WeekendInfo → TrackName`.
//!
//! Bounds (SEC-H-05c): session YAML ≤ 256 KiB, `numVars` ≤ 10000,
//! `numBuf` 1..=4, every offset/length pair is checked against the real
//! map size before any read. The header is self-describing, so layout
//! drift is a non-issue here — the one place the C# doc calls safest.

use crate::access::{MapAccessor, read_consistent, read_i32};
use crate::types::{GameId, SessionState, TelemetrySnapshot};

/// Name of the iRacing shared-memory map (SDK: `Local\IRSDKMemMapFileName`).
pub const MAP_NAME: &str = r"Local\IRSDKMemMapFileName";
/// Fixed size of the iRacing shared-memory map (SDK: 780 KiB).
pub const MAP_SIZE: usize = 780 * 1024;
/// Maximum session-info (YAML) length accepted (SEC-H-05c).
pub const MAX_SESSION_INFO_BYTES: usize = 256 * 1024;
/// Same bound as an `i32`, for comparing against header fields.
const MAX_SESSION_INFO_BYTES_I32: i32 = 256 * 1024;
/// Maximum variable count accepted (SEC-H-05c).
pub const MAX_VARS: i32 = 10_000;
/// Maximum buffer count accepted (SEC-H-05c; SDK uses up to 4).
pub const MAX_BUFFERS: i32 = 4;
/// Maximum single telemetry buffer length accepted (real buffers are
/// tens of KiB; the whole map is 780 KiB).
pub const MAX_BUFFER_LEN: i32 = 1024 * 1024;

/// Size of the iRacing header in bytes (112).
pub const HEADER_LEN: usize = 112;
/// Byte offset of the `varBuf[4]` array inside the header.
pub const VAR_BUF_ARRAY_OFFSET: usize = 48;
/// Size of one `varBuf` entry in bytes (16).
pub const VAR_BUF_LEN: usize = 16;
/// Size of one variable header in bytes (144).
pub const VAR_HEADER_LEN: usize = 144;
/// Same size as an `i32`, for `checked_mul` against `num_vars` (avoids
/// a narrowing cast; kept in sync by the layout test).
const VAR_HEADER_LEN_I32: i32 = 144;

/// `status` bit 0: the sim is connected (SDK `StatusField.status_connected`).
pub const STATUS_CONNECTED: i32 = 1;

/// Variable type code for a 32-bit int (SDK `irsdk_VarType`).
pub const VAR_TYPE_INT: i32 = 2;

/// A `repr(C)` mirror of the SDK `irsdk_header`, used only for layout
/// asserts (see tests): parsing itself reads fields byte-by-byte from
/// the accessor, never by casting a byte slice to this struct.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HeaderRaw {
    /// SDK version (currently 2).
    pub ver: i32,
    /// Status bitfield; bit 0 = connected.
    pub status: i32,
    /// Telemetry tick rate (Hz).
    pub tick_rate: i32,
    /// Session-info update counter.
    pub session_info_update: i32,
    /// Session-info (YAML) length in bytes.
    pub session_info_len: i32,
    /// Session-info (YAML) offset in the map.
    pub session_info_offset: i32,
    /// Number of variable headers.
    pub num_vars: i32,
    /// Offset of the variable-header array.
    pub var_header_offset: i32,
    /// Number of data buffers (1..=4).
    pub num_buf: i32,
    /// Length of one data buffer in bytes.
    pub buf_len: i32,
    /// Padding.
    pub pad: [i32; 2],
    /// Per-buffer tick count + offset.
    pub var_buf: [VarBufRaw; 4],
}

/// A `repr(C)` mirror of one SDK `irsdk_varBuf` entry (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VarBufRaw {
    /// Tick count of the data in this buffer.
    pub tick_count: i32,
    /// Offset of this buffer's data in the map.
    pub buf_offset: i32,
    /// Padding.
    pub pad: [i32; 2],
}

/// A `repr(C)` mirror of the SDK `irsdk_varHeader` (144 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VarHeaderRaw {
    /// Variable type code (0 char, 1 bool, 2 int, 3 bitfield, 4 float, 5 double).
    pub var_type: i32,
    /// Byte offset of the variable inside a data buffer.
    pub offset: i32,
    /// Value count (1 for scalars).
    pub count: i32,
    /// Whether the value is a time.
    pub count_as_time: u8,
    /// Padding.
    pub pad: [u8; 3],
    /// Variable name, NUL-terminated ASCII.
    pub name: [u8; 32],
    /// Description, NUL-terminated ASCII.
    pub desc: [u8; 64],
    /// Unit, NUL-terminated ASCII.
    pub unit: [u8; 32],
}

/// A validated iRacing header: every offset/length already proven to be
/// inside the map (SEC-H-05c).
#[derive(Clone, Debug)]
struct Header {
    session_info_len: usize,
    session_info_offset: usize,
    num_vars: i32,
    var_header_offset: usize,
    num_buf: i32,
    /// Length of one data buffer in bytes (validated 1..=`MAX_BUFFER_LEN`).
    buf_len: i32,
}

/// One data buffer's live position.
#[derive(Clone, Copy, Debug)]
struct Buffer {
    /// Index into the header's `varBuf` array.
    index: usize,
    /// Tick count read for this buffer.
    tick_count: i32,
    /// Offset of this buffer's data in the map.
    buf_offset: usize,
}

/// Validates an `[offset, offset + len)` range from untrusted header
/// fields: both must be non-negative and the end must fit in the map.
fn checked_range(offset: i32, len: i32, map_len: usize) -> Option<(usize, usize)> {
    if offset < 0 || len < 0 {
        return None;
    }
    let start = usize::try_from(offset).ok()?;
    let len = usize::try_from(len).ok()?;
    let end = start.checked_add(len)?;
    if end > map_len {
        return None;
    }
    Some((start, end))
}

/// Parses and validates the iRacing header (double-read, SEC-H-05a).
fn parse_header(map: &dyn MapAccessor) -> Option<Header> {
    let bytes = read_consistent(map, 0, HEADER_LEN)?;
    let get = |offset: usize| -> Option<i32> {
        let arr: [u8; 4] = bytes[offset..offset + 4].try_into().ok()?;
        Some(i32::from_le_bytes(arr))
    };
    let ver = get(0)?;
    let status = get(4)?;
    let session_info_len = get(16)?;
    let session_info_offset = get(20)?;
    let num_vars = get(24)?;
    let var_header_offset = get(28)?;
    let num_buf = get(32)?;
    let buf_len = get(36)?;
    if ver < 1 || status & STATUS_CONNECTED == 0 {
        return None;
    }
    if !(0..=MAX_VARS).contains(&num_vars)
        || !(1..=MAX_BUFFERS).contains(&num_buf)
        || !(1..=MAX_BUFFER_LEN).contains(&buf_len)
    {
        return None;
    }
    if !(0..=MAX_SESSION_INFO_BYTES_I32).contains(&session_info_len) {
        return None;
    }
    let (session_info_offset, _) = checked_range(session_info_offset, session_info_len, map.len())?;
    let var_headers_len = num_vars.checked_mul(VAR_HEADER_LEN_I32)?;
    let (var_header_offset, _) = checked_range(var_header_offset, var_headers_len, map.len())?;
    Some(Header {
        session_info_len: usize::try_from(session_info_len).ok()?,
        session_info_offset,
        num_vars,
        var_header_offset,
        num_buf,
        buf_len,
    })
}

/// Reads the tick counts of all buffers and returns the newest buffer.
///
/// "Newest" uses wrapping subtraction, so it stays correct when the
/// `i32` tick counter wraps (buffers are written ~60 Hz, so live
/// buffers are always within a few ticks of each other).
fn latest_buffer(map: &dyn MapAccessor, header: &Header) -> Option<Buffer> {
    let mut best: Option<Buffer> = None;
    for index in 0..usize::try_from(header.num_buf).ok()? {
        let tick_offset = VAR_BUF_ARRAY_OFFSET + index * VAR_BUF_LEN;
        let tick_count = read_i32(map, tick_offset)?;
        let buf_offset = read_i32(map, tick_offset + 4)?;
        let (buf_offset, _) = checked_range(buf_offset, header.buf_len, map.len())?;
        let candidate = Buffer {
            index,
            tick_count,
            buf_offset,
        };
        let is_newer = match best {
            None => true,
            Some(current) => tick_count.wrapping_sub(current.tick_count) > 0,
        };
        if is_newer {
            best = Some(candidate);
        }
    }
    best
}

/// Finds the `SessionState` variable header: `(offset_in_buffer,)`.
fn find_session_state_var(map: &dyn MapAccessor, header: &Header) -> Option<i32> {
    for index in 0..header.num_vars {
        let offset = header
            .var_header_offset
            .checked_add(usize::try_from(index).ok()?.checked_mul(VAR_HEADER_LEN)?)?;
        let bytes = read_consistent(map, offset, VAR_HEADER_LEN)?;
        let var_type = i32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let var_offset = i32::from_le_bytes(bytes[4..8].try_into().ok()?);
        let count = i32::from_le_bytes(bytes[8..12].try_into().ok()?);
        let name = parse_ascii_name(&bytes[16..48]);
        if name == "SessionState" {
            if var_type != VAR_TYPE_INT || count < 1 {
                return None;
            }
            if var_offset < 0 || var_offset.checked_add(4)? > header.buf_len {
                return None;
            }
            return Some(var_offset);
        }
    }
    None
}

/// Reads one `i32` telemetry variable from the newest buffer, verifying
/// the buffer's `tickCount` before and after the copy (SEC-H-05a):
/// a changed tick means the sim rewrote the buffer mid-copy. Retried
/// once, then `None`.
fn read_latest_var_i32(map: &dyn MapAccessor, header: &Header, var_offset: i32) -> Option<i32> {
    for _ in 0..2 {
        let buffer = latest_buffer(map, header)?;
        let value_offset = buffer
            .buf_offset
            .checked_add(usize::try_from(var_offset).ok()?)?;
        let value = read_i32(map, value_offset)?;
        let tick_after = read_i32(map, VAR_BUF_ARRAY_OFFSET + buffer.index * VAR_BUF_LEN)?;
        if tick_after == buffer.tick_count {
            return Some(value);
        }
    }
    None
}

/// Reads the `SessionState` variable; `None` when the variable is
/// absent (state then stays [`SessionState::Unknown`]).
fn read_session_state(map: &dyn MapAccessor, header: &Header) -> Option<SessionState> {
    let var_offset = find_session_state_var(map, header)?;
    let raw = read_latest_var_i32(map, header, var_offset)?;
    Some(session_state_from_raw(raw))
}

/// Maps a raw `irsdk_SessionState` value to a [`SessionState`].
///
/// Parade laps, racing, checkered and cool-down all mean the car is on
/// track (a profile switch there is wanted); get-in-car and warmup are
/// still loading.
#[must_use]
pub const fn session_state_from_raw(raw: i32) -> SessionState {
    match raw {
        1 | 2 => SessionState::Loading,
        3..=6 => SessionState::OnTrack,
        _ => SessionState::Unknown,
    }
}

/// NUL-terminated ASCII field (var-header `name`): bytes up to the first
/// NUL, lossy-decoded (names are ASCII by spec; non-ASCII bytes become
/// U+FFFD and simply never match a wanted name).
fn parse_ascii_name(field: &[u8]) -> String {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim().to_string()
}

/// Splits a `Key: Value` YAML line; returns `None` for lines without a
/// colon. The value is trimmed and unquoted.
fn split_key_value(line: &str) -> Option<(&str, String)> {
    let (key, value) = line.split_once(':')?;
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value);
    Some((key.trim(), value.trim().to_string()))
}

/// One entry of the session YAML's `DriverInfo → Drivers` list.
#[derive(Default)]
struct DriverEntry {
    idx: Option<i32>,
    path: String,
    screen_name: String,
}

/// Parses the session-info YAML for car + track.
///
/// Returns `(car_display, track_name)`: the player car's
/// `CarScreenName` (falling back to `CarPath`), and
/// `WeekendInfo → TrackName`. The player is the `Drivers` entry whose
/// `CarIdx` equals `DriverCarIdx` (first entry when that key is absent).
/// Returns `None` when no driver entry yields a car name. This is a
/// small line parser, not a YAML library: the session info is flat
/// `Key: Value` lines, and adding a YAML dependency for it would pull a
/// whole parser into a fail-safe reader.
fn parse_session_yaml(text: &str) -> Option<(String, String)> {
    let mut section = "";
    let mut in_drivers = false;
    let mut driver_car_idx: Option<i32> = None;
    let mut track_name = String::new();
    let mut entries: Vec<DriverEntry> = Vec::new();
    let mut current: Option<DriverEntry> = None;

    // Flushes the in-progress entry into `entries`.
    let flush = |current: &mut Option<DriverEntry>, entries: &mut Vec<DriverEntry>| {
        if let Some(entry) = current.take() {
            entries.push(entry);
        }
    };

    for line in text.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let stripped = line.trim_start_matches(' ');
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if indent == 0 && stripped.ends_with(':') {
            flush(&mut current, &mut entries);
            section = stripped.trim_end_matches(':');
            in_drivers = false;
            continue;
        }
        if section == "DriverInfo" {
            if let Some((key, value)) = split_key_value(stripped) {
                if key == "DriverCarIdx" && !stripped.starts_with('-') {
                    driver_car_idx = value.parse::<i32>().ok();
                    continue;
                }
                if key == "Drivers" && value.is_empty() {
                    in_drivers = true;
                    continue;
                }
            }
            if in_drivers && stripped.starts_with("- CarIdx:") {
                flush(&mut current, &mut entries);
                let mut entry = DriverEntry::default();
                if let Some((_, value)) = split_key_value(stripped.trim_start_matches('-')) {
                    entry.idx = value.parse::<i32>().ok();
                }
                current = Some(entry);
                continue;
            }
            if in_drivers && indent >= 4 {
                if let Some(entry) = current.as_mut()
                    && let Some((key, value)) = split_key_value(stripped)
                {
                    match key {
                        "CarPath" => entry.path = value,
                        "CarScreenName" => entry.screen_name = value,
                        _ => {}
                    }
                }
                continue;
            }
        }
        if section == "WeekendInfo"
            && let Some((key, value)) = split_key_value(stripped)
            && key == "TrackName"
        {
            track_name = value;
        }
    }
    flush(&mut current, &mut entries);

    let wanted = driver_car_idx;
    let entry = wanted
        .and_then(|idx| entries.iter().find(|e| e.idx == Some(idx)))
        .or_else(|| entries.first())?;
    let car = if entry.screen_name.is_empty() {
        entry.path.clone()
    } else {
        entry.screen_name.clone()
    };
    if car.trim().is_empty() {
        return None;
    }
    Some((car, track_name))
}

/// Reads one iRacing identity snapshot from a map accessor.
///
/// Pure and platform-independent: on Windows the caller passes the live
/// mapped view; in tests it passes [`SliceMap`](crate::access::SliceMap)
/// over a recorded fixture image. Returns `None` when the sim is not
/// connected, the header fails validation, the YAML is missing/oversize,
/// or no car name resolves — never panics.
#[must_use]
pub fn read_iracing_snapshot(map: &dyn MapAccessor) -> Option<TelemetrySnapshot> {
    let header = parse_header(map)?;
    let yaml_bytes = read_consistent(map, header.session_info_offset, header.session_info_len)?;
    let yaml_end = yaml_bytes
        .iter()
        .rposition(|b| *b != 0)
        .map_or(0, |last| last + 1);
    let yaml_text = String::from_utf8_lossy(&yaml_bytes[..yaml_end]).into_owned();
    let (car, track) = parse_session_yaml(&yaml_text)?;
    let state = read_session_state(map, &header).unwrap_or(SessionState::Unknown);
    Some(TelemetrySnapshot::new(GameId::IRacing, &car, &track, state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::SliceMap;
    use std::mem::{offset_of, size_of};

    const YAML: &str = "WeekendInfo:\n  TrackName: Spa-Francorchamps\nDriverInfo:\n  DriverCarIdx: 0\n  Drivers:\n  - CarIdx: 0\n    UserName: Test Driver\n    CarPath: ferrari488gt3\n    CarScreenName: Ferrari 488 GT3 Evo 2020\n  - CarIdx: 1\n    UserName: Rival Driver\n    CarPath: porsche991gt3\n    CarScreenName: Porsche 911 GT3 R\n";

    /// Writes a little-endian `i32` into `image` at `offset`.
    fn put_i32(image: &mut [u8], offset: usize, value: i32) {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Builds a minimal valid iRacing map image: header + 1 var header
    /// (`SessionState`) + 1 data buffer + session YAML.
    fn build_image(session_state: i32, yaml: &str) -> Vec<u8> {
        let yaml_bytes = yaml.as_bytes();
        let var_header_offset: usize = HEADER_LEN;
        let buf_offset: usize = var_header_offset + VAR_HEADER_LEN;
        let buf_len: usize = 64;
        let yaml_offset: usize = buf_offset + buf_len;
        let total = yaml_offset + yaml_bytes.len();
        let mut image = vec![0_u8; total];
        put_i32(&mut image, 0, 2); // ver
        put_i32(&mut image, 4, 1); // status: connected
        put_i32(&mut image, 8, 60); // tickRate
        put_i32(&mut image, 12, 7); // sessionInfoUpdate
        put_i32(&mut image, 16, i32::try_from(yaml_bytes.len()).unwrap());
        put_i32(&mut image, 20, i32::try_from(yaml_offset).unwrap());
        put_i32(&mut image, 24, 1); // numVars
        put_i32(&mut image, 28, i32::try_from(var_header_offset).unwrap());
        put_i32(&mut image, 32, 1); // numBuf
        put_i32(&mut image, 36, i32::try_from(buf_len).unwrap());
        // varBuf[0]: tickCount + bufOffset.
        put_i32(&mut image, 48, 1234);
        put_i32(&mut image, 52, i32::try_from(buf_offset).unwrap());
        // varHeader[0]: SessionState, int, offset 0, count 1.
        let vh = var_header_offset;
        put_i32(&mut image, vh, VAR_TYPE_INT);
        put_i32(&mut image, vh + 4, 0);
        put_i32(&mut image, vh + 8, 1);
        image[vh + 16..vh + 16 + "SessionState".len()].copy_from_slice(b"SessionState");
        // Buffer data: SessionState value.
        put_i32(&mut image, buf_offset, session_state);
        image[yaml_offset..].copy_from_slice(yaml_bytes);
        image
    }

    #[test]
    fn header_layout_matches_sdk() {
        assert_eq!(size_of::<HeaderRaw>(), HEADER_LEN);
        assert_eq!(offset_of!(HeaderRaw, ver), 0);
        assert_eq!(offset_of!(HeaderRaw, status), 4);
        assert_eq!(offset_of!(HeaderRaw, session_info_len), 16);
        assert_eq!(offset_of!(HeaderRaw, session_info_offset), 20);
        assert_eq!(offset_of!(HeaderRaw, num_vars), 24);
        assert_eq!(offset_of!(HeaderRaw, var_header_offset), 28);
        assert_eq!(offset_of!(HeaderRaw, num_buf), 32);
        assert_eq!(offset_of!(HeaderRaw, buf_len), 36);
        assert_eq!(offset_of!(HeaderRaw, var_buf), VAR_BUF_ARRAY_OFFSET);
        assert_eq!(size_of::<VarBufRaw>(), VAR_BUF_LEN);
        assert_eq!(size_of::<VarHeaderRaw>(), VAR_HEADER_LEN);
        assert_eq!(offset_of!(VarHeaderRaw, name), 16);
        assert_eq!(offset_of!(VarHeaderRaw, desc), 48);
        assert_eq!(offset_of!(VarHeaderRaw, unit), 112);
    }

    #[test]
    fn session_state_mapping() {
        assert_eq!(session_state_from_raw(0), SessionState::Unknown);
        assert_eq!(session_state_from_raw(1), SessionState::Loading);
        assert_eq!(session_state_from_raw(2), SessionState::Loading);
        assert_eq!(session_state_from_raw(3), SessionState::OnTrack);
        assert_eq!(session_state_from_raw(4), SessionState::OnTrack);
        assert_eq!(session_state_from_raw(5), SessionState::OnTrack);
        assert_eq!(session_state_from_raw(6), SessionState::OnTrack);
        assert_eq!(session_state_from_raw(99), SessionState::Unknown);
    }

    #[test]
    fn yaml_parser_picks_player_car_and_track() {
        let (car, track) = parse_session_yaml(YAML).expect("YAML must parse");
        assert_eq!(car, "Ferrari 488 GT3 Evo 2020");
        assert_eq!(track, "Spa-Francorchamps");
    }

    #[test]
    fn yaml_parser_falls_back_to_car_path_and_first_driver() {
        let yaml = "WeekendInfo:\n  TrackName: Monza\nDriverInfo:\n  Drivers:\n  - CarIdx: 5\n    CarPath: ks_ferrari_488_gt3\n";
        let (car, track) = parse_session_yaml(yaml).expect("YAML must parse");
        assert_eq!(car, "ks_ferrari_488_gt3");
        assert_eq!(track, "Monza");
    }

    #[test]
    fn yaml_parser_returns_none_without_car() {
        assert_eq!(
            parse_session_yaml("WeekendInfo:\n  TrackName: Monza\n"),
            None
        );
        assert_eq!(parse_session_yaml(""), None);
    }

    #[test]
    fn iracing_snapshot_parses_fixture_image() {
        let image = build_image(4, YAML);
        let map = SliceMap::new(&image);
        let snap = read_iracing_snapshot(&map).expect("image must parse");
        assert_eq!(snap.game, GameId::IRacing);
        assert_eq!(snap.car_display, "Ferrari 488 GT3 Evo 2020");
        assert_eq!(snap.car_model, "ferrari_488_gt3_evo_2020");
        assert_eq!(snap.track_name, "Spa-Francorchamps");
        assert_eq!(snap.state, SessionState::OnTrack);
        assert!(snap.is_on_track());
    }

    #[test]
    fn iracing_snapshot_warmup_is_loading_not_on_track() {
        let image = build_image(2, YAML);
        let snap = read_iracing_snapshot(&SliceMap::new(&image)).expect("must parse");
        assert_eq!(snap.state, SessionState::Loading);
        assert!(!snap.is_on_track());
    }

    #[test]
    fn iracing_rejects_not_connected() {
        let mut image = build_image(4, YAML);
        image[4..8].copy_from_slice(&0_i32.to_le_bytes()); // status = 0
        assert!(read_iracing_snapshot(&SliceMap::new(&image)).is_none());
    }

    #[test]
    fn iracing_rejects_bad_header_fields() {
        // ver = 0
        let mut image = build_image(4, YAML);
        image[0..4].copy_from_slice(&0_i32.to_le_bytes());
        assert!(read_iracing_snapshot(&SliceMap::new(&image)).is_none());
        // numVars = 10001
        let mut image = build_image(4, YAML);
        image[24..28].copy_from_slice(&10_001_i32.to_le_bytes());
        assert!(read_iracing_snapshot(&SliceMap::new(&image)).is_none());
        // numBuf = 0
        let mut image = build_image(4, YAML);
        image[32..36].copy_from_slice(&0_i32.to_le_bytes());
        assert!(read_iracing_snapshot(&SliceMap::new(&image)).is_none());
        // sessionInfoLen over the 256 KiB bound
        let mut image = build_image(4, YAML);
        image[16..20].copy_from_slice(&(256 * 1024 + 1_i32).to_le_bytes());
        assert!(read_iracing_snapshot(&SliceMap::new(&image)).is_none());
        // sessionInfoOffset pointing outside the map
        let mut image = build_image(4, YAML);
        image[20..24].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(read_iracing_snapshot(&SliceMap::new(&image)).is_none());
    }

    #[test]
    fn iracing_rejects_truncated_image() {
        let image = build_image(4, YAML);
        let truncated = &image[..HEADER_LEN - 1];
        assert!(read_iracing_snapshot(&SliceMap::new(truncated)).is_none());
        assert!(read_iracing_snapshot(&SliceMap::new(&[])).is_none());
    }
}
