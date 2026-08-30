//! Caption timeline flattening and SRT/VTT serialization.
//!
//! [`cue_timeline`] walks a [`MoviePlan`]'s scenes in order, offsetting each
//! scene's caption cues onto the absolute movie timeline and subtracting the
//! crossfade/fade-to-black overlap introduced by the transition that follows
//! each scene. This mirrors the compose filtergraph's `xfade` offsets over
//! *planned* durations; actual clip drift is the documented v1 tolerance.

use crate::plan::{MoviePlan, TransitionKindName};

/// A single caption cue positioned on the absolute movie timeline.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TimedCue {
    /// Caption text.
    pub text: String,
    /// Absolute start time on the movie timeline, in seconds.
    pub start_s: f64,
    /// Absolute end time on the movie timeline, in seconds.
    pub end_s: f64,
}

/// Flatten a movie plan's per-scene caption cues onto the absolute movie
/// timeline.
///
/// Scenes are walked in order starting at `offset = 0.0`. Each cue is emitted
/// at `offset + start_s` / `offset + end_s`. After each scene, `offset` is
/// advanced by the scene's effective duration (`duration_s` or the plan
/// default) minus the overlap introduced by the transition that follows it:
/// the transition's `duration_s` (default `0.5`) for `crossfade` and
/// `fade_to_black`, or `0.0` for `cut` or an absent transition.
pub(crate) fn cue_timeline(plan: &MoviePlan) -> Vec<TimedCue> {
    let mut cues = Vec::new();
    let mut offset = 0.0_f64;
    for scene in &plan.scenes {
        if let Some(scene_cues) = &scene.captions {
            for cue in scene_cues {
                cues.push(TimedCue {
                    text: cue.text.clone(),
                    start_s: offset + cue.start_s,
                    end_s: offset + cue.end_s,
                });
            }
        }
        let effective_duration_s =
            f64::from(scene.duration_s.unwrap_or(plan.defaults.scene_duration_s));
        let overlap_s = plan
            .transitions
            .iter()
            .flatten()
            .find(|transition| transition.after == scene.id)
            .map_or(0.0, |transition| match transition.kind {
                TransitionKindName::Crossfade | TransitionKindName::FadeToBlack => {
                    transition.duration_s.unwrap_or(0.5)
                }
                TransitionKindName::Cut => 0.0,
            });
        offset += effective_duration_s - overlap_s;
    }
    cues
}

/// Format `seconds` as `HH:MM:SS{millis_sep}mmm`.
fn stamp(seconds: f64, millis_sep: char) -> String {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "movie timelines never approach u64::MAX milliseconds"
    )]
    #[allow(
        clippy::cast_sign_loss,
        reason = "caption cue times are validated non-negative before reaching this module"
    )]
    let total_millis = (seconds * 1000.0).round() as u64;
    let millis = total_millis % 1000;
    let total_seconds = total_millis / 1000;
    let secs = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let mins = total_minutes % 60;
    let hours = total_minutes / 60;
    format!("{hours:02}:{mins:02}:{secs:02}{millis_sep}{millis:03}")
}

/// Render `cues` as an SRT subtitle file.
pub(crate) fn to_srt(cues: &[TimedCue]) -> String {
    let mut out = String::new();
    for (index, cue) in cues.iter().enumerate() {
        out.push_str(&(index + 1).to_string());
        out.push('\n');
        out.push_str(&stamp(cue.start_s, ','));
        out.push_str(" --> ");
        out.push_str(&stamp(cue.end_s, ','));
        out.push('\n');
        out.push_str(&cue.text);
        out.push_str("\n\n");
    }
    out
}

