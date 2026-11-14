//! Millivolt-domain calibration (SW-R3.4, D6 / D10 / P9).
//!
//! Every axis calibration lives in millivolts. The full ADC scale is
//! `0..=3300` mV and a calibration is only sane when its span is at least
//! 20 % of full scale ([`MIN_SPAN_MV`] = 660 mV). A narrow span is
//! rejected, never silently accepted: the pipeline falls back to the
//! default range instead (see [`DEFAULT_MIN_MV`] / [`DEFAULT_MAX_MV`]).
//!
//! Unit-less (legacy v1) calibrations are never treated as millivolts:
//! their values are kept verbatim, `is_millivolts` stays `false`, the
//! mapper treats them as insane, and the UI must ask for recalibration
//! (P9). Guessing the unit from the values is forbidden — the raw 12-bit
//! range (`0..=4095`) overlaps the mV range (`0..=3300`).

/// Full-scale ADC range in millivolts (D6: axes are mV, `0..=3300`).
pub const FULL_SCALE_MV: u16 = 3300;
/// Minimum sane calibration span in millivolts (D10: 20 % of full scale).
pub const MIN_SPAN_MV: u16 = 660;
/// Minimum sane half-span per side for steering (D10: 330 mV each side
/// of centre).
pub const MIN_HALF_SPAN_MV: u16 = 330;
/// Default range low end, used when no sane calibration exists.
pub const DEFAULT_MIN_MV: u16 = 300;
/// Default range high end, used when no sane calibration exists.
pub const DEFAULT_MAX_MV: u16 = 3300;
/// Default steering centre, used when no sane calibration exists
/// (midpoint of the default range).
pub const DEFAULT_CENTER_MV: u16 = 1800;
/// Maximum samples kept by [`AutoCalibrator`]'s bounded ring buffer.
pub const AUTO_CAL_SAMPLES: usize = 64;

/// The unit a stored calibration is expressed in.
///
/// Only [`CalUnit::Millivolts`] is usable. [`CalUnit::Unknown`] marks a
/// legacy (v1) calibration that carries no unit tag; it must never be
/// interpreted as millivolts (P9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CalUnit {
    /// Values are millivolts.
    Millivolts,
    /// No unit tag (legacy v1 import). Values are verbatim and unusable.
    #[default]
    Unknown,
}

/// Why a calibration is insane (unusable as a mapping range).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationError {
    /// No calibration was ever captured (`is_set` is false).
    NotSet,
    /// The stored values carry no millivolt unit tag (P9).
    NotMillivolts,
    /// The span is narrower than [`MIN_SPAN_MV`] (D10).
    SpanTooNarrow {
        /// The measured span in millivolts.
        span_mv: u16,
    },
    /// The endpoints are not ordered (`min < centre < max` for
    /// steering, `min < max` for pedals).
    BadOrder,
    /// One steering half-span (`centre - min` or `max - centre`) is
    /// narrower than [`MIN_HALF_SPAN_MV`].
    HalfSpanTooNarrow {
        /// The measured half-span in millivolts.
        half_span_mv: u16,
    },
}

/// Validate a steering range: `min < centre < max`, each half-span at
/// least [`MIN_HALF_SPAN_MV`], total span at least [`MIN_SPAN_MV`].
///
/// # Errors
///
/// Returns [`CalibrationError::BadOrder`], `HalfSpanTooNarrow`, or
/// `SpanTooNarrow` when the range fails the corresponding check.
pub fn validate_steering_range(
    min_mv: u16,
    center_mv: u16,
    max_mv: u16,
) -> Result<(), CalibrationError> {
    if !(min_mv < center_mv && center_mv < max_mv) {
        return Err(CalibrationError::BadOrder);
    }
    let low_half = center_mv - min_mv;
    let high_half = max_mv - center_mv;
    let half = low_half.min(high_half);
    if half < MIN_HALF_SPAN_MV {
        return Err(CalibrationError::HalfSpanTooNarrow { half_span_mv: half });
    }
    let span = max_mv - min_mv;
    if span < MIN_SPAN_MV {
        return Err(CalibrationError::SpanTooNarrow { span_mv: span });
    }
    Ok(())
}

