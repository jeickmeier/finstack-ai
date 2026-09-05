# finstack-ai-workflow-media-pipeline

MoviePlan pipeline driver, in the `finstack-ai-workflow-local` mold:
tick-based, resumable, with adapter-owned sqlite state rather than kernel
journal records.

Tools `render_movie`, `advance_render`, and `get_render_status` expose the
pipeline to agents. Hosts bound spend via `PlanLimits`; all generated media
moves through the host's artifact store as `ArtifactRef`s, never as raw
bytes on the wire.

This crate is T1 native and is not isolated.

## Host composition

The pipeline composes three host-owned pieces: an artifact store with a
ceiling raised for video, the OpenRouter media toolset that generates frames
and clips, and the ffmpeg composition toolset that stitches them. The store
is host-injected — the SDK never constructs one on the caller's behalf.

```rust,ignore
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::artifact::ArtifactStore;
use finstack_ai_store_artifact::LocalArtifactStore;
use finstack_ai_tools_openrouter_media::{OpenRouterMediaConfig, OpenRouterMediaToolset};
use finstack_ai_tools_video_compose::{VideoComposeConfig, VideoComposeToolset};
use finstack_ai_workflow_media_pipeline::{
    MediaPipelineConfig, MediaPipelineDriver, MediaPipelineToolset, PlanLimits,
    SqliteRenderStateStore,
};

// Default artifact ceilings are sized for documents. Raise them before any
// video render: a single 1080p clip exceeds them.
let store: Arc<dyn ArtifactStore> = Arc::new(
    LocalArtifactStore::try_new(PathBuf::from("/var/lib/finstack/artifacts"))?
        .with_max_artifact_bytes(256 * 1024 * 1024),
);

let media_tools = Arc::new(
    OpenRouterMediaToolset::try_new(OpenRouterMediaConfig {
        api_key: openrouter_api_key,
        endpoint: String::new(),
        referer: None,
        title: None,
        max_result_bytes: 262_144,
    })?
    .with_artifact_store(Arc::clone(&store)),
);

let compose_tools = Arc::new(VideoComposeToolset::try_new(VideoComposeConfig {
    ffmpeg_path: PathBuf::from("/usr/bin/ffmpeg"),
    ffprobe_path: PathBuf::from("/usr/bin/ffprobe"),
    artifact_store: Arc::clone(&store),
    scratch_dir: PathBuf::from("/var/tmp/finstack-video-compose"),
    render_timeout: Duration::from_secs(300),
})?);

let driver = Arc::new(MediaPipelineDriver::try_new(MediaPipelineConfig {
    media_tools,
    compose_tools,
    state: Arc::new(SqliteRenderStateStore::open("/var/lib/finstack/renders.sqlite3")?),
    artifact_store: Some(store),
    limits: PlanLimits {
        max_scenes: 8,
        max_total_video_s: 240,
        max_concurrent_jobs: 2,
    },
})?);

let pipeline_tools = MediaPipelineToolset::try_new(driver)?;
```

`MemoryRenderStateStore` replaces the sqlite store for tests and for
throwaway renders; a restart cannot resume that state.

The `finstack-ai` facade wires the same stack from a linked constructor.
Enable `workflow-media-pipeline` (which turns on `tool-video-compose` and
`tool-openrouter-media`) and set `LinkedCommon::openrouter_media`,
`LinkedCommon::video_compose`, and `LinkedCommon::media_pipeline` together
on a host that already supplies an artifact store. Any missing piece is a
configuration error rather than a silently reduced toolset. Python exposes
the same fields as the `openrouter_media_*`, `video_compose_*`, and
`media_pipeline_*` keyword arguments on every linked factory. Both toolsets
are native-only and stay off the `wasm-host` feature graph.

## The MoviePlan contract

`render_movie` takes one `plan` object whose shape is published as
[`schemas/movie-plan/movie-plan.v1.json`](../../../schemas/movie-plan/movie-plan.v1.json).
The runtime does not evaluate that JSON Schema literally: the plan is
deserialized into the Rust `MoviePlan` types (which reject unknown fields and
ambiguous frame sources) and then checked by `validate_plan`, which enforces
the schema's semantic bounds — enum members, ranges, and the host's own
ceilings — as well.
A plan is a bounded, declarative description of a whole movie:

