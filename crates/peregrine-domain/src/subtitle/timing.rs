//! Subtitle timing: a signed offset, a frame rate, and the conversions between them.
//!
//! Everything a subtitle needs to be shifted or re-timed is integer arithmetic over
//! [`Millis`] and [`MicrosPerFrame`]. Nothing here touches a file, a clock, or the
//! network; a caller that has read a subtitle file hands this module text, and gets
//! text back.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::format::SubtitleTiming;

/// The whole-millisecond range a subtitle may be shifted or scaled over: ±999 hours.
///
/// The bound is the one the timestamp formats can spell — `HHH:MM:SS.mmm` reaches `999`
/// hours and no further — so an operation that would leave it is clamped to a time the
/// format can still write rather than silently wrapping.
pub const MAX_MILLIS: i64 = 999 * 3_600_000;

/// The fastest accepted frame rate, in microseconds per frame: 1 000 fps.
///
/// The floor is not about plausibility. `micros_per_frame` is what multiplication has to
/// use to stay exact, and a frame shorter than a microsecond cannot be represented at
/// all, so a rate above 1 000 fps has no value here rather than a wrong one.
pub const MIN_MICROS_PER_FRAME: u32 = 1_000;
/// The slowest accepted frame rate: one frame per second.
pub const MAX_MICROS_PER_FRAME: u32 = 1_000_000;

/// A signed duration in milliseconds.
///
/// Signed because a timing correction is applied in both directions: `+n` delays the
/// subtitles, `-n` advances them, which is the sign `subtitle_track.offset_ms` stores.
/// Every operation saturates at ±[`MAX_MILLIS`] rather than wrapping, so a nonsensical
/// offset moves a cue to the edge of the range a format can write instead of to a
/// different part of the film.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Millis(i64);

impl Millis {
    /// No duration at all.
    pub const ZERO: Self = Self(0);
    /// The earliest representable time.
    pub const MIN: Self = Self(-MAX_MILLIS);
    /// The latest representable time.
    pub const MAX: Self = Self(MAX_MILLIS);

    /// Builds a duration, clamping it to [`Millis::MIN`] or [`Millis::MAX`].
    #[must_use]
    pub const fn from_millis(value: i64) -> Self {
        Self(clamp(value))
    }

    /// The duration in milliseconds.
    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// Whether the duration is exactly zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// This duration moved later by `offset`, saturating at [`Millis::MAX`].
    ///
    /// "Later" is the direction a positive offset has everywhere else in the domain:
    /// `subtitle_track.offset_ms` is signed, and a positive value delays the subtitles.
    #[must_use]
    pub const fn delayed_by(self, offset: Self) -> Self {
        Self(clamp(self.0.saturating_add(offset.0)))
    }

    /// This duration moved earlier by `offset`, saturating at [`Millis::MIN`].
    ///
    /// The exact inverse of [`Millis::delayed_by`], which is the property the timing
    /// acceptance test states: advancing by the same amount a cue was delayed restores it,
    /// unless either end of the move hit a bound.
    #[must_use]
    pub const fn advanced_by(self, offset: Self) -> Self {
        Self(clamp(self.0.saturating_sub(offset.0)))
    }
}

/// Clamps a raw millisecond count into the range a subtitling format can write.
const fn clamp(value: i64) -> i64 {
    if value > MAX_MILLIS {
        MAX_MILLIS
    } else if value < -MAX_MILLIS {
        -MAX_MILLIS
    } else {
        value
    }
}

impl fmt::Display for Millis {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ms", self.0)
    }
}

/// Why a frame rate was refused.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum TimingError {
    /// A frame rate was zero, negative, or not finite.
    #[error("a frame rate must be a finite, positive number of frames per second, not {value}")]
    InvalidFrameRate {
        /// The refused value.
        value: f64,
    },
    /// A frame rate fell outside the range the microsecond arithmetic can represent.
    #[error("a frame rate of {value} fps is outside the supported range of 1 to 1 000 fps")]
    UnsupportedFrameRate {
        /// The refused value.
        value: f64,
    },
}

