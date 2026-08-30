//! Declarative composition-spec types and bounds validation.
//!
//! Every field here is data, never a shell fragment: nothing in this module
//! ever becomes a raw `ffmpeg` flag or filesystem path directly. The
//! filtergraph builder (`graph.rs`, arriving in a later task) is the only
//! place that reads a [`CompositionSpec`] to build a command.
//!
//! `graph.rs` now consumes these types to build `ffmpeg` argument vectors,
//! but nothing yet calls `graph::build_ffmpeg_args` outside its own tests:
//! `Toolset::call` still rejects every request until `exec.rs` lands and
//! wires the whole pipeline together. Until then the compiler sees this
//! whole chain (spec types -> `graph.rs` -> unreachable) as dead code, so
//! `dead_code` stays muted here for that interim stretch.
#![allow(dead_code)]

use finstack_ai_kernel::ArtifactRef;
use serde::{Deserialize, Serialize};

/// Declarative video composition request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompositionSpec {
    /// Spec schema version. Only `1` is accepted.
    pub(crate) version: u32,
    /// Ordered input clips, concatenated in order.
    pub(crate) clips: Vec<ClipSpec>,
    /// Optional per-boundary transitions, one fewer than `clips`.
    pub(crate) transitions: Option<Vec<TransitionSpec>>,
    /// Optional replacement or mixed-in audio track.
    pub(crate) audio: Option<AudioSpec>,
    /// Optional burned-in or muxed subtitle track.
    pub(crate) subtitles: Option<SubtitlesSpec>,
    /// Output container and encoding hints.
    pub(crate) output: OutputSpec,
}

/// One input clip and its optional trim window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClipSpec {
    /// Stored artifact holding the clip bytes.
    pub(crate) artifact: ArtifactRef,
    /// Optional trim window applied before concatenation.
    pub(crate) trim: Option<TrimSpec>,
}

/// Trim window in seconds, relative to the clip's own timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TrimSpec {
    /// Inclusive start offset in seconds.
    pub(crate) start_s: f64,
    /// Exclusive end offset in seconds. Must be greater than `start_s`.
    pub(crate) end_s: f64,
}

/// Transition style applied at a clip boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransitionKind {
    /// Hard cut; `duration_s` is ignored.
    Cut,
    /// Cross-dissolve between the two clips.
    Crossfade,
    /// Fade through black between the two clips.
    FadeToBlack,
}

/// Transition applied at one clip boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransitionSpec {
    /// Transition style.
    #[serde(rename = "type")]
    pub(crate) kind: TransitionKind,
    /// Transition duration in seconds. Defaults to `0.5` when omitted.
    pub(crate) duration_s: Option<f64>,
}

/// How a replacement or additional audio track is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AudioMode {
    /// Replace every clip's original audio with this track.
    Replace,
    /// Mix this track in alongside the clips' original audio.
    Mix,
}

/// Replacement or mixed-in audio track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioSpec {
    /// Stored artifact holding the audio bytes.
    pub(crate) artifact: ArtifactRef,
    /// How the track combines with the clips' original audio.
    pub(crate) mode: AudioMode,
    /// Optional gain adjustment in decibels, clamped to `[-60, 12]`.
    pub(crate) gain_db: Option<f64>,
}

/// How a subtitle track is applied to the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SubtitleMode {
    /// Render subtitles into the video stream.
    BurnIn,
    /// Mux subtitles as a separate stream (mp4 `mov_text` only).
    Mux,
}

/// Subtitle track and how it is applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtitlesSpec {
    /// Stored artifact holding the subtitle bytes (SRT).
    pub(crate) artifact: ArtifactRef,
    /// How the subtitles are applied.
    pub(crate) mode: SubtitleMode,
    /// Optional burn-in styling. Ignored for `mux`.
    pub(crate) style: Option<SubtitleStyle>,
}

