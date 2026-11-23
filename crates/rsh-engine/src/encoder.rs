//! Cumulative i16 encoder deltas (wrap-aware) and encoder→button
//! mapping (SW-R3.3, D6).
//!
//! Encoders arrive as cumulative `i16` counts (`n_enc <= 4`). The delta
//! between two readings is a wrapping subtraction, so `32767 -> -32768`
//! counts as `+1` and `-32768 -> 32767` counts as `-1` — never a jump
//! of ~65535.
//!
//! Encoder→button mapping (buttons 65–72): each encoder drives two
//! virtual buttons, one per direction. Encoder `i` (`0..=3`) clockwise
//! is button `65 + 2*i`, counter-clockwise is `66 + 2*i`:
//!
//! | encoder | CW (delta > 0) | CCW (delta < 0) |
//! |---------|----------------|-----------------|
//! | 0       | 65             | 66              |
//! | 1       | 67             | 68              |
//! | 2       | 69             | 70              |
//! | 3       | 71             | 72              |
//!
//! Pulses are momentary: a delta produces a button press for exactly
//! the tick in which the delta was observed.

use rsh_protocol::stream::MAX_ENCODERS;

/// Largest per-tick encoder step the tracker accepts.
///
/// A jump of `|delta| > 32` in one 5 ms tick is physically impossible
/// at 200 Hz and is a glitch (TESTING-INPUT.md Phase 2 test scope:
/// "encoder tracker (|delta| > 32/tick filter, reversal-undo)";
/// §8.2 injects rare ±50..200 jumps, §8.4 `encoder-glitch-v2`
/// asserts `maxAbsEncoderDelta <= 32`). Such a delta is dropped —
/// never applied to the output — while the baseline still advances
/// to the new count, so the next tick measures from the post-glitch
/// count instead of re-reporting the same jump forever (the wheel's
/// glitch is a permanent offset in its cumulative count).
pub const MAX_ENCODER_DELTA_PER_TICK: i16 = 32;

/// Rotation direction of one encoder step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderDirection {
    /// Count increased (delta > 0).
    Clockwise,
    /// Count decreased (delta < 0).
    CounterClockwise,
}

/// Wrap-aware delta between two cumulative `i16` encoder readings.
///
/// `32767 -> -32768` is `+1`; `-32768 -> 32767` is `-1`. Pure and
/// allocation-free.
#[must_use]
pub const fn encoder_delta(new: i16, old: i16) -> i16 {
    new.wrapping_sub(old)
}

/// The virtual button number (65–72) for one encoder step.
///
/// Returns `None` when `index >= 4` (D6: at most 4 encoders).
#[must_use]
pub fn encoder_button_number(index: usize, direction: EncoderDirection) -> Option<u8> {
    let index = u8::try_from(index).ok()?;
    if index as usize >= MAX_ENCODERS {
        return None;
    }
    let base = 65 + 2 * index;
    Some(match direction {
        EncoderDirection::Clockwise => base,
        EncoderDirection::CounterClockwise => base + 1,
    })
}

/// Bit position inside an [`EncoderButtons`] mask for one encoder step.
///
/// Bit `2*i` = encoder `i` clockwise, bit `2*i + 1` = counter-clockwise.
/// Returns `None` when `index >= 4`.
#[must_use]
pub fn encoder_button_bit(index: usize, direction: EncoderDirection) -> Option<u8> {
    let index = u8::try_from(index).ok()?;
    if index as usize >= MAX_ENCODERS {
        return None;
    }
    Some(match direction {
        EncoderDirection::Clockwise => 2 * index,
        EncoderDirection::CounterClockwise => 2 * index + 1,
    })
}

/// Momentary encoder-button pulses for one tick, as a bitmask.
///
/// Bit `2*i` set means encoder `i` stepped clockwise this tick
/// (virtual button `65 + 2*i`); bit `2*i + 1` set means it stepped
/// counter-clockwise (virtual button `66 + 2*i`). Only bits `0..=7`
/// are ever used; a bit is set for exactly the tick its delta was
/// observed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EncoderButtons(pub u8);

impl EncoderButtons {
    /// No pulses this tick.
    #[must_use]
    pub const fn none() -> Self {
        Self(0)
    }

