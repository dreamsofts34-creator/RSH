//! `rsh-led`: the LED pattern model on the PC side — brightness scaling and
//! the `OFF` / `SOLID` / `BLINK` / `CHASE` / `RAINBOW` patterns for the WS2812
//! strip (at most 60 LEDs, `LED_TEST` capped at 60 seconds).
//!
//! This crate is the PC-side mirror of the firmware's `led_patterns.h`
//! (PROTOCOL-V2.1 §3.7, 8-byte `LED_TEST` request payload). The firmware
//! is the authority — its `rsh_led_pattern_validate` is what the board
//! enforces — and this module validates the same rules before a pattern
//! ever goes on the wire, so a bad request fails on the PC with a clear
//! reason instead of travelling to the board and coming back as
//! `UNSUPPORTED`.
//!
//! # Period rule (M7 / C-LED-1)
//!
//! `period_ms` must be 100–5000 for `BLINK` / `CHASE` / `RAINBOW` —
//! exactly the firmware's rule (`led_patterns.h`). `SOLID` and `OFF`
//! do not use a period (the firmware's pattern engine runs `SOLID` at
//! 1 s per repeat and `OFF` for 0 ms), so their period is ignored,
//! not rejected. Brightness scaling (0–100 → RGB) lives in the
//! app-core LED apply path, not here (C-LED-1).

/// Minimum `LED_TEST` period in milliseconds (firmware `led_patterns.h`).
pub const LED_TEST_PERIOD_MIN_MS: u16 = 100;
/// Maximum `LED_TEST` period in milliseconds (firmware `led_patterns.h`).
pub const LED_TEST_PERIOD_MAX_MS: u16 = 5000;
/// Maximum LEDs on one strip (firmware board limit, §1.3).
pub const LED_MAX_COUNT: usize = 60;
/// Maximum total `LED_TEST` run time in milliseconds — SEC-H-10 cap
/// (firmware `RSH_LED_TEST_MAX_MS`).
pub const LED_TEST_MAX_MS: u32 = 60_000;
/// Default run time when `repeats == 0` (§3.7: 5 s, then stop).
pub const LED_TEST_DEFAULT_MS: u32 = 5_000;

/// A `LED_TEST` pattern id (§3.7 wire values — never change these).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedPatternId {
    /// Strip off.
    Off = 0,
    /// Solid colour.
    Solid = 1,
    /// Blink on/off.
    Blink = 2,
    /// One lit pixel chases along the strip.
    Chase = 3,
    /// Hue wheel over the whole strip.
    Rainbow = 4,
}

impl LedPatternId {
    /// Parse a wire pattern id; `None` for anything outside 0–4.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Off),
            1 => Some(Self::Solid),
            2 => Some(Self::Blink),
            3 => Some(Self::Chase),
            4 => Some(Self::Rainbow),
            _ => None,
        }
    }

    /// True when this pattern's `period_ms` is meaningful — and
    /// therefore validated to 100–5000 (firmware rule).
    #[must_use]
    pub const fn uses_period(self) -> bool {
        matches!(self, Self::Blink | Self::Chase | Self::Rainbow)
    }
}

/// What can be wrong with a `LED_TEST` pattern request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedPatternError {
    /// `patternId` is not 0–4.
    UnknownPattern(u8),
    /// `reserved` byte is not 0.
    ReservedNonZero(u8),
    /// `period_ms` is outside 100–5000 for a pattern that uses it.
    PeriodOutOfRange {
        /// The pattern that needs a valid period.
        pattern: LedPatternId,
        /// The period that was supplied.
        period_ms: u16,
    },
}

impl std::fmt::Display for LedPatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPattern(id) => write!(f, "unknown LED pattern id {id} (0–4)"),
            Self::ReservedNonZero(value) => {
                write!(f, "LED_TEST reserved byte must be 0, got {value}")
            }
            Self::PeriodOutOfRange { pattern, period_ms } => write!(
                f,
                "LED {pattern:?} period {period_ms} ms is outside 100–5000 ms"
            ),
        }
    }
}

impl std::error::Error for LedPatternError {}

