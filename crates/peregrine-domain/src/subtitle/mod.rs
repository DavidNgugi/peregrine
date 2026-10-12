//! Subtitle timing and text formats.
//!
//! Two responsibilities, split because they fail differently and are read by different
//! callers:
//!
//! - [`timing`] is the arithmetic — a signed offset, a frame rate, and the conversions
//!   between them (`PG-E4-14`). It is pure numbers and is where the drift hazards live.
//! - [`format`] is the document model and the SRT, `WebVTT`, and ASS writers, with the
//!   readers the round-trip properties need (`PG-E4-15`). It is text in and text out.
//!
//! What is *not* here is as deliberate: no charset detection (`PG-E4-17-06`), no file
//! reading, and no embedded-stream extraction — extraction is ffmpeg's job behind the port
//! in `docs/adr/0014-ffmpeg-build-and-transcode.md`, and the adapter asks ffmpeg for a text
//! stream that this module then parses.

pub mod format;
pub mod timing;

pub use format::{AssFields, FormatError, SubtitleCue, SubtitleDocument, SubtitleTiming};
pub use timing::{Conversion, FrameRate, MAX_MILLIS, MicrosPerFrame, Millis, TimingError};
