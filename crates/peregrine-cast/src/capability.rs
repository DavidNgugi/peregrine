//! What a renderer can play, and how a file's requirements are matched against it.
//!
//! A capability is what a device advertised; a match is whether one file's container, video
//! codec, HDR format, audio codec, and subtitle format fit inside it. The fallback paths that
//! turn a gap into a remux, a transcode, or a burn-in are a later slice (ADR 0010 §5, PG-E19-05
//! and PG-E19-10); this module answers only what a direct hand-off would need.

use peregrine_domain::{AudioCodec, HdrFormat, SubtitleFormat, VideoCodec};
use serde::{Deserialize, Serialize};

/// A container a renderer may accept.
///
/// The spelling is the one a stored capability set carries; it is deliberately not the file
/// extension, because several extensions name the same ISO base media container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    /// ISO base media file format: mp4, m4v, and mov.
    Mp4,
    /// `WebM`.
    Webm,
    /// Matroska.
    Matroska,
    /// MPEG transport stream.
    MpegTs,
    /// MPEG program stream.
    MpegPs,
    /// AVI.
    Avi,
    /// MP3.
    Mp3,
    /// Ogg.
    Ogg,
    /// WAV.
    Wav,
    /// FLAC.
    Flac,
}

/// The containers, codecs, HDR formats, audio formats, and subtitle formats one renderer says it
/// supports.
///
/// The fields are lists because matching asks only whether one value is present; a duplicated
/// entry is harmless, and the order a device advertised its support in is kept for the log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Containers the renderer can demux.
    pub containers: Vec<Container>,
    /// Video codecs the renderer can decode.
    pub video_codecs: Vec<VideoCodec>,
    /// HDR formats the renderer can display. Sdr never needs to be listed.
    pub hdr_formats: Vec<HdrFormat>,
    /// Audio codecs the renderer can decode.
    pub audio_codecs: Vec<AudioCodec>,
    /// Subtitle formats the renderer can display without the file being re-encoded.
    pub subtitle_formats: Vec<SubtitleFormat>,
}

/// The requirements one file places on a renderer.
///
/// `video_codec` and `audio_codec` are optional so that an audio renderer can be matched against
/// an audio file. `hdr` is meaningful only when there is a video codec: an audio file has no
/// dynamic range to advertise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaRequirements {
    /// The container the bytes are in.
    pub container: Container,
    /// The video codec, or None for audio-only media.
    pub video_codec: Option<VideoCodec>,
    /// The video's dynamic range. Ignored when `video_codec` is None.
    pub hdr: HdrFormat,
    /// The audio codec, or None for silent media.
    pub audio_codec: Option<AudioCodec>,
    /// The subtitle format to deliver, or None when the file carries no subtitle to show.
    pub subtitle_format: Option<SubtitleFormat>,
}

/// One requirement a renderer's capabilities do not satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityGap {
    /// The renderer cannot demux this container.
    Container(Container),
    /// The renderer cannot decode this video codec.
    VideoCodec(VideoCodec),
    /// The renderer cannot display this HDR format.
    Hdr(HdrFormat),
    /// The renderer cannot decode this audio codec.
    AudioCodec(AudioCodec),
    /// The renderer cannot display this subtitle format without a re-encode.
    Subtitles(SubtitleFormat),
}

impl Capabilities {
    /// Every requirement this renderer does not satisfy, in a stable order.
    ///
    /// The order is the order the requirements are checked — container, video, HDR, audio,
    /// subtitle — so two calls with the same input produce the same list, and a test can assert
    /// the whole result rather than a membership.
    #[must_use]
    pub fn gaps(&self, requirements: &MediaRequirements) -> Vec<CapabilityGap> {
        let mut gaps = Vec::new();
        if !self.containers.contains(&requirements.container) {
            gaps.push(CapabilityGap::Container(requirements.container));
        }
        if let Some(video) = requirements.video_codec {
            if !self.video_codecs.contains(&video) {
                gaps.push(CapabilityGap::VideoCodec(video));
            }
            // SDR is the absence of an HDR requirement, so a renderer that advertises no HDR
            // format at all still plays an SDR file. Every other format must be advertised.
            if requirements.hdr != HdrFormat::Sdr && !self.hdr_formats.contains(&requirements.hdr) {
                gaps.push(CapabilityGap::Hdr(requirements.hdr));
            }
        }
        if let Some(audio) = requirements.audio_codec
            && !self.audio_codecs.contains(&audio)
        {
            gaps.push(CapabilityGap::AudioCodec(audio));
        }
        if let Some(subtitles) = requirements.subtitle_format
            && !self.subtitle_formats.contains(&subtitles)
        {
            gaps.push(CapabilityGap::Subtitles(subtitles));
        }
        gaps
    }