/// Validate a pedal range: `min < max` and span at least
/// [`MIN_SPAN_MV`].
///
/// # Errors
///
/// Returns [`CalibrationError::BadOrder`] or `SpanTooNarrow` when the
/// range fails the corresponding check.
pub fn validate_pedal_range(min_mv: u16, max_mv: u16) -> Result<(), CalibrationError> {
    if min_mv >= max_mv {
        return Err(CalibrationError::BadOrder);
    }
    let span = max_mv - min_mv;
    if span < MIN_SPAN_MV {
        return Err(CalibrationError::SpanTooNarrow { span_mv: span });
    }
    Ok(())
}

/// Steering calibration: three points in the mV domain (D6 / D10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SteeringCalibration {
    /// Minimum millivolts (full left).
    pub min_mv: u16,
    /// Centre millivolts (wheels straight).
    pub center_mv: u16,
    /// Maximum millivolts (full right).
    pub max_mv: u16,
    /// True once a calibration was captured (wizard or import).
    pub is_set: bool,
    /// True only when the values are known to be millivolts (P9).
    pub is_millivolts: bool,
}

impl SteeringCalibration {
    /// The default range (`300..=3300` mV, centre `1800` mV), used when
    /// no sane calibration exists. Marked set and millivolts.
    #[must_use]
    pub const fn default_range() -> Self {
        Self {
            min_mv: DEFAULT_MIN_MV,
            center_mv: DEFAULT_CENTER_MV,
            max_mv: DEFAULT_MAX_MV,
            is_set: true,
            is_millivolts: true,
        }
    }

    /// Check the calibration is usable as a mapping range.
    ///
    /// # Errors
    ///
    /// Returns the matching [`CalibrationError`] when the calibration
    /// is unset, unit-less, mis-ordered, or too narrow.
    pub fn validate(&self) -> Result<(), CalibrationError> {
        if !self.is_set {
            return Err(CalibrationError::NotSet);
        }
        if !self.is_millivolts {
            return Err(CalibrationError::NotMillivolts);
        }
        validate_steering_range(self.min_mv, self.center_mv, self.max_mv)
    }

    /// True when [`validate`](Self::validate) succeeds.
    #[must_use]
    pub fn is_sane(&self) -> bool {
        self.validate().is_ok()
    }

    /// The mapping range to use: this calibration when sane, otherwise
    /// the default range. Never returns an insane range.
    #[must_use]
    pub fn effective(&self) -> Self {
        if self.is_sane() {
            *self
        } else {
            Self::default_range()
        }
    }
}

/// Pedal calibration: two points in the mV domain (D6 / D10).
/// Used for accelerator, brake, clutch, and handbrake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PedalCalibration {
    /// Minimum millivolts (pedal released).
    pub min_mv: u16,
    /// Maximum millivolts (pedal fully pressed).
    pub max_mv: u16,
    /// True once a calibration was captured (wizard or import).
    pub is_set: bool,
    /// True only when the values are known to be millivolts (P9).
    pub is_millivolts: bool,
}

impl PedalCalibration {
    /// The default range (`300..=3300` mV), used when no sane
    /// calibration exists. Marked set and millivolts.
    #[must_use]
    pub const fn default_range() -> Self {
        Self {
            min_mv: DEFAULT_MIN_MV,
            max_mv: DEFAULT_MAX_MV,
            is_set: true,
            is_millivolts: true,
        }
    }

    /// Check the calibration is usable as a mapping range.
    ///
    /// # Errors
    ///
    /// Returns the matching [`CalibrationError`] when the calibration
    /// is unset, unit-less, mis-ordered, or too narrow.
    pub fn validate(&self) -> Result<(), CalibrationError> {
        if !self.is_set {
            return Err(CalibrationError::NotSet);
        }
        if !self.is_millivolts {
            return Err(CalibrationError::NotMillivolts);
        }
        validate_pedal_range(self.min_mv, self.max_mv)
    }

    /// True when [`validate`](Self::validate) succeeds.
    #[must_use]
    pub fn is_sane(&self) -> bool {
        self.validate().is_ok()
    }