/// Burn-in subtitle styling bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtitleStyle {
    /// Font size in points, clamped to `[8, 96]`.
    pub(crate) font_size: Option<u32>,
    /// Vertical margin in pixels, clamped to `<= 400`.
    pub(crate) margin_v: Option<u32>,
}

/// Output container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Container {
    /// MPEG-4 container.
    Mp4,
    /// `WebM` container.
    Webm,
}

/// Output container and encoding hints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutputSpec {
    /// Output container.
    pub(crate) container: Container,
    /// Optional `WIDTHxHEIGHT` resolution, bounded to `16..=7680` by
    /// `16..=4320`.
    pub(crate) resolution: Option<String>,
    /// Optional frame rate, clamped to `[1, 120]`.
    pub(crate) fps: Option<u32>,
}

/// Validate bounds a caller-supplied [`CompositionSpec`] must satisfy before
/// it can be translated into an `ffmpeg` invocation.
///
/// # Errors
///
/// Returns a stable, non-secret reason string on the first violated bound.
pub(crate) fn validate_spec(spec: &CompositionSpec) -> Result<(), &'static str> {
    if spec.version != 1 {
        return Err("composition spec version must be 1");
    }
    if spec.clips.is_empty() || spec.clips.len() > 64 {
        return Err("composition needs between 1 and 64 clips");
    }
    if let Some(transitions) = &spec.transitions {
        if !transitions.is_empty() && transitions.len() != spec.clips.len().saturating_sub(1) {
            return Err("transition count must be clip count minus one");
        }
        for transition in transitions {
            let duration = transition.duration_s.unwrap_or(0.5);
            let needs_duration = !matches!(transition.kind, TransitionKind::Cut);
            if needs_duration && !(0.05..=5.0).contains(&duration) {
                return Err("transition duration must be between 0.05 and 5 seconds");
            }
        }
    }
    for clip in &spec.clips {
        if let Some(trim) = &clip.trim
            && (!trim.start_s.is_finite()
                || !trim.end_s.is_finite()
                || trim.start_s < 0.0
                || trim.end_s <= trim.start_s)
        {
            return Err("clip trim must satisfy 0 <= start < end");
        }
    }
    if let Some(audio) = &spec.audio
        && let Some(gain) = audio.gain_db
        && (!gain.is_finite() || !(-60.0..=12.0).contains(&gain))
    {
        return Err("audio gain must be between -60 and 12 dB");
    }
    if let Some(subtitles) = &spec.subtitles {
        if matches!(subtitles.mode, SubtitleMode::Mux)
            && !matches!(spec.output.container, Container::Mp4)
        {
            return Err("muxed subtitles require the mp4 container");
        }
        if let Some(style) = &subtitles.style
            && (style.font_size.is_some_and(|v| !(8..=96).contains(&v))
                || style.margin_v.is_some_and(|v| v > 400))
        {
            return Err("subtitle style values are out of bounds");
        }
    }
    if let Some(resolution) = &spec.output.resolution {
        let valid = resolution.split_once('x').is_some_and(|(w, h)| {
            w.parse::<u32>().is_ok_and(|w| (16..=7_680).contains(&w))
                && h.parse::<u32>().is_ok_and(|h| (16..=4_320).contains(&h))
        });
        if !valid {
            return Err("output resolution must be WIDTHxHEIGHT within 16..7680x4320");
        }
    }
    if let Some(fps) = spec.output.fps
        && !(1..=120).contains(&fps)
    {
        return Err("output fps must be between 1 and 120");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::{ArtifactId, BlobRef, Digest, Metadata};

    use super::*;

    fn artifact_ref() -> ArtifactRef {
        let content = b"clip".as_slice();
        let digest = Digest::blob_content(content);
        let blob = BlobRef::try_new(
            "blob-1",
            "video/mp4",
            u64::try_from(content.len()).expect("length"),
            Some(digest),
            None::<&str>,
        )
        .expect("blob");
        ArtifactRef::try_new(
            ArtifactId::from_bytes([7; 16]),
            "video",
            blob,
            digest,
            Digest::raw_json(b"scope"),
            Metadata::empty(),
        )
        .expect("artifact")
    }

    fn minimal(clips: usize, transitions: Option<Vec<TransitionSpec>>) -> CompositionSpec {
        CompositionSpec {
            version: 1,
            clips: (0..clips)
                .map(|_| ClipSpec {
                    artifact: artifact_ref(),
                    trim: None,
                })
                .collect(),
            transitions,
            audio: None,
            subtitles: None,
            output: OutputSpec {
                container: Container::Mp4,
                resolution: None,
                fps: None,
            },
        }
    }

    #[test]
    fn spec_json_round_trips_with_snake_case_enums() {
        let spec = minimal(
            2,
            Some(vec![TransitionSpec {
                kind: TransitionKind::FadeToBlack,
                duration_s: Some(0.5),
            }]),
        );
        let json = serde_json::to_string(&spec).expect("serialize");
        assert!(json.contains(r#""type":"fade_to_black""#));
        let back: CompositionSpec = serde_json::from_str(&json).expect("parse");
        assert_eq!(back.clips.len(), 2);
        assert!(
            serde_json::from_str::<CompositionSpec>(&json.replacen('{', r#"{"extra":1,"#, 1))
                .is_err(),
            "unknown fields must be rejected"
        );
    }

    #[test]
    fn validation_enforces_counts_bounds_and_version() {
        assert!(validate_spec(&minimal(2, None)).is_ok());
        assert!(validate_spec(&minimal(0, None)).is_err(), "needs >= 1 clip");
        let mut wrong_version = minimal(1, None);
        wrong_version.version = 2;
        assert!(validate_spec(&wrong_version).is_err());
        let mismatched = minimal(
            3,
            Some(vec![TransitionSpec {
                kind: TransitionKind::Cut,
                duration_s: None,
            }]),
        );
        assert!(
            validate_spec(&mismatched).is_err(),
            "transitions must be clips-1"
        );
        let bad_duration = minimal(
            2,
            Some(vec![TransitionSpec {
                kind: TransitionKind::Crossfade,
                duration_s: Some(30.0),
            }]),
        );
        assert!(validate_spec(&bad_duration).is_err(), "transition <= 5s");
        let mut bad_trim = minimal(1, None);
        bad_trim.clips[0].trim = Some(TrimSpec {
            start_s: 5.0,
            end_s: 2.0,
        });
        assert!(validate_spec(&bad_trim).is_err());
        let mut bad_gain = minimal(1, None);
        bad_gain.audio = Some(AudioSpec {
            artifact: artifact_ref(),
            mode: AudioMode::Mix,
            gain_db: Some(-100.0),
        });
        assert!(validate_spec(&bad_gain).is_err(), "gain in [-60, 12]");
        let mut bad_style = minimal(1, None);
        bad_style.subtitles = Some(SubtitlesSpec {
            artifact: artifact_ref(),
            mode: SubtitleMode::BurnIn,
            style: Some(SubtitleStyle {
                font_size: Some(4),
                margin_v: None,
            }),
        });
        assert!(validate_spec(&bad_style).is_err(), "font_size in [8, 96]");
        let mut bad_mux = minimal(1, None);
        bad_mux.output.container = Container::Webm;
        bad_mux.subtitles = Some(SubtitlesSpec {
            artifact: artifact_ref(),
            mode: SubtitleMode::Mux,
            style: None,
        });
        assert!(validate_spec(&bad_mux).is_err(), "mux is mp4-only");
        let mut bad_res = minimal(1, None);
        bad_res.output.resolution = Some("bogus".into());
        assert!(
            validate_spec(&bad_res).is_err(),
            "resolution is WIDTHxHEIGHT"
        );
        let mut bad_fps = minimal(1, None);
        bad_fps.output.fps = Some(500);
        assert!(validate_spec(&bad_fps).is_err(), "fps in [1, 120]");
    }
}
