//! Assetto Corsa / ACC shared-memory reader (Kunos `acpmf_*` maps).
//!
//! Layout source: `TELEMETRY-READER.md` §2 (Phase H corrections) cross-
//! checked against the community Kunos shared-memory documentation:
//!
//! * Static block (`acpmf_static`): `wchar_t smVersion[15]` @ 0,
//!   `wchar_t acVersion[15]` @ 30, `int numberOfSessions` @ 60,
//!   `int numCars` @ 64, **`wchar_t carModel[33]` @ 68**,
//!   `wchar_t track[33]` @ 134. All strings are UTF-16LE, NUL-terminated
//!   within 33 `wchar_t` units.
//! * Graphics block (`acpmf_graphics`): `int packetId` @ 0,
//!   **`AC_STATUS status` @ 4** (`0` OFF, `1` REPLAY, `2` LIVE, `3`
//!   PAUSE), `AC_SESSION_TYPE session` @ 8.
//!
//! ACC uses the **same map names** (`acpmf_*`) and the **same identity
//! field offsets** as AC — the two readers differ only by the process
//! gate (`acs.exe` vs `acc.exe`). ACC's later static fields differ from
//! AC's (air/road temp instead of the deprecated slots), but no field
//! after `track` is read here, so one parser serves both games.
//!
//! Version tolerance: Kunos has appended fields to the static struct
//! across AC versions (`trackConfiguration`, `ersMaxJ`, `carSkin`, …),
//! so an exact-size check would false-reject newer games. The reader
//! therefore requires only the **minimum** size covering the fields it
//! reads (static ≥ 200 B, graphics ≥ 8 B) and parses NUL-terminated
//! UTF-16 — the same posture as the C# size check, minus the false
//! rejections. There is no magic/version field in the Kunos maps to
//! check; a map of all zeros yields an empty car name and is rejected.

use crate::access::{MapAccessor, read_consistent, read_i32};
use crate::types::{GameId, SessionState, TelemetrySnapshot};

/// Name of the Kunos static shared-memory map (AC and ACC alike).
pub const STATIC_MAP_NAME: &str = "acpmf_static";
/// Name of the Kunos graphics shared-memory map (AC and ACC alike).
pub const GRAPHICS_MAP_NAME: &str = "acpmf_graphics";

/// Byte offset of `carModel` (`wchar_t[33]`) in the static block.
pub const STATIC_CAR_MODEL_OFFSET: usize = 68;
/// Byte offset of `track` (`wchar_t[33]`) in the static block.
pub const STATIC_TRACK_OFFSET: usize = 134;
/// Length of a Kunos `wchar_t[33]` string field, in UTF-16 code units.
pub const WSTR_LEN: usize = 33;
/// Minimum static-block size: must cover the whole `track` field
/// (134 + 33 × 2 = 200 bytes).
pub const STATIC_MIN_LEN: usize = STATIC_TRACK_OFFSET + WSTR_LEN * 2;
/// Byte offset of `AC_STATUS status` in the graphics block.
pub const GRAPHICS_STATUS_OFFSET: usize = 4;
/// Minimum graphics-block size: must cover the `status` field (4 + 4).
pub const GRAPHICS_MIN_LEN: usize = GRAPHICS_STATUS_OFFSET + 4;

/// `AC_STATUS` values (Kunos docs; shared by AC and ACC).
pub const AC_STATUS_OFF: i32 = 0;
/// `AC_STATUS` values (Kunos docs; shared by AC and ACC).
pub const AC_STATUS_REPLAY: i32 = 1;
/// `AC_STATUS` values (Kunos docs; shared by AC and ACC).
pub const AC_STATUS_LIVE: i32 = 2;
/// `AC_STATUS` values (Kunos docs; shared by AC and ACC).
pub const AC_STATUS_PAUSE: i32 = 3;

/// Maps a raw `AC_STATUS` value to a [`SessionState`].
///
/// Only `LIVE` is [`SessionState::OnTrack`]; `OFF` means the game is in
/// menus, which must never trigger a switch.
#[must_use]
pub const fn ac_status_to_state(status: i32) -> SessionState {
    match status {
        AC_STATUS_OFF => SessionState::Menu,
        AC_STATUS_REPLAY => SessionState::Replay,
        AC_STATUS_LIVE => SessionState::OnTrack,
        AC_STATUS_PAUSE => SessionState::Paused,
        _ => SessionState::Unknown,
    }
}