    /// The mapping range to use: this calibration when sane, otherwise
    /// the default range. Never returns an insane range.
    #[must_use]
    pub fn effective(&self) -> Self {
        if self.is_sane() {
            *self
        } else {
            Self::default_range()
        }
    }
}

/// Live capture of one axis' min/max millivolts.
///
/// Feed raw mV samples with [`observe`](Self::observe); the observed
/// extremes are kept as plain scalars (no buffer, no allocation).
/// [`finish_pedal`](Self::finish_pedal) / [`finish_steering`](Self::finish_steering)
/// validate the captured range (D10) and reject a narrow span instead
/// of returning it.
#[derive(Clone, Copy, Debug, Default)]
pub struct AxisCalibrator {
    /// Smallest observed sample, or `None` before the first sample.
    min_mv: Option<u16>,
    /// Largest observed sample, or `None` before the first sample.
    max_mv: Option<u16>,
    /// Centre sample captured with [`capture_center`](Self::capture_center),
    /// or `None` until captured.
    center_mv: Option<u16>,
    /// Number of samples observed (saturates at `u32::MAX`).
    samples: u32,
}

impl AxisCalibrator {
    /// An empty calibrator (no samples yet).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            min_mv: None,
            max_mv: None,
            center_mv: None,
            samples: 0,
        }
    }

    /// Observe one millivolt sample, updating the min/max extremes.
    pub fn observe(&mut self, mv: u16) {
        self.min_mv = Some(match self.min_mv {
            Some(min) => min.min(mv),
            None => mv,
        });
        self.max_mv = Some(match self.max_mv {
            Some(max) => max.max(mv),
            None => mv,
        });
        self.samples = self.samples.saturating_add(1);
    }

    /// Capture the centre sample (steering only: wheels straight).
    /// The last captured centre wins.
    pub fn capture_center(&mut self, mv: u16) {
        self.center_mv = Some(mv);
    }

    /// Number of samples observed so far.
    #[must_use]
    pub const fn samples(&self) -> u32 {
        self.samples
    }

    /// True once at least one sample was observed.
    #[must_use]
    pub const fn has_samples(&self) -> bool {
        self.min_mv.is_some()
    }

    /// The observed minimum, or `None` before the first sample.
    #[must_use]
    pub const fn min_mv(&self) -> Option<u16> {
        self.min_mv
    }

    /// The observed maximum, or `None` before the first sample.
    #[must_use]
    pub const fn max_mv(&self) -> Option<u16> {
        self.max_mv
    }

    /// Finish as a pedal calibration (span-checked, D10).
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError::NotSet`] with no samples, or the
    /// range error when the captured span is too narrow / mis-ordered.
    pub fn finish_pedal(&self) -> Result<PedalCalibration, CalibrationError> {
        let (Some(min_mv), Some(max_mv)) = (self.min_mv, self.max_mv) else {
            return Err(CalibrationError::NotSet);
        };
        validate_pedal_range(min_mv, max_mv)?;
        Ok(PedalCalibration {
            min_mv,
            max_mv,
            is_set: true,
            is_millivolts: true,
        })
    }

    /// Finish as a steering calibration (span-checked, D10). The
    /// centre must have been captured with
    /// [`capture_center`](Self::capture_center).
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError::NotSet`] with no samples or no
    /// centre, or the range error when the captured range is too
    /// narrow / mis-ordered.
    pub fn finish_steering(&self) -> Result<SteeringCalibration, CalibrationError> {
        let (Some(min_mv), Some(center_mv), Some(max_mv)) =
            (self.min_mv, self.center_mv, self.max_mv)
        else {
            return Err(CalibrationError::NotSet);
        };
        validate_steering_range(min_mv, center_mv, max_mv)?;
        Ok(SteeringCalibration {
            min_mv,
            center_mv,
            max_mv,
            is_set: true,
            is_millivolts: true,
        })
    }

    /// Discard all captured samples and start over.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// Percentile-guarded auto-calibration over a bounded ring buffer.
