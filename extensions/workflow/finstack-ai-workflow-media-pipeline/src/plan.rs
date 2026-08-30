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
/// durations, malformed or overlapping caption cues, or transitions that
/// name an unknown or final scene) or exceeds the supplied `limits`.
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
        if let Some(cues) = &scene.captions {
            if cues.len() > 32 {
                return Err("scene has more than 32 caption cues");
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
    let last_id = plan.scenes.last().map(|scene| scene.id.as_str());
    for transition in plan.transitions.iter().flatten() {
        if !seen.contains(transition.after.as_str()) {
            return Err("transition names an unknown scene");
        }
        if Some(transition.after.as_str()) == last_id {
            return Err("transition cannot follow the final scene");
        }
    }
    Ok(PlanBudget {
        scene_count: plan.scenes.len(),
        total_video_s,
    })
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
    }
}