/// One `LED_TEST` request (wire layout §3.7, 8 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedTestPattern {
    /// Pattern id.
    pub pattern: LedPatternId,
    /// Red (0–255).
    pub r: u8,
    /// Green (0–255).
    pub g: u8,
    /// Blue (0–255).
    pub b: u8,
    /// Period in milliseconds (100–5000 for `BLINK` / `CHASE` /
    /// `RAINBOW`; ignored for `SOLID` / `OFF`).
    pub period_ms: u16,
    /// Repeats (0 = run for [`LED_TEST_DEFAULT_MS`], then stop).
    pub repeats: u8,
}

impl LedTestPattern {
    /// Build + validate a pattern. The period rule is enforced here
    /// (see the module docs) so callers cannot construct an invalid
    /// request by hand.
    ///
    /// # Errors
    ///
    /// [`LedPatternError::PeriodOutOfRange`] when a period-using
    /// pattern has `period_ms` outside 100–5000.
    pub const fn new(
        pattern: LedPatternId,
        r: u8,
        g: u8,
        b: u8,
        period_ms: u16,
        repeats: u8,
    ) -> Result<Self, LedPatternError> {
        if pattern.uses_period()
            && (period_ms < LED_TEST_PERIOD_MIN_MS || period_ms > LED_TEST_PERIOD_MAX_MS)
        {
            return Err(LedPatternError::PeriodOutOfRange { pattern, period_ms });
        }
        Ok(Self {
            pattern,
            r,
            g,
            b,
            period_ms,
            repeats,
        })
    }

    /// Parse an 8-byte wire payload (§3.7) and validate it — the same
    /// checks the firmware runs in `rsh_led_pattern_parse` +
    /// `rsh_led_pattern_validate` (minus the pin rule, which needs the
    /// board's config).
    ///
    /// # Errors
    ///
    /// [`LedPatternError::UnknownPattern`] for a wrong length (the
    /// length byte is reported as the offending value) or an unknown
    /// pattern id; `ReservedNonZero` / `PeriodOutOfRange` for the
    /// other bad fields.
    pub fn from_wire(payload: &[u8]) -> Result<Self, LedPatternError> {
        if payload.len() != 8 {
            // A wrong-length payload has no pattern id to report; use
            // the length as the offending value (capped at u8::MAX so
            // the report stays specific without a new variant).
            return Err(LedPatternError::UnknownPattern(
                u8::try_from(payload.len()).unwrap_or(u8::MAX),
            ));
        }
        let Some(pattern) = LedPatternId::from_u8(payload[0]) else {
            return Err(LedPatternError::UnknownPattern(payload[0]));
        };
        if payload[7] != 0 {
            return Err(LedPatternError::ReservedNonZero(payload[7]));
        }
        let period_ms = u16::from_le_bytes([payload[4], payload[5]]);
        Self::new(
            pattern, payload[1], payload[2], payload[3], period_ms, payload[6],
        )
    }