/// The duration of one frame, in exact microseconds.
///
/// The conversion arithmetic needs a whole number of microseconds per frame: a scale
/// factor as an `f64` accumulates a rounding error at every step, and a two-hour film has
/// enough steps — 172 800 frames at 24 fps — for a last-bit error in the factor to move a
/// cue by a visible amount. Storing microseconds makes the scale a ratio of two integers,
/// so the only rounding is the one division at the end of [`Conversion::convert`], whose
/// rule is stated there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MicrosPerFrame(u32);

impl MicrosPerFrame {
    /// Builds a frame duration from whole microseconds.
    ///
    /// # Errors
    ///
    /// [`TimingError::UnsupportedFrameRate`] outside one to 1 000 fps.
    pub fn from_micros(value: u32) -> Result<Self, TimingError> {
        if (MIN_MICROS_PER_FRAME..=MAX_MICROS_PER_FRAME).contains(&value) {
            Ok(Self(value))
        } else {
            Err(TimingError::UnsupportedFrameRate {
                value: 1_000_000.0 / f64::from(value),
            })
        }
    }

    /// Builds a frame duration from a rate in frames per second, rounding to the nearest
    /// microsecond.
    ///
    /// The rounding is deliberate and is not the drift hazard: it happens once, when the
    /// rate is read, so at worst every converted timestamp is off by the same fraction of a
    /// microsecond. Re-rounding per cue would be the accumulating mistake.
    ///
    /// # Errors
    ///
    /// [`TimingError::InvalidFrameRate`] for a rate that is not a finite, positive number,
    /// and [`TimingError::UnsupportedFrameRate`] for one whose frame is shorter than a
    /// microsecond or longer than a second.
    pub fn from_fps(fps: f64) -> Result<Self, TimingError> {
        if !fps.is_finite() || fps <= 0.0 {
            return Err(TimingError::InvalidFrameRate { value: fps });
        }
        let micros = 1_000_000.0 / fps;
        let rounded = round_to_u32(micros);
        Self::from_micros(rounded).map_err(|_| TimingError::UnsupportedFrameRate { value: fps })
    }

    /// The frame duration in microseconds.
    #[must_use]
    pub const fn as_micros(self) -> u32 {
        self.0
    }
}

/// Rounds a positive `f64` to the nearest whole number, saturating rather than wrapping.
fn round_to_u32(value: f64) -> u32 {
    let rounded = value.round();
    if rounded <= 0.0 {
        0
    } else if rounded >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        // Both bounds above hold the value inside `u32`, so the narrowing is exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let narrowed = rounded as u32;
        narrowed
    }
}

/// A frame rate, held as the whole-microsecond duration of one frame.
///
/// The rate is not stored as frames per second because the arithmetic must not be: see
/// [`MicrosPerFrame`]. The rates a library meets are constants, so a caller does not have
/// to know that 23.976 is really `24000/1001` or that PAL speedup is exactly `25 ÷ 24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameRate(MicrosPerFrame);

impl FrameRate {
    /// `24000/1001` — the film rate the industry calls 23.976.
    pub const FILM: Self = Self(MicrosPerFrame(41_708));
    /// Exact 24 fps.
    pub const FILM_EXACT: Self = Self(MicrosPerFrame(41_667));
    /// `30000/1001` — the NTSC rate broadcasters call 29.97.
    pub const NTSC: Self = Self(MicrosPerFrame(33_367));
    /// Exact 25 fps, which is both PAL and the rate a film is played at after PAL speedup.
    pub const PAL: Self = Self(MicrosPerFrame(40_000));
    /// Exact 30 fps.
    pub const NTSC_EXACT: Self = Self(MicrosPerFrame(33_333));
    /// Exact 50 fps.
    pub const PAL_DOUBLE: Self = Self(MicrosPerFrame(20_000));
    /// `60000/1001` — 59.94 fps.
    pub const NTSC_DOUBLE: Self = Self(MicrosPerFrame(16_683));

    /// Builds a rate from a rate in frames per second.
    ///
    /// # Errors
    ///
    /// [`TimingError::InvalidFrameRate`] or [`TimingError::UnsupportedFrameRate`].
    pub fn from_fps(fps: f64) -> Result<Self, TimingError> {
        Ok(Self(MicrosPerFrame::from_fps(fps)?))
    }

    /// Builds a rate from the duration of one frame in microseconds.
    ///
    /// # Errors
    ///
    /// [`TimingError::UnsupportedFrameRate`] outside one to 1 000 fps.
    pub fn from_micros_per_frame(value: u32) -> Result<Self, TimingError> {
        Ok(Self(MicrosPerFrame::from_micros(value)?))
    }

