//! `MoviePlan` v1 types and plan-level validation.

use finstack_ai_kernel::ArtifactRef;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

/// Top-level declarative movie generation plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoviePlan {
    /// Plan format version. Only `1` is currently accepted.
    pub version: u32,
    /// Optional human-readable plan title.
    pub title: Option<String>,
    /// Defaults inherited by every scene unless overridden.
    pub defaults: PlanDefaults,
    /// Ordered list of scenes making up the movie. Must be non-empty.
    pub scenes: Vec<SceneSpec>,
    /// Optional transitions between adjacent scenes.
    pub transitions: Option<Vec<PlanTransition>>,
    /// Optional plan-wide audio track.
    pub audio: Option<PlanAudio>,
    /// Output rendering configuration.
    pub output: PlanOutput,
}

/// Plan-wide defaults inherited by scenes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDefaults {
    /// Default image generation model identifier.
    pub image_model: String,
    /// Default video generation model identifier.
    pub video_model: String,
    /// Optional default resolution label (e.g. `"720p"`).
    pub resolution: Option<String>,
    /// Optional default aspect ratio label.
    pub aspect_ratio: Option<String>,
    /// Default scene duration in seconds, used when a scene omits its own.
    pub scene_duration_s: u32,
}

/// A single scene within the movie plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneSpec {
    /// Scene identifier, matching `^[a-z0-9-]{1,64}$`.
    pub id: String,
    /// Video generation prompt for this scene.
    pub video_prompt: String,
    /// Starting frame source for the scene.
    pub start_frame: FrameSource,
    /// Optional ending frame source for the scene.
    pub end_frame: Option<FrameSource>,
    /// Optional reference images used for style guidance.
    pub reference_images: Option<Vec<FrameSource>>,
    /// Optional scene duration override, in seconds.
    pub duration_s: Option<u32>,
    /// Optional generation seed for reproducibility.
    pub seed: Option<i64>,
    /// Optional caption cues to burn in or ship as a sidecar.
    pub captions: Option<Vec<CaptionCue>>,
    /// Optional per-scene model/resolution overrides.
    pub overrides: Option<SceneOverrides>,
}

/// A single caption cue with a text span and timing window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptionCue {
    /// Caption text, 1 to 200 characters.
    pub text: String,
    /// Cue start time, in seconds.
    pub start_s: f64,
    /// Cue end time, in seconds.
    pub end_s: f64,
}

/// Per-scene overrides of plan defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneOverrides {
    /// Overridden image generation model identifier.
    pub image_model: Option<String>,
    /// Overridden video generation model identifier.
    pub video_model: Option<String>,
    /// Overridden resolution label.
    pub resolution: Option<String>,
}

/// Source of a single frame: an inline prompt, a stored artifact, or a URL.
///
/// This is conceptually an untagged enum of single-key objects, each of
/// which rejects unknown fields. serde's derived `untagged` support does
/// not let a struct-like variant carry its own `deny_unknown_fields` (that
/// attribute is rejected at the variant level, and derived untagged
/// deserialization silently ignores fields the matched variant doesn't
/// recognize instead of trying the next variant), so a two-key object like
/// `{"prompt": "x", "url": "y"}` would otherwise parse successfully as
/// whichever variant is listed first. To keep that fixture failing, this
/// type implements `Deserialize` by hand below: it requires the JSON value
/// to be an object with exactly one of `prompt`, `artifact`, or `url`.
/// `Serialize` is still derived as `untagged`, since serialization has no
/// such ambiguity. The `invalid-ambiguous-frame.json` and
/// `invalid-missing-prompt.json` fixtures are the oracle for this behavior;
/// see the `plan` module tests.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum FrameSource {
    /// An inline text-to-image prompt.
    Prompt {
        /// Prompt text.
        prompt: String,
    },
    /// A reference to an artifact already held by the host's artifact store.
    Artifact {
        /// Artifact reference.
        artifact: ArtifactRef,
    },
    /// A remote URL.
    Url {
        /// Source URL.
        url: String,
    },
}

impl<'de> Deserialize<'de> for FrameSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let serde_json::Value::Object(map) = value else {
            return Err(de::Error::custom("frame source must be a JSON object"));
        };
        if map.len() != 1 {
            return Err(de::Error::custom(
                "frame source must have exactly one of: prompt, artifact, url",
            ));
        }
        let Some((key, val)) = map.into_iter().next() else {
            return Err(de::Error::custom(
                "frame source must have exactly one of: prompt, artifact, url",
            ));
        };
        match key.as_str() {
            "prompt" => {
                let prompt = serde_json::from_value(val).map_err(de::Error::custom)?;
                Ok(FrameSource::Prompt { prompt })
            }
            "artifact" => {
                let artifact = serde_json::from_value(val).map_err(de::Error::custom)?;
                Ok(FrameSource::Artifact { artifact })
            }
            "url" => {
                let url = serde_json::from_value(val).map_err(de::Error::custom)?;
                Ok(FrameSource::Url { url })
            }
            other => Err(de::Error::unknown_field(
                other,
                &["prompt", "artifact", "url"],
            )),
        }
    }
}