    /// True when encoder `index` stepped clockwise this tick.
    #[must_use]
    pub const fn clockwise(&self, index: usize) -> bool {
        index < MAX_ENCODERS && self.0 & (1u8 << (2 * index)) != 0
    }

    /// True when encoder `index` stepped counter-clockwise this tick.
    #[must_use]
    pub const fn counter_clockwise(&self, index: usize) -> bool {
        index < MAX_ENCODERS && self.0 & (1u8 << (2 * index + 1)) != 0
    }

    /// True when any encoder stepped this tick.
    #[must_use]
    pub const fn any(&self) -> bool {
        self.0 != 0
    }
}

/// Convert per-encoder deltas into a momentary [`EncoderButtons`] mask.
///
/// A positive delta sets the clockwise bit, a negative delta the
/// counter-clockwise bit, a zero delta neither. Deltas past index 3
/// are ignored (D6).
#[must_use]
pub fn deltas_to_buttons(deltas: &[i16; MAX_ENCODERS]) -> EncoderButtons {
    let mut mask: u8 = 0;
    for (i, &delta) in deltas.iter().enumerate() {
        if delta > 0 {
            mask |= 1u8 << (2 * i);
        } else if delta < 0 {
            mask |= 1u8 << (2 * i + 1);
        }
    }
    EncoderButtons(mask)
}

/// Tracks cumulative encoder counts and emits wrap-aware deltas.
///
/// The first sample for an encoder only sets the baseline (delta 0);
/// later samples report `new.wrapping_sub(old)`. Fixed-size arrays
/// only — no allocation in the hot path.
#[derive(Clone, Copy, Debug, Default)]
pub struct EncoderTracker {
    /// Last seen cumulative count per encoder, or `None` before the
    /// first sample.
    previous: [Option<i16>; MAX_ENCODERS],
    /// Glitch jumps dropped by the filter (M4: gap-aware, counted).
    glitch_dropped: u64,
}

impl EncoderTracker {
    /// An empty tracker (no baselines yet).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            previous: [None; MAX_ENCODERS],
            glitch_dropped: 0,
        }
    }

    /// Update one encoder and return its wrap-aware delta since the
    /// previous sample (0 on the first sample for that encoder).
    /// Out-of-range indices (`>= 4`) return 0 and change nothing.
    /// A glitch jump (`|delta| > 32`, see
    /// [`MAX_ENCODER_DELTA_PER_TICK`]) returns 0 and is never
    /// applied — but the baseline still advances to the new count.
    pub fn update(&mut self, index: usize, count: i16) -> i16 {
        self.update_with_gap(index, count, 0)
    }

    /// Update one encoder with the sequence gap since the previous
    /// sample (`skipped` stream frames were lost in between, M4).
    ///
    /// The per-tick limit scales with the gap: a wheel that kept
    /// turning while `skipped` frames were lost may legitimately move
    /// `32 * (skipped + 1)` counts, so the glitch threshold is
    /// `32 * (skipped + 1)` (capped at the `i16` range). A jump past
    /// that is still a glitch: dropped, counted in
    /// [`Self::glitch_dropped`], baseline still advances.
    pub fn update_with_gap(&mut self, index: usize, count: i16, skipped: u16) -> i16 {
        self.update_with_limit(index, count, gap_limit(skipped))
    }

    /// One encoder update against a precomputed glitch limit (see
    /// [`gap_limit`]): the per-encoder work of
    /// [`Self::update_with_gap`]. [`Self::update_all_with_gap`]
    /// computes the limit once per tick instead of once per
    /// encoder — the limit depends only on the gap, not the
    /// encoder.
    fn update_with_limit(&mut self, index: usize, count: i16, limit: u32) -> i16 {
        if index >= MAX_ENCODERS {
            return 0;
        }
        let delta = match self.previous[index] {
            Some(old) => encoder_delta(count, old),
            None => 0,
        };
        self.previous[index] = Some(count);
        if u32::from(delta.unsigned_abs()) > limit {
            self.glitch_dropped += 1;
            0
        } else {
            delta
        }
    }

    /// Update all encoders present in the frame and return the
    /// per-encoder deltas. Only the first `n_enc` entries are read
    /// (`n_enc <= 4`); the rest of the returned array is 0. Glitch
    /// jumps (`|delta| > 32`) are filtered to 0 by [`Self::update`].
    pub fn update_all(
        &mut self,
        counts: &[i16; MAX_ENCODERS],
        n_enc: usize,
    ) -> [i16; MAX_ENCODERS] {
        self.update_all_with_gap(counts, n_enc, 0)
    }

    /// [`Self::update_all`] with the sequence gap since the previous
    /// frame (M4): the glitch limit scales with the gap, see
    /// [`Self::update_with_gap`].
    pub fn update_all_with_gap(
        &mut self,
        counts: &[i16; MAX_ENCODERS],
        n_enc: usize,
        skipped: u16,
    ) -> [i16; MAX_ENCODERS] {
        let mut deltas = [0i16; MAX_ENCODERS];
        let limit = gap_limit(skipped);
        for (i, &count) in counts.iter().enumerate().take(n_enc.min(MAX_ENCODERS)) {
            deltas[i] = self.update_with_limit(i, count, limit);
        }
        deltas
    }

    /// Glitch jumps dropped so far by the filter.
    #[must_use]
    pub const fn glitch_dropped(&self) -> u64 {
        self.glitch_dropped
    }

    /// Forget all baselines; the next sample for each encoder is a
    /// first sample again (delta 0).
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// Glitch limit in counts for a sequence gap of `skipped` frames
/// (M4): 32 counts/tick × (`skipped` + 1) ticks of legitimate
/// motion, saturating so a huge gap can never overflow the limit
/// (capped at the `i16` range).
#[must_use]
fn gap_limit(skipped: u16) -> u32 {
    u32::from(MAX_ENCODER_DELTA_PER_TICK.unsigned_abs())
        .saturating_mul(u32::from(skipped) + 1)
        .min(u32::from(i16::MAX.unsigned_abs()) + 1)
}