    /// The duration of one frame.
    #[must_use]
    pub const fn micros_per_frame(self) -> MicrosPerFrame {
        self.0
    }

    /// The rate in frames per second, rounded to three decimal places.
    ///
    /// A display value, not an input to arithmetic: `24000/1001` fps prints as `23.976`.
    #[must_use]
    pub fn as_fps(self) -> f64 {
        let micros = f64::from(self.0.as_micros());
        ((1_000_000.0 / micros) * 1000.0).round() / 1000.0
    }
}

/// A conversion of a subtitle's timing from one frame rate to another.
///
/// The two cases this exists for are the two a library actually meets:
///
/// - **Frame-rate conversion**, when a release's subtitles were timed against a different
///   rate than the copy being watched — 23.976 fps subtitles over a 25 fps print, or the
///   reverse.
/// - **PAL speedup**, which is the same arithmetic with a known pair: a film shot at 24 fps
///   plays 4.096 % faster at 25 fps, so subtitles timed for the film must be stretched by
///   `24000/1001 ÷ 25` when they are attached to the sped-up print.
///   [`Conversion::pal_speedup`] and [`Conversion::pal_slowdown`] spell that pair out so no
///   caller hard-codes it.
///
/// The conversion is a scale about zero: a cue at 00:00:00 stays there, and the error grows
/// with the timestamp because the drift is proportional to it. That is the property a user
/// notices — the subtitle ninety minutes in is the one that looks wrong — and it is why the
/// rounding rule below is a single division rather than a per-frame step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Conversion {
    from: FrameRate,
    to: FrameRate,
}

impl Conversion {
    /// The conversion from one frame rate to another.
    #[must_use]
    pub const fn new(from: FrameRate, to: FrameRate) -> Self {
        Self { from, to }
    }

    /// The conversion that attaches subtitles timed for `24000/1001` fps to a 25 fps print.
    #[must_use]
    pub const fn pal_speedup() -> Self {
        Self::new(FrameRate::FILM, FrameRate::PAL)
    }

    /// The conversion that attaches subtitles timed for a 25 fps print back to
    /// `24000/1001` fps. The exact inverse of [`Conversion::pal_speedup`].
    #[must_use]
    pub const fn pal_slowdown() -> Self {
        Self::new(FrameRate::PAL, FrameRate::FILM)
    }

    /// The rate being converted from.
    #[must_use]
    pub const fn from(self) -> FrameRate {
        self.from
    }

    /// The rate being converted to.
    #[must_use]
    pub const fn to(self) -> FrameRate {
        self.to
    }

    /// The inverse conversion.
    #[must_use]
    pub const fn reversed(self) -> Self {
        Self {
            from: self.to,
            to: self.from,
        }
    }

    /// Scales one timestamp.
    ///
    /// The scale is the exact ratio `from_micros : to_micros` applied to the microsecond
    /// value of the timestamp, divided once at the end, rounding half away from zero.
    /// Rounding once is the whole point: converting frame by frame would round at every
    /// frame and hand back an error that grows with the file's length, while a single
    /// division's error is bounded by half a millisecond no matter how long the film is.
    /// Rounding half *away from zero* rather than down keeps the conversion symmetric — a
    /// cue a second before zero and its mirror after it move by exactly the same amount.
    ///
    /// The result saturates at ±[`MAX_MILLIS`] rather than wrapping.
    #[must_use]
    pub fn convert(self, timestamp: Millis) -> Millis {
        // A timestamp anywhere near the bounds needs no arithmetic: no reachable ratio can
        // bring it back inside the range, and multiplying first would overflow the i128.
        if timestamp.0 >= MAX_MILLIS {
            return Millis::MAX;
        }
        if timestamp.0 <= -MAX_MILLIS {
            return Millis::MIN;
        }
        let numerator = i128::from(timestamp.0) * i128::from(self.from.0.as_micros());
        let denominator = i128::from(self.to.0.as_micros());
        let rounded = div_round_half_away(numerator, denominator);
        let bounded = rounded.clamp(i128::from(-MAX_MILLIS), i128::from(MAX_MILLIS));
        // The clamp holds the value inside ±3.6e9, so the narrowing is exact.
        #[allow(clippy::cast_possible_truncation)]
        let narrowed = bounded as i64;
        Millis::from_millis(narrowed)
    }