/// A transition placed after a named scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanTransition {
    /// Id of the scene this transition follows.
    pub after: String,
    /// Transition kind.
    #[serde(rename = "type")]
    pub kind: TransitionKindName,
    /// Optional transition duration, in seconds.
    pub duration_s: Option<f64>,
}

/// Named transition kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKindName {
    /// Hard cut, no transition.
    Cut,
    /// Cross-fade between scenes.
    Crossfade,
    /// Fade to black between scenes.
    FadeToBlack,
}

/// Plan-wide audio track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanAudio {
    /// Artifact reference for the audio track.
    pub artifact: ArtifactRef,
    /// Audio mixing mode (`"replace"` or `"mix"`).
    pub mode: String,
}

/// Output rendering configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOutput {
    /// Output container format (`"mp4"` or `"webm"`).
    pub container: String,
    /// Optional output frame rate.
    pub fps: Option<u32>,
    /// Optional caption handling mode.
    pub captions: Option<CaptionsMode>,
}

/// Caption handling modes for the rendered output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionsMode {
    /// No captions in the output.
    None,
    /// Captions shipped as a separate sidecar file.
    Sidecar,
    /// Captions burned into the video frames.
    BurnIn,
}

/// Host-imposed bounds on a movie plan's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanLimits {
    /// Maximum number of scenes allowed in a single plan.
    pub max_scenes: usize,
    /// Maximum total video duration, in seconds, across all scenes.
    pub max_total_video_s: u64,
    /// Maximum number of concurrent render jobs.
    pub max_concurrent_jobs: usize,
}

/// Computed resource budget for a validated movie plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanBudget {
    /// Number of scenes in the plan.
    pub scene_count: usize,
    /// Total video duration, in seconds, across all scenes.
    pub total_video_s: u64,
}

/// Validate a movie plan against host-imposed limits, computing its budget.
///
/// # Errors
///
/// Returns an error message when the plan is malformed (bad version, empty
/// scene list, duplicate or malformed scene ids, empty prompts, out-of-range
/// durations, malformed or overlapping caption cues, too many or unpinned
/// reference images, a caption delivery mode with no cues anywhere, or
/// transitions that name an unknown or final scene, an out-of-range transition
/// duration, an unknown output container or audio mode, or an out-of-range
/// output frame rate) or exceeds the supplied `limits`.
#[allow(
    clippy::cast_precision_loss,
    reason = "scene durations are bounded to 1..=60 seconds above; the cast to f64 for a \
              caption-cue timing comparison never loses precision in practice"
)]
pub fn validate_plan(plan: &MoviePlan, limits: &PlanLimits) -> Result<PlanBudget, &'static str> {
    if plan.version != 1 {
        return Err("movie plan version must be 1");
    }
    if plan.scenes.is_empty() {
        return Err("movie plan needs at least one scene");
    }
    if plan.scenes.len() > limits.max_scenes {
        return Err("movie plan exceeds the scene ceiling");
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut total_video_s: u64 = 0;
    let mut any_cues = false;
    for scene in &plan.scenes {
        let valid_id = !scene.id.is_empty()
            && scene.id.len() <= 64
            && scene
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid_id {
            return Err("scene id must match [a-z0-9-]{1,64}");
        }
        if !seen.insert(scene.id.as_str()) {
            return Err("scene ids must be unique");
        }
        if scene.video_prompt.is_empty() {
            return Err("scene video prompt must not be empty");
        }
        let duration = u64::from(scene.duration_s.unwrap_or(plan.defaults.scene_duration_s));
        if !(1..=60).contains(&duration) {
            return Err("scene duration must be between 1 and 60 seconds");
        }
        // Mirrors the video tool schema's `reference_images` maxItems, and its
        // requirement that every reference resolve to a URL or stored artifact:
        // a prompt has no resolution path here, so rejecting beats dropping it.
        if let Some(references) = &scene.reference_images {
            if references.len() > 4 {
                return Err("scene has more than 4 reference images");
            }
            if references
                .iter()
                .any(|source| matches!(source, FrameSource::Prompt { .. }))
            {
                return Err("reference images must be pinned artifacts or urls");
            }
        }
        if let Some(cues) = &scene.captions {
            if cues.len() > 32 {
                return Err("scene has more than 32 caption cues");
            }
            if !cues.is_empty() {
                any_cues = true;
            }
            let mut previous_end = 0.0_f64;
            for cue in cues {
                if cue.text.is_empty() || cue.text.chars().count() > 200 {
                    return Err("caption text must be 1 to 200 characters");
                }
                let in_order = cue.start_s.is_finite()
                    && cue.end_s.is_finite()
                    && cue.start_s >= previous_end
                    && cue.end_s > cue.start_s
                    && cue.end_s <= duration as f64;
                if !in_order {
                    return Err("caption cues must be ordered and inside the scene duration");
                }
                previous_end = cue.end_s;
            }
        }
        total_video_s = total_video_s.saturating_add(duration);
    }
    if total_video_s > limits.max_total_video_s {
        return Err("movie plan exceeds the total video seconds ceiling");
    }
    // A caption delivery mode with nothing to deliver would otherwise stage an
    // empty transcript, or silently ship a movie with no captions at all.
    if matches!(
        plan.output.captions,
        Some(CaptionsMode::Sidecar | CaptionsMode::BurnIn)
    ) && !any_cues
    {
        return Err("captions delivery requested but no scene has cues");
    }
    let last_id = plan.scenes.last().map(|scene| scene.id.as_str());
    for transition in plan.transitions.iter().flatten() {
        if !seen.contains(transition.after.as_str()) {
            return Err("transition names an unknown scene");
        }
        if Some(transition.after.as_str()) == last_id {
            return Err("transition cannot follow the final scene");
        }
        // A `cut` carries no visible duration, so its value is unconstrained
        // (matching the compose crate's `validate_spec`); every other kind is
        // fed to `xfade`/`acrossfade` and must be a sane, finite window.
        if !matches!(transition.kind, TransitionKindName::Cut)
            && let Some(duration) = transition.duration_s
            && (!duration.is_finite() || !(0.05..=5.0).contains(&duration))
        {
            return Err("transition duration must be between 0.05 and 5 seconds");
        }
    }
    validate_schema_bounds(plan)?;
    Ok(PlanBudget {
        scene_count: plan.scenes.len(),
        total_video_s,
    })
}