    /// Whether this renderer satisfies every requirement directly.
    ///
    /// false does not mean the file cannot be shown; it means one of the fallbacks ADR 0010 §5
    /// permits — remux, transcode, or burn-in — would be needed. gaps says which.
    #[must_use]
    pub fn supports(&self, requirements: &MediaRequirements) -> bool {
        self.gaps(requirements).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a capability set from the five advertised lists.
    fn capabilities(
        containers: &[Container],
        video: &[VideoCodec],
        hdr: &[HdrFormat],
        audio: &[AudioCodec],
        subtitles: &[SubtitleFormat],
    ) -> Capabilities {
        Capabilities {
            containers: containers.to_vec(),
            video_codecs: video.to_vec(),
            hdr_formats: hdr.to_vec(),
            audio_codecs: audio.to_vec(),
            subtitle_formats: subtitles.to_vec(),
        }
    }

    /// The reference Chromecast: no Matroska, no HEVC, and no DTS, per ADR 0010's verified
    /// refinements. HDR10 and HLG are advertised; SDR needs no entry.
    fn chromecast() -> Capabilities {
        capabilities(
            &[
                Container::Mp4,
                Container::Webm,
                Container::MpegTs,
                Container::Mp3,
                Container::Ogg,
                Container::Wav,
            ],
            &[VideoCodec::H264, VideoCodec::Vp9, VideoCodec::Av1],
            &[HdrFormat::Hdr10, HdrFormat::Hlg],
            &[
                AudioCodec::Aac,
                AudioCodec::Mp3,
                AudioCodec::Opus,
                AudioCodec::Flac,
                AudioCodec::Pcm,
            ],
            &[SubtitleFormat::Vtt],
        )
    }

    /// A DLNA television that accepts Matroska and HEVC but no bitmap subtitles and no HDR.
    fn dlna_tv() -> Capabilities {
        capabilities(
            &[
                Container::Mp4,
                Container::Matroska,
                Container::MpegTs,
                Container::MpegPs,
                Container::Avi,
            ],
            &[VideoCodec::H264, VideoCodec::H265],
            &[],
            &[
                AudioCodec::Aac,
                AudioCodec::Ac3,
                AudioCodec::Eac3,
                AudioCodec::Dts,
                AudioCodec::Mp3,
                AudioCodec::Flac,
                AudioCodec::Pcm,
            ],
            &[
                SubtitleFormat::Srt,
                SubtitleFormat::Vtt,
                SubtitleFormat::Ass,
                SubtitleFormat::Ssa,
                SubtitleFormat::Sub,
            ],
        )
    }

    /// A DLNA audio renderer: containers and codecs for sound, no video, no subtitles.
    fn dlna_audio() -> Capabilities {
        capabilities(
            &[
                Container::Mp3,
                Container::Flac,
                Container::Wav,
                Container::Ogg,
            ],
            &[],
            &[],
            &[
                AudioCodec::Mp3,
                AudioCodec::Flac,
                AudioCodec::Pcm,
                AudioCodec::Aac,
            ],
            &[],
        )
    }

    fn film(
        container: Container,
        video: VideoCodec,
        hdr: HdrFormat,
        audio: AudioCodec,
        subtitle: Option<SubtitleFormat>,
    ) -> MediaRequirements {
        MediaRequirements {
            container,
            video_codec: Some(video),
            hdr,
            audio_codec: Some(audio),
            subtitle_format: subtitle,
        }
    }

    #[test]
    fn a_chromecast_direct_plays_an_h264_aac_mp4_with_webvtt() {
        let requirements = film(
            Container::Mp4,
            VideoCodec::H264,
            HdrFormat::Sdr,
            AudioCodec::Aac,
            Some(SubtitleFormat::Vtt),
        );

        assert!(chromecast().supports(&requirements));
        assert_eq!(chromecast().gaps(&requirements), Vec::new());
    }

    #[test]
    fn a_chromecast_names_every_gap_in_a_matroska_hevc_dts_file_with_pgs() {
        // The fixture ADR 0010 says must not be attempted directly: Matroska is absent, first-
        // and second-generation devices have no HEVC, DTS is never supported, and PGS is bitmap.
        let requirements = film(
            Container::Matroska,
            VideoCodec::H265,
            HdrFormat::Sdr,
            AudioCodec::Dts,
            Some(SubtitleFormat::Pgs),
        );

        assert!(!chromecast().supports(&requirements));
        assert_eq!(
            chromecast().gaps(&requirements),
            vec![
                CapabilityGap::Container(Container::Matroska),
                CapabilityGap::VideoCodec(VideoCodec::H265),
                CapabilityGap::AudioCodec(AudioCodec::Dts),
                CapabilityGap::Subtitles(SubtitleFormat::Pgs),
            ]
        );
    }

    #[test]
    fn sdr_needs_no_hdr_entry_and_hdr10_needs_one() {
        let sdr = film(
            Container::Mp4,
            VideoCodec::H264,
            HdrFormat::Sdr,
            AudioCodec::Aac,
            None,
        );
        let hdr10 = MediaRequirements {
            hdr: HdrFormat::Hdr10,
            ..sdr
        };

        // The DLNA fixture advertises no HDR format at all.
        assert!(dlna_tv().supports(&sdr));
        assert_eq!(
            dlna_tv().gaps(&hdr10),
            vec![CapabilityGap::Hdr(HdrFormat::Hdr10)]
        );
        // The Chromecast advertised HDR10, so the same file fits it.
        assert!(chromecast().supports(&hdr10));
    }

    #[test]
    fn a_dlna_tv_accepts_matroska_hevc_and_refuses_pgs() {
        let accepted = film(
            Container::Matroska,
            VideoCodec::H265,
            HdrFormat::Sdr,
            AudioCodec::Eac3,
            Some(SubtitleFormat::Srt),
        );
        assert!(dlna_tv().supports(&accepted));

        let bitmap = MediaRequirements {
            subtitle_format: Some(SubtitleFormat::Pgs),
            ..accepted
        };
        assert_eq!(
            dlna_tv().gaps(&bitmap),
            vec![CapabilityGap::Subtitles(SubtitleFormat::Pgs)]
        );
    }

    #[test]
    fn an_audio_renderer_matches_audio_and_refuses_a_film() {
        let audio = MediaRequirements {
            container: Container::Flac,
            video_codec: None,
            hdr: HdrFormat::Sdr,
            audio_codec: Some(AudioCodec::Flac),
            subtitle_format: None,
        };
        assert!(dlna_audio().supports(&audio));

        let film_requirements = film(
            Container::Mp4,
            VideoCodec::H264,
            HdrFormat::Sdr,
            AudioCodec::Aac,
            None,
        );
        assert_eq!(
            dlna_audio().gaps(&film_requirements),
            vec![
                CapabilityGap::Container(Container::Mp4),
                CapabilityGap::VideoCodec(VideoCodec::H264),
            ]
        );
    }

    #[test]
    fn a_renderer_that_advertised_nothing_supports_nothing() {
        let nothing = Capabilities::default();
        let requirements = MediaRequirements {
            container: Container::Mp3,
            video_codec: None,
            hdr: HdrFormat::Sdr,
            audio_codec: Some(AudioCodec::Mp3),
            subtitle_format: None,
        };
        assert_eq!(
            nothing.gaps(&requirements),
            vec![
                CapabilityGap::Container(Container::Mp3),
                CapabilityGap::AudioCodec(AudioCodec::Mp3),
            ]
        );
    }
}