/// Render `cues` as a WebVTT subtitle file.
pub(crate) fn to_vtt(cues: &[TimedCue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues {
        out.push_str(&stamp(cue.start_s, '.'));
        out.push_str(" --> ");
        out.push_str(&stamp(cue.end_s, '.'));
        out.push('\n');
        out.push_str(&cue.text);
        out.push_str("\n\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        CaptionCue, CaptionsMode, FrameSource, PlanDefaults, PlanOutput, PlanTransition, SceneSpec,
    };

    fn two_scene_plan() -> MoviePlan {
        MoviePlan {
            version: 1,
            title: Some("Two scene test".to_string()),
            defaults: PlanDefaults {
                image_model: "test/image-model".to_string(),
                video_model: "test/video-model".to_string(),
                resolution: Some("720p".to_string()),
                aspect_ratio: None,
                scene_duration_s: 6,
            },
            scenes: vec![
                SceneSpec {
                    id: "scene-01".to_string(),
                    video_prompt: "camera glides across a harbor at dawn".to_string(),
                    start_frame: FrameSource::Prompt {
                        prompt: "wide shot of a harbor at dawn, golden light".to_string(),
                    },
                    end_frame: Some(FrameSource::Prompt {
                        prompt: "close-up of a moored fishing boat".to_string(),
                    }),
                    reference_images: None,
                    duration_s: Some(8),
                    seed: Some(42),
                    captions: Some(vec![
                        CaptionCue {
                            text: "Harbors wake up slowly.".to_string(),
                            start_s: 0.0,
                            end_s: 3.5,
                        },
                        CaptionCue {
                            text: "Then all at once.".to_string(),
                            start_s: 3.5,
                            end_s: 7.5,
                        },
                    ]),
                    overrides: None,
                },
                SceneSpec {
                    id: "scene-02".to_string(),
                    video_prompt: "gulls lift off the pier".to_string(),
                    start_frame: FrameSource::Url {
                        url: "https://example.test/pier.png".to_string(),
                    },
                    end_frame: None,
                    reference_images: None,
                    duration_s: None,
                    seed: None,
                    captions: None,
                    overrides: None,
                },
            ],
            transitions: Some(vec![PlanTransition {
                after: "scene-01".to_string(),
                kind: TransitionKindName::Crossfade,
                duration_s: Some(0.5),
            }]),
            audio: None,
            output: PlanOutput {
                container: "mp4".to_string(),
                fps: Some(24),
                captions: Some(CaptionsMode::BurnIn),
            },
        }
    }

    #[test]
    fn timeline_offsets_scene_cues_and_subtracts_crossfade_overlap() {
        let mut plan = two_scene_plan();
        plan.scenes[1].captions = Some(vec![CaptionCue {
            text: "Gulls, incoming.".into(),
            start_s: 0.0,
            end_s: 2.0,
        }]);
        let cues = cue_timeline(&plan);
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].start_s, 0.0);
        assert_eq!(cues[1].end_s, 7.5);
        // scene-02 offset = 8.0 - 0.5 crossfade overlap
        assert_eq!(cues[2].start_s, 7.5);
        assert_eq!(cues[2].end_s, 9.5);
    }

    #[test]
    fn srt_and_vtt_render_known_answers() {
        let cues = vec![
            TimedCue {
                text: "Harbors wake up slowly.".into(),
                start_s: 0.0,
                end_s: 3.5,
            },
            TimedCue {
                text: "Then all at once.".into(),
                start_s: 3.5,
                end_s: 7.5,
            },
        ];
        assert_eq!(
            to_srt(&cues),
            "1\n00:00:00,000 --> 00:00:03,500\nHarbors wake up slowly.\n\n2\n00:00:03,500 --> 00:00:07,500\nThen all at once.\n\n"
        );
        assert_eq!(
            to_vtt(&cues),
            "WEBVTT\n\n00:00:00.000 --> 00:00:03.500\nHarbors wake up slowly.\n\n00:00:03.500 --> 00:00:07.500\nThen all at once.\n\n"
        );
    }

    #[test]
    fn hour_rollover_formats_correctly() {
        let cues = vec![TimedCue {
            text: "late".into(),
            start_s: 3_661.25,
            end_s: 3_662.0,
        }];
        assert!(to_srt(&cues).contains("01:01:01,250 --> 01:01:02,000"));
    }
}