/// Enforce the published schema's enum members and numeric ranges.
///
/// `movie-plan.v1.json` constrains `output.container`, `audio.mode`, and
/// `output.fps` as enums and bounded integers, but the Rust types are looser
/// (`String` / `Option<u32>`), so deserialization alone lets a typo through.
/// Enforcing the bounds at submit time keeps a bad container from burning the
/// whole generation budget and then failing terminally at the final render.
fn validate_schema_bounds(plan: &MoviePlan) -> Result<(), &'static str> {
    if !matches!(plan.output.container.as_str(), "mp4" | "webm") {
        return Err("output container must be mp4 or webm");
    }
    if let Some(fps) = plan.output.fps
        && !(1..=120).contains(&fps)
    {
        return Err("output fps must be between 1 and 120");
    }
    if let Some(audio) = &plan.audio
        && !matches!(audio.mode.as_str(), "replace" | "mix")
    {
        return Err("audio mode must be replace or mix");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> PlanLimits {
        PlanLimits {
            max_scenes: 10,
            max_total_video_s: 120,
            max_concurrent_jobs: 2,
        }
    }

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
    fn valid_fixture_parses_and_budgets() {
        let bytes = std::fs::read(
            finstack_ai_test::repo_root()
                .join("fixtures/compatibility/movie-plan/valid-two-scene.json"),
        )
        .expect("fixture");
        let plan: MoviePlan = serde_json::from_slice(&bytes).expect("parse");
        let budget = validate_plan(&plan, &limits()).expect("valid");
        assert_eq!(budget.scene_count, 2);
        assert_eq!(budget.total_video_s, 14); // scene-01 explicit 8s + scene-02 default 6s
    }

    #[test]
    fn invalid_fixtures_are_rejected() {
        for name in [
            "invalid-missing-prompt.json",
            "invalid-ambiguous-frame.json",
        ] {
            let bytes = std::fs::read(
                finstack_ai_test::repo_root()
                    .join("fixtures/compatibility/movie-plan")
                    .join(name),
            )
            .expect("fixture");
            assert!(
                serde_json::from_slice::<MoviePlan>(&bytes).is_err(),
                "{name} must fail to parse"
            );
        }
    }

    #[test]
    fn budget_reference_and_caption_violations_fail_closed() {
        let mut plan = two_scene_plan();
        plan.scenes[1].id = plan.scenes[0].id.clone();
        assert!(validate_plan(&plan, &limits()).is_err(), "duplicate ids");

        let mut plan = two_scene_plan();
        plan.transitions = Some(vec![PlanTransition {
            after: "scene-99".into(),
            kind: TransitionKindName::Crossfade,
            duration_s: Some(0.5),
        }]);
        assert!(validate_plan(&plan, &limits()).is_err(), "unknown after");

        let plan = two_scene_plan();
        let tight = PlanLimits {
            max_scenes: 1,
            ..limits()
        };
        assert!(validate_plan(&plan, &tight).is_err(), "scene ceiling");
        let tight = PlanLimits {
            max_total_video_s: 5,
            ..limits()
        };
        assert!(validate_plan(&plan, &tight).is_err(), "seconds ceiling");

        let mut plan = two_scene_plan();
        plan.scenes[0].captions = Some(vec![CaptionCue {
            text: "x".repeat(300),
            start_s: 0.0,
            end_s: 2.0,
        }]);
        assert!(
            validate_plan(&plan, &limits()).is_err(),
            "caption text bound"
        );

        let mut plan = two_scene_plan();
        plan.scenes[0].captions = Some(vec![
            CaptionCue {
                text: "one".into(),
                start_s: 0.0,
                end_s: 4.0,
            },
            CaptionCue {
                text: "two".into(),
                start_s: 3.0,
                end_s: 6.0,
            },
        ]);
        assert!(validate_plan(&plan, &limits()).is_err(), "overlapping cues");

        let mut plan = two_scene_plan();
        plan.scenes[0].reference_images = Some(
            (0..5)
                .map(|index| FrameSource::Url {
                    url: format!("https://example.test/ref-{index}.png"),
                })
                .collect(),
        );
        assert_eq!(
            validate_plan(&plan, &limits()),
            Err("scene has more than 4 reference images")
        );

        let mut plan = two_scene_plan();
        plan.scenes[0].reference_images = Some(vec![FrameSource::Prompt {
            prompt: "moody teal grade".into(),
        }]);
        assert_eq!(
            validate_plan(&plan, &limits()),
            Err("reference images must be pinned artifacts or urls"),
            "an unresolvable reference is reported, not silently dropped"
        );

        let mut plan = two_scene_plan();
        plan.scenes[0].captions = None;
        assert_eq!(
            validate_plan(&plan, &limits()),
            Err("captions delivery requested but no scene has cues"),
            "burn_in output with nothing to caption"
        );
    }

    #[test]
    fn schema_enum_and_range_violations_fail_closed() {
        let mut plan = two_scene_plan();
        plan.output.container = "mkv".into();
        assert_eq!(
            validate_plan(&plan, &limits()),
            Err("output container must be mp4 or webm")
        );

        let mut plan = two_scene_plan();
        plan.audio = Some(PlanAudio {
            artifact: audio_artifact(),
            mode: "duck".into(),
        });
        assert_eq!(
            validate_plan(&plan, &limits()),
            Err("audio mode must be replace or mix")
        );
        // The two schema-legal modes still pass.
        for mode in ["replace", "mix"] {
            let mut plan = two_scene_plan();
            plan.audio = Some(PlanAudio {
                artifact: audio_artifact(),
                mode: mode.into(),
            });
            assert!(validate_plan(&plan, &limits()).is_ok(), "{mode} is legal");
        }

        for bad in [0.0_f64, 0.01, 6.0, f64::NAN] {
            let mut plan = two_scene_plan();
            plan.transitions = Some(vec![PlanTransition {
                after: "scene-01".into(),
                kind: TransitionKindName::Crossfade,
                duration_s: Some(bad),
            }]);
            assert_eq!(
                validate_plan(&plan, &limits()),
                Err("transition duration must be between 0.05 and 5 seconds"),
                "crossfade duration {bad}"
            );
        }
        // A `cut` is unbounded, matching the compose crate's `validate_spec`.
        let mut plan = two_scene_plan();
        plan.transitions = Some(vec![PlanTransition {
            after: "scene-01".into(),
            kind: TransitionKindName::Cut,
            duration_s: Some(99.0),
        }]);
        assert!(validate_plan(&plan, &limits()).is_ok(), "cut is unbounded");

        for bad in [0, 121] {
            let mut plan = two_scene_plan();
            plan.output.fps = Some(bad);
            assert_eq!(
                validate_plan(&plan, &limits()),
                Err("output fps must be between 1 and 120"),
                "fps {bad}"
            );
        }
    }

    fn audio_artifact() -> ArtifactRef {
        use finstack_ai_kernel::{ArtifactId, BlobRef, Digest, Metadata};
        let content = b"score".as_slice();
        let digest = Digest::blob_content(content);
        let blob = BlobRef::try_new(
            "blob-audio",
            "audio/mpeg",
            u64::try_from(content.len()).expect("length"),
            Some(digest),
            None::<&str>,
        )
        .expect("blob");
        ArtifactRef::try_new(
            ArtifactId::from_bytes([9; 16]),
            "audio",
            blob,
            digest,
            Digest::raw_json(b"scope"),
            Metadata::empty(),
        )
        .expect("artifact")
    }
}