/// Decodes one NUL-terminated UTF-16LE `wchar_t[33]` field.
///
/// `bytes` is the raw block, `offset` the field's byte offset. Reads at
/// most 33 code units, stops at the first NUL, and requires strictly
/// valid UTF-16 (an unpaired surrogate rejects the read — fail-safe).
/// Returns the trimmed string, or `None` when the field is out of range
/// or not valid UTF-16.
#[must_use]
pub fn decode_wstr(bytes: &[u8], offset: usize) -> Option<String> {
    let end = offset.checked_add(WSTR_LEN * 2)?;
    let field = bytes.get(offset..end)?;
    let mut units = Vec::with_capacity(WSTR_LEN);
    for pair in field.as_chunks::<2>().0 {
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    let text = String::from_utf16(&units).ok()?;
    Some(text.trim().to_string())
}

/// Parses the Kunos static block: `(car_model_raw, track_raw)`.
///
/// Returns `None` when the block is too short, torn (double-read
/// mismatch), or the car name is empty.
fn parse_static(map: &dyn MapAccessor) -> Option<(String, String)> {
    let bytes = read_consistent(map, 0, STATIC_MIN_LEN)?;
    let car = decode_wstr(&bytes, STATIC_CAR_MODEL_OFFSET)?;
    if car.is_empty() {
        return None;
    }
    let track = decode_wstr(&bytes, STATIC_TRACK_OFFSET).unwrap_or_default();
    Some((car, track))
}

/// Parses the Kunos graphics block into a [`SessionState`].
///
/// Returns `None` when the block is too short or torn.
fn parse_graphics_state(map: &dyn MapAccessor) -> Option<SessionState> {
    let status = read_i32(map, GRAPHICS_STATUS_OFFSET)?;
    Some(ac_status_to_state(status))
}

/// Reads one Kunos (AC or ACC) identity snapshot from two map accessors.
///
/// Pure and platform-independent: on Windows the caller passes the live
/// mapped views; in tests it passes [`SliceMap`](crate::access::SliceMap)
/// over recorded fixture bytes. Returns `None` when either map is
/// missing/short/torn, or when no car name is present — never panics.
#[must_use]
pub fn read_kunos_snapshot(
    static_map: &dyn MapAccessor,
    graphics_map: &dyn MapAccessor,
    game: GameId,
) -> Option<TelemetrySnapshot> {
    let (car, track) = parse_static(static_map)?;
    let state = parse_graphics_state(graphics_map)?;
    Some(TelemetrySnapshot::new(game, &car, &track, state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::SliceMap;

    /// Writes a NUL-terminated UTF-16LE string into `block` at `offset`.
    fn put_wstr(block: &mut [u8], offset: usize, text: &str) {
        for (i, unit) in text.encode_utf16().enumerate() {
            let at = offset + i * 2;
            block[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
    }

    /// Writes a little-endian `i32` into `block` at `offset`.
    fn put_i32(block: &mut [u8], offset: usize, value: i32) {
        block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Builds a minimal static block: `carModel` / `track` as UTF-16LE.
    fn static_block(car: &str, track: &str) -> Vec<u8> {
        let mut block = vec![0_u8; 256];
        put_wstr(&mut block, 0, "1.4");
        put_wstr(&mut block, 30, "1.16.4");
        put_i32(&mut block, 60, 1);
        put_i32(&mut block, 64, 20);
        put_wstr(&mut block, STATIC_CAR_MODEL_OFFSET, car);
        put_wstr(&mut block, STATIC_TRACK_OFFSET, track);
        block
    }

    fn graphics_block(status: i32) -> Vec<u8> {
        let mut block = vec![0_u8; 64];
        put_i32(&mut block, 0, 1);
        put_i32(&mut block, 4, status);
        block
    }

    fn snapshot(
        static_bytes: &[u8],
        graphics_bytes: &[u8],
        game: GameId,
    ) -> Option<TelemetrySnapshot> {
        let static_map = SliceMap::new(static_bytes);
        let graphics_map = SliceMap::new(graphics_bytes);
        read_kunos_snapshot(&static_map, &graphics_map, game)
    }

    #[test]
    fn layout_offsets_match_kunos_docs() {
        // wchar_t[15] + wchar_t[15] + int + int = 30 + 30 + 4 + 4 = 68.
        assert_eq!(STATIC_CAR_MODEL_OFFSET, 15 * 2 + 15 * 2 + 4 + 4);
        assert_eq!(STATIC_TRACK_OFFSET, STATIC_CAR_MODEL_OFFSET + 33 * 2);
        assert_eq!(STATIC_MIN_LEN, 200);
        assert_eq!(GRAPHICS_STATUS_OFFSET, 4);
    }

    #[test]
    fn ac_status_mapping() {
        assert_eq!(ac_status_to_state(0), SessionState::Menu);
        assert_eq!(ac_status_to_state(1), SessionState::Replay);
        assert_eq!(ac_status_to_state(2), SessionState::OnTrack);
        assert_eq!(ac_status_to_state(3), SessionState::Paused);
        assert_eq!(ac_status_to_state(99), SessionState::Unknown);
    }

    #[test]
    fn decode_wstr_ascii_and_empty() {
        let block = static_block("ferrari_488_gt3", "monza");
        assert_eq!(
            decode_wstr(&block, STATIC_CAR_MODEL_OFFSET),
            Some("ferrari_488_gt3".to_string())
        );
        assert_eq!(
            decode_wstr(&block, STATIC_TRACK_OFFSET),
            Some("monza".to_string())
        );
        // All-zero field decodes to an empty string.
        let zeros = vec![0_u8; 256];
        assert_eq!(
            decode_wstr(&zeros, STATIC_CAR_MODEL_OFFSET),
            Some(String::new())
        );
        // Out of range.
        assert_eq!(decode_wstr(&zeros, 200), None);
    }

    #[test]
    fn decode_wstr_rejects_unpaired_surrogate() {
        let mut block = vec![0_u8; 256];
        // Lone high surrogate 0xD800, then NUL.
        block[STATIC_CAR_MODEL_OFFSET..STATIC_CAR_MODEL_OFFSET + 2]
            .copy_from_slice(&0xD800_u16.to_le_bytes());
        assert_eq!(decode_wstr(&block, STATIC_CAR_MODEL_OFFSET), None);
    }

    #[test]
    fn kunos_snapshot_parses_car_track_state() {
        let static_bytes = static_block("ferrari_488_gt3", "monza");
        let graphics_bytes = graphics_block(AC_STATUS_LIVE);
        let snap = snapshot(&static_bytes, &graphics_bytes, GameId::AssettoCorsa)
            .expect("fixture must parse");
        assert_eq!(snap.game, GameId::AssettoCorsa);
        assert_eq!(snap.car_model, "ferrari_488_gt3");
        assert_eq!(snap.car_display, "ferrari_488_gt3");
        assert_eq!(snap.track_name, "monza");
        assert_eq!(snap.state, SessionState::OnTrack);
    }

    #[test]
    fn kunos_snapshot_acc_display_name_normalizes() {
        let static_bytes = static_block("Ferrari 488 GT3 Evo", "spa");
        let graphics_bytes = graphics_block(AC_STATUS_LIVE);
        let snap =
            snapshot(&static_bytes, &graphics_bytes, GameId::Acc).expect("fixture must parse");
        assert_eq!(snap.car_model, "ferrari_488_gt3_evo");
        assert_eq!(snap.car_display, "Ferrari 488 GT3 Evo");
    }

    #[test]
    fn kunos_snapshot_rejects_short_static() {
        let static_bytes = static_block("ferrari_488_gt3", "monza");
        let graphics_bytes = graphics_block(AC_STATUS_LIVE);
        assert!(snapshot(&static_bytes[..100], &graphics_bytes, GameId::AssettoCorsa).is_none());
    }

    #[test]
    fn kunos_snapshot_rejects_empty_car_and_missing_graphics() {
        let empty_car = static_block("", "monza");
        let graphics_bytes = graphics_block(AC_STATUS_LIVE);
        assert!(snapshot(&empty_car, &graphics_bytes, GameId::AssettoCorsa).is_none());
        let static_bytes = static_block("ferrari_488_gt3", "monza");
        let short_graphics = vec![0_u8; 4];
        assert!(snapshot(&static_bytes, &short_graphics, GameId::AssettoCorsa).is_none());
    }

    #[test]
    fn kunos_snapshot_menu_state_never_on_track() {
        let static_bytes = static_block("ferrari_488_gt3", "monza");
        let graphics_bytes = graphics_block(AC_STATUS_OFF);
        let snap = snapshot(&static_bytes, &graphics_bytes, GameId::AssettoCorsa)
            .expect("menu snapshot must still parse");
        assert_eq!(snap.state, SessionState::Menu);
        assert!(!snap.is_on_track());
    }
}