    /// Scales a whole cue, start and end together.
    ///
    /// Both ends go through the same conversion, so a cue's length scales with its
    /// position and no rounding can make it invert or collapse.
    #[must_use]
    pub fn convert_timing(self, timing: SubtitleTiming) -> SubtitleTiming {
        SubtitleTiming::new(self.convert(timing.start()), self.convert(timing.end()))
    }
}

/// Divides two integers, rounding half away from zero.
///
/// Truncating division rounds towards zero, so shifting a negative numerator down by half a
/// denominator before dividing gives `1/2 -> 1` and `-1/2 -> -1`: the same distance from
/// zero on both sides. `div_euclid` would round towards negative infinity instead and make
/// the conversion asymmetric, and shifting both signs the same way would undo the rounding
/// on one of them.
fn div_round_half_away(numerator: i128, denominator: i128) -> i128 {
    debug_assert!(denominator > 0, "a frame duration is always positive");
    if numerator < 0 {
        (numerator - denominator / 2) / denominator
    } else {
        (numerator + denominator / 2) / denominator
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn millis(value: i64) -> Millis {
        Millis::from_millis(value)
    }

    fn frames(fps: f64) -> FrameRate {
        FrameRate::from_fps(fps).expect("the fixture rate is inside the supported range")
    }

    #[test]
    fn the_common_rates_are_the_published_ones() {
        assert_eq!(FrameRate::FILM.as_fps(), 23.976);
        assert_eq!(FrameRate::PAL.as_fps(), 25.0);
        assert_eq!(FrameRate::NTSC.as_fps(), 29.97);
        assert_eq!(FrameRate::PAL.micros_per_frame().as_micros(), 40_000);
        assert_eq!(FrameRate::FILM.micros_per_frame().as_micros(), 41_708);
    }

    #[test]
    fn an_offset_and_its_reverse_are_the_identity() {
        for timestamp in [0, 1, -1, 250, -250, 7_200_000, -7_200_000] {
            for offset in [0, 1, -1, 250, -250, 1_000_000] {
                let shifted = millis(timestamp).delayed_by(millis(offset));
                assert_eq!(
                    shifted.advanced_by(millis(offset)),
                    millis(timestamp),
                    "{timestamp} ms delayed then advanced by {offset} ms"
                );
            }
        }
    }

    #[test]
    fn a_cue_can_be_advanced_before_zero_and_come_back() {
        let timestamp = millis(500);
        let before_zero = timestamp.advanced_by(millis(1_500));
        assert_eq!(before_zero.as_millis(), -1_000);
        assert_eq!(before_zero.delayed_by(millis(1_500)), timestamp);
    }

    #[test]
    fn an_offset_saturates_at_the_range_the_timestamp_format_can_write() {
        assert_eq!(Millis::MAX.as_millis(), MAX_MILLIS);
        assert_eq!(
            millis(MAX_MILLIS).delayed_by(millis(1)),
            Millis::MAX,
            "past 999 hours there is no timestamp to write"
        );
        assert_eq!(millis(-MAX_MILLIS).advanced_by(millis(1)), Millis::MIN);
        assert_eq!(
            millis(i64::MIN).delayed_by(millis(1)).as_millis(),
            -MAX_MILLIS + 1,
            "a timestamp at the clamp is a real value, so delaying it moves it"
        );
        assert_eq!(
            millis(i64::MAX).delayed_by(millis(i64::MAX)),
            Millis::MAX,
            "the clamp happens after the addition saturates"
        );
        assert_eq!(
            millis(i64::MIN),
            Millis::MIN,
            "an out-of-range value is clamped where it enters the domain type"
        );
        assert_eq!(
            millis(i64::MIN).advanced_by(millis(i64::MIN)),
            Millis::ZERO,
            "two clamped minimums subtract to zero without overflowing i64"
        );
    }

    #[test]
    fn a_zero_offset_leaves_every_timestamp_alone() {
        assert!(Millis::ZERO.is_zero());
        assert_eq!(millis(-1).as_millis(), -1);
        assert_eq!(millis(1).delayed_by(Millis::ZERO), millis(1));
        assert_eq!(millis(1).advanced_by(Millis::ZERO), millis(1));
    }

    #[test]
    fn a_frame_rate_must_be_finite_and_positive() {
        assert_eq!(
            FrameRate::from_fps(0.0),
            Err(TimingError::InvalidFrameRate { value: 0.0 })
        );
        assert_eq!(
            FrameRate::from_fps(-1.0),
            Err(TimingError::InvalidFrameRate { value: -1.0 })
        );
        assert!(matches!(
            FrameRate::from_fps(f64::NAN),
            Err(TimingError::InvalidFrameRate { .. })
        ));
        assert!(FrameRate::from_fps(f64::INFINITY).is_err());
        assert!(
            FrameRate::from_fps(0.5).is_err(),
            "a two-second frame is not a video rate"
        );
        assert!(
            FrameRate::from_fps(2_000.0).is_err(),
            "a sub-microsecond frame has no value"
        );
        assert!(
            FrameRate::from_micros_per_frame(40_000) == Ok(FrameRate::PAL),
            "25 fps is exactly 40 000 microseconds a frame"
        );
    }

    #[test]
    fn a_frame_duration_rounds_once_when_it_is_read() {
        assert_eq!(frames(23.976).micros_per_frame().as_micros(), 41_708);
        assert_eq!(frames(24.0).micros_per_frame().as_micros(), 41_667);
        assert_eq!(frames(25.0).micros_per_frame().as_micros(), 40_000);
        assert_eq!(frames(29.97).micros_per_frame().as_micros(), 33_367);
        assert_eq!(FrameRate::from_fps(1.0).map(FrameRate::as_fps), Ok(1.0));
    }

    #[test]
    fn pal_speedup_stretches_by_the_published_ratio() {
        let conversion = Conversion::pal_speedup();
        assert_eq!(
            conversion.convert(millis(0)),
            millis(0),
            "the scale is about zero"
        );

        // A 25 fps frame lasts 40 ms but carries 41.708 ms of film time, so a cue timed
        // against the film moves later on the sped-up print by that ratio.
        assert_eq!(conversion.convert(millis(1)), millis(1));
        assert_eq!(conversion.convert(millis(1_000)), millis(1_043));

        // The hazard the exact ratio exists for: at two hours a rounded 1.0427 factor and
        // the exact one disagree, and a naive 0.96 factor is minutes out.
        let two_hours = millis(2 * 3_600_000);
        let scaled = conversion.convert(two_hours);
        assert_eq!(scaled.as_millis(), 7_507_440);
        assert_eq!(
            conversion.reversed().convert(scaled).as_millis(),
            two_hours.as_millis(),
            "half a millisecond of rounding is the only error at two hours"
        );
    }

    #[test]
    fn a_conversion_rounds_symmetrically_about_zero() {
        let conversion = Conversion::pal_speedup();
        for timestamp in [1, 7, 1_000, 1_001, 12_345_678] {
            assert_eq!(
                conversion.convert(millis(-timestamp)).as_millis(),
                -conversion.convert(millis(timestamp)).as_millis(),
                "a cue before zero and its mirror move by the same amount"
            );
        }
        // 41 708 / 40 000 of 2 ms is 2.0854, which rounds away from zero to 2 on both sides.
        assert_eq!(conversion.convert(millis(2)).as_millis(), 2);
        assert_eq!(conversion.convert(millis(-2)).as_millis(), -2);
    }

    #[test]
    fn a_conversion_saturates_rather_than_wrapping() {
        let conversion = Conversion::new(FrameRate::PAL, FrameRate::FILM);
        assert_eq!(conversion.convert(Millis::MAX), Millis::MAX);
        assert_eq!(conversion.convert(Millis::MIN), Millis::MIN);
        assert_eq!(
            Conversion::pal_speedup().convert(millis(MAX_MILLIS)),
            Millis::MAX
        );
    }

    #[test]
    fn an_identity_conversion_is_exact() {
        let conversion = Conversion::new(FrameRate::FILM, FrameRate::FILM);
        for timestamp in [0, 1, -1, 123_456_789, -123_456_789, MAX_MILLIS, -MAX_MILLIS] {
            assert_eq!(conversion.convert(millis(timestamp)), millis(timestamp));
        }
    }

    /// The largest timestamp a conversion round trip is exact for.
    ///
    /// A PAL speedup multiplies a timestamp by `41 708 / 40 000`, which is 1.0427×, so a
    /// timestamp within 4 % of [`MAX_MILLIS`] saturates on the way out and one within 4 % of
    /// the minimum saturates on the way back: this bound leaves room for both ends of the
    /// trip, which is why the round-trip properties stop here rather than at the format's
    /// own limit.
    const ROUND_TRIP_LIMIT: i64 = MAX_MILLIS * 40 / 100;

    #[test]
    fn a_cue_scales_at_both_ends() {
        let timing = SubtitleTiming::new(millis(10_000), millis(12_000));
        let scaled = Conversion::pal_speedup().convert_timing(timing);
        assert_eq!(
            scaled.start(),
            Conversion::pal_speedup().convert(millis(10_000))
        );
        assert_eq!(
            scaled.end(),
            Conversion::pal_speedup().convert(millis(12_000))
        );
        assert!(
            scaled.start() < scaled.end(),
            "a scaled cue keeps its order"
        );
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(div_round_half_away(1, 2), 1);
        assert_eq!(div_round_half_away(-1, 2), -1);
        assert_eq!(div_round_half_away(1, 3), 0);
        assert_eq!(div_round_half_away(-1, 3), 0);
        assert_eq!(div_round_half_away(2, 3), 1);
        assert_eq!(div_round_half_away(-2, 3), -1);
        assert_eq!(div_round_half_away(3, 2), 2);
        assert_eq!(div_round_half_away(-3, 2), -2);
        assert_eq!(div_round_half_away(0, 7), 0);
    }

    proptest! {
        // The acceptance property, over generated values rather than only fixtures:
        // offsetting by +n and then by -n is the identity. "Offset" here is a Millis, which
        // is the clamped domain type, so every offset a caller can construct is covered.
        #[test]
        fn delaying_then_advancing_restores_every_timestamp(
            timestamp in -399_000_000i64..=399_000_000,
            offset in -399_000_000i64..=399_000_000,
        ) {
            // Each input is a quarter of the representable range, so neither the delay nor
            // the advance can reach a bound: the identity is being tested, not saturation,
            // and the boundary tests above cover saturation explicitly.
            let start = millis(timestamp);
            let forward = start.delayed_by(millis(offset));
            prop_assert_eq!(forward.advanced_by(millis(offset)), start);
        }

        // A conversion may only differ from the exact scale by half a millisecond: one
        // division, one rounding. The scale is `from_micros / to_micros`.
        #[test]
        fn a_conversion_is_never_off_by_more_than_half_a_millisecond(
            timestamp in -2_400_000_000i64..=2_400_000_000,
        ) {
            let conversion = Conversion::pal_speedup();
            let scaled = conversion.convert(millis(timestamp));
            let exact = 41_708.0 * (timestamp as f64) / 40_000.0;
            prop_assert!(
                (scaled.as_millis() as f64 - exact).abs() <= 0.5 + f64::EPSILON * exact.abs(),
                "{} ms scaled to {} ms, exact {exact}",
                timestamp,
                scaled.as_millis()
            );
        }

        // A round trip through a conversion is bounded by one millisecond, which is what "no
        // accumulated drift" means: the bound does not grow with the timestamp.
        #[test]
        fn a_conversion_round_trip_stays_within_one_millisecond(
            timestamp in -ROUND_TRIP_LIMIT..=ROUND_TRIP_LIMIT,
        ) {
            let conversion = Conversion::pal_speedup();
            let restored = conversion.reversed().convert(conversion.convert(millis(timestamp)));
            prop_assert!(
                (restored.as_millis() - timestamp).abs() <= 1,
                "{timestamp} ms came back as {} ms",
                restored.as_millis()
            );
        }

        // The arithmetic is total: no input reaches a panic, an overflow, or a NaN.
        #[test]
        fn the_timing_arithmetic_never_panics(
            timestamp in any::<i64>(),
            offset in any::<i64>(),
            fps in any::<f64>(),
        ) {
            let start = millis(timestamp);
            let _ = start.delayed_by(millis(offset)).advanced_by(millis(offset));
            let _ = FrameRate::from_fps(fps);
            let _ = Conversion::pal_speedup().convert(start);
        }
    }
}