- `defaults` — `image_model`, `video_model`, and `scene_duration_s`, plus
  optional `resolution` and `aspect_ratio`, applied to every scene.
- `scenes` — an ordered list. Each scene has a lowercase-hyphen `id`, a
  `video_prompt`, a `start_frame`, and optionally an `end_frame`,
  `reference_images`, `duration_s`, `seed`, `captions`, and per-scene
  `overrides`. A frame is exactly one of `{"prompt": …}` (generated),
  `{"artifact": …}` (already staged), or `{"url": …}` (pinned).
- `transitions` — `cut`, `crossfade`, or `fade_to_black`, each keyed by the
  scene id it follows. A transition after the final scene is rejected.
- `audio` — one staged track, `replace` or `mix`.
- `output` — `container` (`mp4` or `webm`), optional `fps`, and `captions`
  delivery (`none`, `sidecar`, or `burn_in`).

Validation is fail-closed and happens before any paid call: an over-limit
scene count or total duration, a duplicate scene id, an ambiguous frame
source, an overlapping caption cue, a caption mode with no cues anywhere, an
unknown `output.container` or `audio.mode`, an `output.fps` outside 1–120, or
a non-`cut` transition `duration_s` outside 0.05–5 seconds is rejected at
submit time.

Each `render_movie` call submits the plan and runs one tick. `advance_render`
runs the next tick for a `render_id`; `get_render_status` reads progress
without advancing it. State is persisted per scene, so a resumed render never
re-pays for a completed stage.

## Authoring scene prompts

The two frames of a scene and its motion prompt are read by different
models, so they have to agree on the world they describe.

- Name concrete nouns. "A brass astrolabe on a walnut desk" survives model
  drift; "something scientific" does not.
- Use camera language for motion: `slow push in`, `handheld pan left`,
  `locked-off wide`. The video model reads composition verbs more reliably
  than mood adjectives.
- Repeat the same style tokens verbatim in a scene's `start_frame` and
  `end_frame` prompts — lighting, lens, palette, and medium. Divergent style
  tokens between the two frames are the usual cause of a clip that appears to
  cut mid-shot.
- Describe motion relative to the start frame, not in the absolute. The
  video model is given the start frame; "the astrolabe rotates a quarter turn
  toward the window" is actionable, "an astrolabe rotating" is not.
- Pin recurring subjects with `reference_images` or a shared `seed` rather
  than by re-describing them in prose across scenes.

## Authoring captions

Caption cues live on the scene, in scene-relative seconds, and are flattened
onto the movie timeline as SRT and VTT sidecars.

- Keep a cue to roughly seven words or fewer. Short-form players give a cue
  a small, fixed box, and a longer line wraps or truncates.
- Align cue boundaries to the scene's own beats — a cue that straddles a
  transition reads as a subtitle for the wrong shot. Cues may not overlap
  within a scene.
- Prefer `output.captions: "burn_in"` for platforms that autoplay muted.
  Burned-in text is the only delivery that survives a player which ignores
  sidecar tracks, at the cost of a re-render to change it.
- Use `sidecar` when captions must stay editable or translatable after the
  render, and `none` when the host adds its own caption layer downstream.

## Tick ownership and interrupted submissions

Each tick atomically claims the render by persisting `status: "advancing"`
with its next revision before invoking leaf tools. Completed scene progress is
saved under that claim immediately. Concurrent `advance` calls return the
claimed state without issuing tools. A clean tick publishes its final status.

An interrupted or failed progress write leaves `advancing` in place. It has no
automatic expiry: a provider may already have accepted a paid submission even
when no job ID was saved. Stop the original caller, inspect the saved stages and
provider jobs, reconcile the outcome through `RenderStateStore::update` using
the current revision, and only then restore `Running` or a terminal status.
Never clear a live claim or automatically resubmit an uncertain stage.

Compatibility: readers must accept the new `RenderStatus::Advancing` variant
(`"advancing"` in JSON). Existing rows remain readable. Upgrade all readers and
drivers together; older drivers do not enforce tick ownership.