    /// Total run time in milliseconds for this pattern (firmware
    /// `rsh_led_pattern_start` rule): `repeats == 0` → 5 s;
    /// `SOLID` → 1 s per repeat; `OFF` → 0; anything else → one
    /// period per repeat. Capped at [`LED_TEST_MAX_MS`] (SEC-H-10).
    #[must_use]
    pub fn total_run_ms(self) -> u32 {
        let one_repeat_ms: u32 = match self.pattern {
            LedPatternId::Off => 0,
            LedPatternId::Solid => 1000,
            _ => {
                if self.period_ms == 0 {
                    1000
                } else {
                    u32::from(self.period_ms)
                }
            }
        };
        // OFF always runs 0 ms — the firmware overrides the default
        // 5 s for OFF after computing it (rsh_led_pattern_start).
        if self.pattern == LedPatternId::Off {
            return 0;
        }
        let total = if self.repeats == 0 {
            LED_TEST_DEFAULT_MS
        } else {
            one_repeat_ms.saturating_mul(u32::from(self.repeats))
        };
        if total > LED_TEST_MAX_MS {
            LED_TEST_MAX_MS
        } else {
            total
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_bounds_are_100_to_5000() {
        assert_eq!(LED_TEST_PERIOD_MIN_MS, 100);
        assert_eq!(LED_TEST_PERIOD_MAX_MS, 5000);
        // Boundaries pass for a period-using pattern…
        assert!(LedTestPattern::new(LedPatternId::Blink, 255, 0, 0, 100, 0).is_ok());
        assert!(LedTestPattern::new(LedPatternId::Chase, 255, 0, 0, 5000, 0).is_ok());
        assert!(LedTestPattern::new(LedPatternId::Rainbow, 255, 0, 0, 5000, 3).is_ok());
        // …and one millisecond outside either end fails (M7).
        assert_eq!(
            LedTestPattern::new(LedPatternId::Blink, 255, 0, 0, 99, 0),
            Err(LedPatternError::PeriodOutOfRange {
                pattern: LedPatternId::Blink,
                period_ms: 99
            })
        );
        assert_eq!(
            LedTestPattern::new(LedPatternId::Blink, 255, 0, 0, 5001, 0),
            Err(LedPatternError::PeriodOutOfRange {
                pattern: LedPatternId::Blink,
                period_ms: 5001
            })
        );
        assert!(LedTestPattern::new(LedPatternId::Chase, 0, 0, 0, 50, 0).is_err());
    }

    #[test]
    fn solid_and_off_ignore_period_like_firmware() {
        // The firmware only validates the period for BLINK / CHASE /
        // RAINBOW — SOLID runs 1 s per repeat, OFF runs 0 ms.
        assert!(LedTestPattern::new(LedPatternId::Solid, 255, 0, 0, 50, 0).is_ok());
        assert!(LedTestPattern::new(LedPatternId::Solid, 255, 0, 0, 1000, 0).is_ok());
        assert!(LedTestPattern::new(LedPatternId::Off, 0, 0, 0, 0, 0).is_ok());
    }

    #[test]
    fn wire_parse_validates_like_firmware() {
        // BLINK, period 1000, repeats 0 — the firmware's own §21 test
        // vector shape (patternId/reserved/period).
        let ok = LedTestPattern::from_wire(&[2, 255, 0, 0, 0xE8, 0x03, 0, 0]).unwrap();
        assert_eq!(ok.pattern, LedPatternId::Blink);
        assert_eq!(ok.period_ms, 1000);
        // BLINK period 50 → rejected (firmware: UNSUPPORTED).
        assert!(LedTestPattern::from_wire(&[2, 255, 0, 0, 50, 0, 0, 0]).is_err());
        // Unknown pattern id → rejected.
        assert_eq!(
            LedTestPattern::from_wire(&[9, 0, 0, 0, 0, 0, 0, 0]),
            Err(LedPatternError::UnknownPattern(9))
        );
        // Reserved != 0 → rejected.
        assert_eq!(
            LedTestPattern::from_wire(&[1, 255, 0, 0, 0, 0, 0, 1]),
            Err(LedPatternError::ReservedNonZero(1))
        );
        // Wrong length → rejected.
        assert!(LedTestPattern::from_wire(&[1, 2, 3]).is_err());
    }

    #[test]
    fn total_run_ms_matches_firmware_rule() {
        // repeats == 0 → 5 s default.
        let p = LedTestPattern::new(LedPatternId::Blink, 0, 0, 0, 1000, 0).unwrap();
        assert_eq!(p.total_run_ms(), 5000);
        // 3 repeats of a 1000 ms blink → 3 s.
        let p = LedTestPattern::new(LedPatternId::Blink, 0, 0, 0, 1000, 3).unwrap();
        assert_eq!(p.total_run_ms(), 3000);
        // SOLID: 1 s per repeat regardless of period.
        let p = LedTestPattern::new(LedPatternId::Solid, 0, 0, 0, 1000, 3).unwrap();
        assert_eq!(p.total_run_ms(), 3000);
        // OFF: 0 ms.
        let p = LedTestPattern::new(LedPatternId::Off, 0, 0, 0, 0, 0).unwrap();
        assert_eq!(p.total_run_ms(), 0);
        // SEC-H-10 cap: 255 repeats × 5000 ms clamps to 60 s.
        let p = LedTestPattern::new(LedPatternId::Blink, 0, 0, 0, 5000, 255).unwrap();
        assert_eq!(p.total_run_ms(), 60_000);
    }
}