#[cfg(test)]
mod tests {
    use super::{
        EncoderButtons, EncoderDirection, EncoderTracker, deltas_to_buttons, encoder_button_bit,
        encoder_button_number, encoder_delta,
    };

    #[test]
    fn golden_wrap_forward_counts_plus_one() {
        // 32767 -> -32768 is +1, not -65535.
        assert_eq!(encoder_delta(-32768, 32767), 1);
        assert_eq!(encoder_delta(32767, -32768), -1);
        assert_eq!(encoder_delta(5, 3), 2);
        assert_eq!(encoder_delta(-5, 5), -10);
        assert_eq!(encoder_delta(0, 0), 0);
    }

    #[test]
    fn golden_button_numbers_65_to_72() {
        assert_eq!(
            encoder_button_number(0, EncoderDirection::Clockwise),
            Some(65)
        );
        assert_eq!(
            encoder_button_number(0, EncoderDirection::CounterClockwise),
            Some(66)
        );
        assert_eq!(
            encoder_button_number(3, EncoderDirection::Clockwise),
            Some(71)
        );
        assert_eq!(
            encoder_button_number(3, EncoderDirection::CounterClockwise),
            Some(72)
        );
        assert_eq!(encoder_button_number(4, EncoderDirection::Clockwise), None);
        assert_eq!(encoder_button_bit(1, EncoderDirection::Clockwise), Some(2));
        assert_eq!(
            encoder_button_bit(1, EncoderDirection::CounterClockwise),
            Some(3)
        );
    }

    #[test]
    fn tracker_baseline_then_wrap_delta() {
        let mut tracker = EncoderTracker::new();
        // First sample only sets the baseline.
        assert_eq!(tracker.update(0, 32760), 0);
        assert_eq!(tracker.update(0, 32767), 7);
        // Wrap across the i16 boundary: +9 total from 32767.
        assert_eq!(tracker.update(0, -32768), 1);
        assert_eq!(tracker.update(0, -32760), 8);
        // Out-of-range index is ignored.
        assert_eq!(tracker.update(4, 100), 0);
    }