///
/// Samples are kept in a fixed `[u16; 64]` ring (no allocation, no
/// unbounded list). The mapping range is taken from low/high
/// percentiles of the buffered samples instead of the raw extremes,
/// so a few noisy spikes cannot stretch the range. Use the 5th / 95th
/// percentiles (`finish_pedal` / `finish_steering` below do exactly
/// that); the span is still checked against [`MIN_SPAN_MV`] (D10).
#[derive(Clone, Debug)]
pub struct AutoCalibrator {
    /// Ring buffer of recent samples (oldest overwritten past 64).
    samples: [u16; AUTO_CAL_SAMPLES],
    /// Number of valid entries in `samples` (`<= 64`).
    len: usize,
    /// Write position in the ring.
    head: usize,
    /// Centre sample for steering (last captured wins).
    center_mv: Option<u16>,
}

impl Default for AutoCalibrator {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoCalibrator {
    /// An empty auto-calibrator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            samples: [0; AUTO_CAL_SAMPLES],
            len: 0,
            head: 0,
            center_mv: None,
        }
    }

    /// Observe one millivolt sample into the bounded ring buffer.
    pub fn observe(&mut self, mv: u16) {
        self.samples[self.head] = mv;
        self.head = (self.head + 1) % AUTO_CAL_SAMPLES;
        if self.len < AUTO_CAL_SAMPLES {
            self.len += 1;
        }
    }

    /// Capture the centre sample (steering only: wheels straight).
    pub fn capture_center(&mut self, mv: u16) {
        self.center_mv = Some(mv);
    }

    /// Number of samples currently buffered (`<= 64`).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when no samples are buffered.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// True once the ring buffer is full (64 samples).
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.len == AUTO_CAL_SAMPLES
    }

    /// The `p`-th percentile of the buffered samples (`p` in `0..=100`,
    /// clamped), by nearest-rank on a stack-sorted copy. Returns `None`
    /// when empty.
    #[must_use]
    pub fn percentile(&self, p: u8) -> Option<u16> {
        if self.len == 0 {
            return None;
        }
        let mut sorted = self.samples;
        sorted[..self.len].sort_unstable();
        let p = usize::from(p.min(100));
        // Nearest-rank: rank 1..=len, index rank-1 (integer ceil div).
        let rank = (p * self.len).div_ceil(100).max(1);
        let idx = (rank - 1).min(self.len - 1);
        Some(sorted[idx])
    }

    /// Finish as a pedal calibration from the 5th / 95th percentiles.
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError::NotSet`] with no samples, or the
    /// range error when the percentile span is too narrow.
    pub fn finish_pedal(&self) -> Result<PedalCalibration, CalibrationError> {
        let (Some(min_mv), Some(max_mv)) = (self.percentile(5), self.percentile(95)) else {
            return Err(CalibrationError::NotSet);
        };
        validate_pedal_range(min_mv, max_mv)?;
        Ok(PedalCalibration {
            min_mv,
            max_mv,
            is_set: true,
            is_millivolts: true,
        })
    }

    /// Finish as a steering calibration from the 5th / 95th
    /// percentiles plus the captured centre.
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationError::NotSet`] with no samples or no
    /// centre, or the range error when the percentile range is too
    /// narrow / mis-ordered.
    pub fn finish_steering(&self) -> Result<SteeringCalibration, CalibrationError> {
        let (Some(min_mv), Some(max_mv)) = (self.percentile(5), self.percentile(95)) else {
            return Err(CalibrationError::NotSet);
        };
        let Some(center_mv) = self.center_mv else {
            return Err(CalibrationError::NotSet);
        };
        validate_steering_range(min_mv, center_mv, max_mv)?;
        Ok(SteeringCalibration {
            min_mv,
            center_mv,
            max_mv,
            is_set: true,
            is_millivolts: true,
        })
    }

    /// Discard all buffered samples and the centre.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AutoCalibrator, AxisCalibrator, CalUnit, CalibrationError, PedalCalibration,
        SteeringCalibration, validate_pedal_range, validate_steering_range,
    };

    #[test]
    fn golden_span_exactly_660mv_is_accepted() {
        // D10 boundary: span exactly 660 mV passes.
        assert!(validate_pedal_range(300, 960).is_ok());
        assert!(validate_steering_range(300, 630, 960).is_ok());
    }

    #[test]
    fn golden_span_659mv_is_rejected() {
        // D10 boundary: 1 mV under the minimum span is rejected.
        assert_eq!(
            validate_pedal_range(300, 959),
            Err(CalibrationError::SpanTooNarrow { span_mv: 659 })
        );
    }

    #[test]
    fn golden_unitless_calibration_is_insane() {
        // P9: unit-less (legacy v1) values are never treated as mV.
        let cal = PedalCalibration {
            min_mv: 300,
            max_mv: 3300,
            is_set: true,
            is_millivolts: false,
        };
        assert_eq!(cal.validate(), Err(CalibrationError::NotMillivolts));
        assert!(!cal.is_sane());
        // Effective range falls back to the sane default.
        assert_eq!(cal.effective(), PedalCalibration::default_range());
        assert_eq!(CalUnit::Unknown, CalUnit::default());
    }

    #[test]
    fn golden_steering_half_span_rule() {
        // Total span is fine (1000 mV) but the low half-span is 100 mV.
        assert_eq!(
            validate_steering_range(1700, 1800, 2700),
            Err(CalibrationError::HalfSpanTooNarrow { half_span_mv: 100 })
        );
        // Mis-ordered endpoints are rejected, not reordered.
        assert_eq!(
            validate_steering_range(1800, 1800, 2700),
            Err(CalibrationError::BadOrder)
        );
        assert_eq!(
            validate_pedal_range(2000, 2000),
            Err(CalibrationError::BadOrder)
        );
    }

    #[test]
    fn axis_calibrator_captures_min_max_and_validates() {
        let mut cal = AxisCalibrator::new();
        assert!(!cal.has_samples());
        assert_eq!(cal.finish_pedal(), Err(CalibrationError::NotSet));
        for mv in [1800, 300, 3300, 1200] {
            cal.observe(mv);
        }
        assert_eq!(cal.samples(), 4);
        assert_eq!(cal.min_mv(), Some(300));
        assert_eq!(cal.max_mv(), Some(3300));
        let pedal = cal.finish_pedal().expect("span 3000 mV is sane");
        assert_eq!(pedal.min_mv, 300);
        assert_eq!(pedal.max_mv, 3300);
        // No centre captured yet: steering finish fails.
        assert_eq!(cal.finish_steering(), Err(CalibrationError::NotSet));
        cal.capture_center(1800);
        let steer = cal.finish_steering().expect("steering range is sane");
        assert_eq!(steer.center_mv, 1800);
    }

    #[test]
    fn axis_calibrator_narrow_span_rejected() {
        let mut cal = AxisCalibrator::new();
        cal.observe(1000);
        cal.observe(1500);
        assert_eq!(
            cal.finish_pedal(),
            Err(CalibrationError::SpanTooNarrow { span_mv: 500 })
        );
    }

    #[test]
    fn auto_calibrator_percentile_ignores_spikes() {
        // 50 samples at the extremes plus 2 spike samples; the 5th/95th
        // percentiles must come from the bulk, not the spikes.
        let mut cal = AutoCalibrator::new();
        assert!(cal.is_empty());
        for _ in 0..25 {
            cal.observe(400);
        }
        for _ in 0..25 {
            cal.observe(3000);
        }
        cal.observe(0);
        cal.observe(3300);
        assert_eq!(cal.len(), 52);
        assert!(!cal.is_full());
        assert_eq!(cal.percentile(5), Some(400));
        assert_eq!(cal.percentile(95), Some(3000));
        let pedal = cal.finish_pedal().expect("percentile span is sane");
        assert_eq!((pedal.min_mv, pedal.max_mv), (400, 3000));
    }

    #[test]
    fn auto_calibrator_ring_is_bounded() {
        let mut cal = AutoCalibrator::new();
        for i in 0..200 {
            cal.observe(i % 3301);
        }
        assert!(cal.is_full());
        assert_eq!(cal.len(), 64);
        assert!(cal.percentile(50).is_some());
        cal.reset();
        assert!(cal.is_empty());
        assert_eq!(cal.percentile(50), None);
    }

    #[test]
    fn steering_default_range_is_sane() {
        let cal = SteeringCalibration::default_range();
        assert!(cal.is_sane());
        assert_eq!(cal.effective(), cal);
        let unset = SteeringCalibration::default();
        assert_eq!(unset.validate(), Err(CalibrationError::NotSet));
    }
}