    #[test]
    fn update_all_respects_n_enc() {
        let mut tracker = EncoderTracker::new();
        let first = tracker.update_all(&[10, 20, 30, 40], 2);
        assert_eq!(first, [0, 0, 0, 0]);
        let second = tracker.update_all(&[12, 18, 99, 99], 2);
        // Only the first 2 encoders were read; the rest stay 0.
        assert_eq!(second, [2, -2, 0, 0]);
        tracker.reset();
        assert_eq!(tracker.update(0, 7), 0);
    }

    #[test]
    fn glitch_delta_over_32_is_dropped_and_baseline_resyncs() {
        let mut tracker = EncoderTracker::new();
        assert_eq!(tracker.update(0, 100), 0); // baseline
        // Boundary: exactly 32 still applies.
        assert_eq!(tracker.update(0, 132), 32);
        // Glitch jump (+200) is dropped, never applied...
        assert_eq!(tracker.update(0, 332), 0);
        // ...but the baseline moved to 332, so the next normal step
        // applies instead of re-reporting the glitch forever.
        assert_eq!(tracker.update(0, 334), 2);
        // A 33-count step is already a glitch.
        assert_eq!(tracker.update(0, 367), 0);
        assert_eq!(tracker.update(0, 369), 2);
        // Negative glitch likewise.
        assert_eq!(tracker.update(0, 169), 0);
        assert_eq!(tracker.update(0, 167), -2);
        // The update_all path filters too.
        let mut tracker = EncoderTracker::new();
        assert_eq!(tracker.update_all(&[0, 0, 0, 0], 4), [0, 0, 0, 0]);
        assert_eq!(tracker.update_all(&[200, 5, -40, 0], 4), [0, 5, 0, 0]);
        // And the tracker keeps working after the glitch.
        assert_eq!(tracker.update_all(&[202, 4, -41, 0], 4), [2, -1, -1, 0]);
    }

    #[test]
    fn deltas_to_buttons_mask() {
        let buttons = deltas_to_buttons(&[3, -2, 0, 1]);
        // Enc 0 CW -> bit 0; enc 1 CCW -> bit 3; enc 3 CW -> bit 6.
        assert_eq!(buttons, EncoderButtons(0b0100_1001));
        assert!(buttons.clockwise(0));
        assert!(!buttons.counter_clockwise(0));
        assert!(buttons.counter_clockwise(1));
        assert!(!buttons.clockwise(2));
        assert!(buttons.clockwise(3));
        assert!(buttons.any());
        assert_eq!(deltas_to_buttons(&[0, 0, 0, 0]), EncoderButtons::none());
    }

    #[test]
    fn gap_aware_filter_scales_limit_and_counts_glitches() {
        // M4: after a gap of 3 skipped frames the wheel may legitimately
        // have moved 32 * (3 + 1) = 128 counts.
        let mut tracker = EncoderTracker::new();
        assert_eq!(tracker.update(0, 0), 0);
        assert_eq!(tracker.update_with_gap(0, 128, 3), 128);
        assert_eq!(tracker.glitch_dropped(), 0);
        // One count past the gap-scaled limit is still a glitch.
        assert_eq!(tracker.update_with_gap(0, 128 + 129, 3), 0);
        assert_eq!(tracker.glitch_dropped(), 1);
        // Baseline advanced past the glitch, so normal steps apply.
        assert_eq!(tracker.update(0, 128 + 129 + 2), 2);
        // Without a gap the same 129-count jump is a glitch.
        let mut tracker = EncoderTracker::new();
        assert_eq!(tracker.update(0, 0), 0);
        assert_eq!(tracker.update(0, 129), 0);
        assert_eq!(tracker.glitch_dropped(), 1);
        // update_all_with_gap passes the gap through per encoder:
        // gap 2 allows 96, so +100 passes and +100 on the second
        // encoder... use +97 (dropped) to show the per-encoder limit.
        let mut tracker = EncoderTracker::new();
        assert_eq!(tracker.update_all(&[0, 0, 0, 0], 2), [0, 0, 0, 0]);
        assert_eq!(
            tracker.update_all_with_gap(&[96, 97, 0, 0], 2, 2),
            [96, 0, 0, 0],
            "gap 2 allows 96 counts per encoder"
        );
        assert_eq!(tracker.glitch_dropped(), 1);
    }
}
