# Media Generation Pipeline — Design

**Date:** 2026-08-19 (revised 2026-08-28 against the migrated library)
**Status:** Approved design, pre-plan
**Baseline:** the OpenRouter provider work has landed. `finstack-ai-provider-openrouter` and `finstack-ai-tools-openrouter-media` exist (the toolset already ships `openrouter_generate_image`, `openrouter_generate_video` — async submit, `openrouter_get_video` — status with in-call `wait_seconds` polling, speech, and transcription, and already stages media into a host-supplied `ArtifactStore` via `deliver_media`). `extensions/artifacts/finstack-ai-store-artifact` already provides `LocalArtifactStore` and `S3ArtifactStore` (hand-rolled SigV4, no AWS SDK) behind the single public `ArtifactStore` contract with store-configurable ceilings. This design builds on those surfaces and adds no parallel storage contract.

## 1. Goal

Add a composable, AI-plannable video generation pipeline to finstack-ai:

1. A text model authors per-scene descriptive prompts (start image, end
   image, video motion) and caption cues — a prompt/skill concern, not code.
2. Images are generated through the existing OpenRouter media toolset.
3. Videos are generated through OpenRouter's async video API with optional
   first-frame, last-frame, and reference images, resolution, duration,
   and seed — extending the existing `openrouter_generate_video` /
   `openrouter_get_video` tools, plus a new download tool.
4. Clips are merged into a final movie with a host-supplied `ffmpeg`
   binary driven by a declarative composition spec, with plan-authored
   captions delivered as SRT/VTT transcripts, burned in, or muxed.

The AI controls the pipeline at two altitudes: calling the media tools
directly (scene-by-scene iteration), or authoring a versioned `MoviePlan`
document executed by a deterministic, resumable, tick-based pipeline
driver exposed as `render_movie` / `advance_render` / `get_render_status`
tools.

Storage of intermediate and final media is host composition — the host
injects an `ArtifactStore` (`LocalArtifactStore` or `S3ArtifactStore`,
ceilings raised via `with_max_artifact_bytes`). Agents never see or choose
the backend; they hold `ArtifactRef`s.

## 2. Non-goals

- **No new storage contract.** The earlier draft's `MediaStore` trait and
  dedicated media store crates are dropped: the migrated library already
  provides local and S3 artifact storage with configurable per-artifact
  ceilings, and the media toolset already stages results as artifacts.
- No streaming multi-gigabyte transfers in v1: artifacts move as `Bytes`
  bounded by the store's `max_artifact_bytes`. The store-artifact crate's
  private driver already has `PutPayload::File` and `get_to_file`
  (streaming, digest-verified); promoting streaming through the public
  `ArtifactStore` contract is deliberate future work, not v1.
- No presigned frame URLs in v1: generated frame images are handed to
  OpenRouter as bounded `data:` URIs (≤ 8 MiB pre-encoding). The S3
  driver's SigV4 presign plumbing exists in-tree when this is wanted.
- No webhook (`callback_url`) or `provider` passthrough exposure to
  agents.
- No titles, overlays, subtitles beyond SRT-based burn-in/mux, and no
  Ken Burns effects in the v1 composition spec (versioned for later).
- No moviepy or Remotion; composition is ffmpeg-only, native.
- No transcription-derived captions in v1: caption cues are plan-authored
  by the text model. Aligning generated audio to word-level timestamps is
  an agent-level flow over the existing `openrouter_transcribe_audio`
  tool, not pipeline machinery.
- No kernel or runtime changes at all. Everything here is extension
  leaves (G-01 holds: the default graph depends on none of this).
- No exactly-once claims: recovery is at-least-once, consistent with the
  kernel's commit-before-effect story.

## 3. Architecture

Three layers of new/extended leaves over existing storage:

```
Layer 3  extensions/workflow/finstack-ai-workflow-media-pipeline   (new)
         MoviePlan schema, tick-based executor, render/advance/status tools
Layer 2  extensions/toolsets/finstack-ai-tools-video-compose       (new)
         Declarative ffmpeg composition (merge, transitions, audio, subtitles)
Layer 1  extensions/toolsets/finstack-ai-tools-openrouter-media    (extended)
         frame-image video submit + video download, artifact-staged
Storage  finstack_ai_runtime::artifact::ArtifactStore              (existing)
         extensions/artifacts/finstack-ai-store-artifact           (existing)
```

Runtime API access follows the migrated namespaced modules:
`finstack_ai_runtime::ports::{PortFuture}`, `ports::model::{...}`,
`ports::tool::{...}`, `finstack_ai_runtime::artifact::{ArtifactStore,
ArtifactScope, ArtifactRef re-exports, validate_retrieved_artifact,
stage_required_artifact}`; kernel types come from `finstack_ai_kernel`.

## 4. Storage: ride the artifact system

- **Refs.** Every media object a tool produces or consumes is an
  `ArtifactRef`, serialized in tool-result JSON exactly as the media
  toolset already does (`{"artifact": ..., "media_type", "byte_length"}`).
  Refs are scope-bound (`ArtifactScope { tenant_scope, session_id,
  run_id, sensitivity }` built from the calling `ToolCallContext`'s
  locator with `Sensitivity::Internal`, matching `deliver_media`).
- **Ceilings.** Hosts running video raise the store ceiling:
  `LocalArtifactStore::try_new(root).with_max_artifact_bytes(256 MiB)`
  (or the S3 equivalent). Tools never bypass store limits; oversized
  content is rejected, never truncated.
- **Integrity.** Reads go through `ArtifactStore::get` +
  `validate_retrieved_artifact` (scope + digest + metadata); writes go
  through `stage_required_artifact`. A consuming toolset that needs a
  local file (ffmpeg) writes the verified bytes to a scratch file.
- **Frame handover to OpenRouter.** An `{"artifact"}` image input is
  fetched from the store and inlined as a `data:{media_type};base64,...`
  URI, bounded at 8 MiB pre-encoding (fail closed with
  `openrouter_media_limit_exceeded`). `{"url"}` inputs pass through.

## 5. Layer 1 — extending `finstack-ai-tools-openrouter-media`

The crate is module-per-tool (`config.rs`, `http.rs`, `image.rs`,
`video.rs`, `speech.rs`, `transcribe.rs`, `url.rs`) with shared helpers
(`send_json`, `send_bytes`, `fetch_bytes_bounded`, `deliver_media`,
`parse_arguments`, net-guard-backed bounded reads) and an optional
`artifact_store` already in its config (`with_artifact_store`). Existing
error codes are reused; one code is added:
`openrouter_media_store_required`.

Changes:

- **`openrouter_generate_video`** (extend in place, keeping the crate's
  required-with-null schema convention): new optional fields `size`
  (`WIDTHxHEIGHT`), `seed`, `generate_audio`, `first_frame`, `last_frame`
  (image inputs), `reference_images` (≤ 4 image inputs). An image input
  is exactly one of `{"url": string}` or `{"artifact": ArtifactRef}`;
  artifact inputs require the configured store and become bounded data
  URIs (spec §4). Wire mapping: `frame_images` items
  `{type:"image_url", image_url:{url}, frame_type:"first_frame"|"last_frame"}`
  and `input_references` (same, no `frame_type`), per the OpenRouter
  video API. Result unchanged: `{id, status}`.
- **`openrouter_get_video`**: unchanged. Its in-call `wait_seconds`
  (0–300) polling and `{id, status, urls}` result already cover the
  polling stage.
- **`openrouter_download_video`** (new, `video.rs` +
  `finstack.tools.openrouter_download_video`): `{id}` → GET
  `/api/v1/videos/{id}/content`, bounded by the store's
  `max_artifact_bytes`, staged via the `deliver_media` store path →
  `{"artifact", "media_type", "byte_length"}`. Requires the artifact
  store (fails closed with `openrouter_media_store_required`). Paid-tool
  metadata (`NonIdempotentWrite`, `AtMostOnce`, approval `Policy`).

`openrouter_generate_image` already stages into the store when
configured; no change needed.

## 6. Layer 2 — `finstack-ai-tools-video-compose`

T1 native toolset. Construction takes explicit absolute ffmpeg/ffprobe
binary paths (never `$PATH` or env), a required `Arc<dyn ArtifactStore>`,
a scratch directory, and a render timeout. ffmpeg only ever sees scratch
paths the toolset wrote; agents can never inject flags or paths.

- **`compose_video`** — declarative spec in, rendered movie ref out:

  ```json
  {
    "version": 1,
    "clips": [{"artifact": {...}, "trim": {"start_s": 0, "end_s": 5}}],
    "transitions": [{"type": "cut|crossfade|fade_to_black", "duration_s": 0.5}],
    "audio": {"artifact": {...}, "mode": "replace|mix", "gain_db": -6},
    "subtitles": {"artifact": {...}, "mode": "burn_in|mux",
                  "style": {"font_size": 42, "margin_v": 80}},
    "output": {"container": "mp4", "resolution": "1920x1080", "fps": 24}
  }
  ```

  JSON-Schema-enforced plus semantic validation (transition count =
  clips − 1 when present, bounded durations/gain/style, mux is mp4-only,
  ≥ 1 clip). The toolset fetches and verifies inputs to scratch files,
  probes clip durations, builds the filtergraph itself (concat / xfade /
  acrossfade / volume / subtitles filters), runs ffmpeg under timeout
  with bounded stderr-tail capture, stages the output artifact, and
  returns `{artifact, duration_s, byte_length}`.
- **`probe_media`** — ffprobe wrapper: `{artifact}` →
  `{duration_s, width?, height?, fps?, has_audio, media_type}`.
  `ReadOnly`, approval `NotRequired`.

Frozen error codes: `video_compose_config_invalid`,
`video_compose_invalid_arguments`, `video_compose_spec_invalid`,
`video_compose_ffmpeg_failed`, `video_compose_timeout`,
`video_compose_media_failure`, `video_compose_limit_exceeded`.

## 7. Layer 3 — `MoviePlan` and the pipeline executor

### 7.1 Schema

`schemas/movie-plan/movie-plan.v1.json` (registered in
`schemas/schema-families.toml`), mirrored by Rust types:

```json
{
  "version": 1,
  "title": "…",
  "defaults": {"image_model": "…", "video_model": "…",
               "resolution": "1080p", "aspect_ratio": "16:9",
               "scene_duration_s": 6},
  "scenes": [{
    "id": "scene-01",
    "video_prompt": "camera glides across …",
    "start_frame": {"prompt": "…"},
    "end_frame": {"prompt": "…"},
    "reference_images": [{"artifact": {...}}],
    "duration_s": 8,
    "seed": 42,
    "captions": [{"text": "Harbors wake up slowly.",
                  "start_s": 0.0, "end_s": 3.5}],
    "overrides": {"video_model": "…", "resolution": "…"}
  }],
  "transitions": [{"after": "scene-01", "type": "crossfade",
                   "duration_s": 0.5}],
  "audio": {"artifact": {...}, "mode": "replace"},
  "output": {"container": "mp4", "fps": 24,
             "captions": "none|sidecar|burn_in"}
}
```

A frame source is exactly one of `{"prompt"}` (generate it),
`{"artifact"}` or `{"url"}` (use as-is) — which makes single-scene
regeneration and human-in-the-loop editing natural: resubmit with one
scene's prompt changed and other frames pinned to previously produced
refs. Scene-prompt and caption authoring guidance ships as a docs
section, not code.

### 7.2 Executor crate: `extensions/workflow/finstack-ai-workflow-media-pipeline`

- **Stage graph per scene:**
  `gen_start_frame → gen_end_frame → submit → polling → download → done`
  (frame stages skipped when pinned), then one global `compose`.
- **Single execution path, tick-based.** Every stage executes by invoking
  the Layer 1/2 tools via `Toolset::call` composed in-process. The
  kernel-journaled effects are the wrapping `render_movie` /
  `advance_render` calls; per-stage durability is the adapter state
  store. Each `advance` is one bounded tick (at most one tool call per
  scene; polls use `openrouter_get_video` with a small `wait_seconds`),
  driven by the caller — an agent loop or a host loop — so there is no
  background task.
- **Adapter-owned state.** Render state (plan digest, per-scene stage →
  produced `ArtifactRef` / job id / status, optimistic revision counter)
  lives in an adapter table (`finstack_workflow_media_pipeline`) in the
  same `JournalStore` sqlite file, scoped by tenant — the workflow-local
  cron-table precedent.
- **Resume.** Resubmitting identical plan bytes resumes the persisted
  render. Recorded artifacts are verified where consumed (store `get` +
  `validate_retrieved_artifact` fail closed); a failed verification
  demotes the scene to re-run its producing stage. At-least-once, never
  exactly-once.
- **Budget guard.** `PlanLimits { max_scenes, max_total_video_s,
  max_concurrent_jobs }` enforced before the first paid call; one
  resubmit per failed video job, then the scene fails; `render_movie`
  completes with per-scene statuses and composes only when every scene
  succeeded.

### 7.3 Tool wrapper

Three tools, shared result shape `{render_id, status, per_scene: [{id,
stage, job_id?, clip_artifact?, failure?}], final_artifact?,
transcript_srt_artifact?, transcript_vtt_artifact?}`:

- **`render_movie`** — validates and persists a MoviePlan, runs one tick.
  Approval `Policy` ("paid multi-scene media generation").
- **`advance_render`** — one bounded tick. Same paid metadata.
- **`get_render_status`** — read-only, `NotRequired` approval.

Frozen error codes: `media_pipeline_config_invalid`,
`media_pipeline_invalid_arguments`, `media_pipeline_plan_invalid`,
`media_pipeline_budget_exceeded`, `media_pipeline_store_failure`,
`media_pipeline_stage_failed`, `media_pipeline_not_found`.

### 7.4 Captions and transcripts (plan-authored)

Short-form video is routinely played muted, so captions are a
first-class deliverable — authored by the same text model that writes
the plan, never derived from audio in v1.

- **Authoring**: each scene may carry up to 32 cues of `{text (1–200
  chars), start_s, end_s}` timed relative to the scene's start,
  validated as ordered, non-overlapping, inside the scene duration.
  `output.captions` selects delivery: `none` (default), `sidecar`,
  `burn_in`.
- **Transcript assembly** (driver, before compose): scene-local cues are
  flattened onto the movie timeline — scene offset = cumulative planned
  durations minus crossfade/fade overlaps, mirroring the filtergraph's
  xfade offset math over planned durations (actual clip drift is an
  accepted v1 tolerance). Whenever any scene has cues, the driver
  renders `transcript.srt` and `transcript.vtt`, stages both as
  artifacts (they are tiny), and records the refs in state and every
  tool result — the standalone upload deliverable.
- **Delivery**: `burn_in` passes the SRT artifact to `compose_video`'s
  `subtitles` (`mode: burn_in`, clamped declarative style — recommended
  for muted autoplay); `sidecar` on mp4 muxes a `mov_text` track
  (`mode: mux`); `sidecar` on webm ships the refs alone. Caption plans
  require the driver's artifact store; rejected at submit otherwise.

## 8. Composition and bindings

- SDK wiring mirrors the existing `tool-openrouter-media` feature
  pattern in `crates/finstack-ai`: `tool-video-compose` and
  `workflow-media-pipeline` optional features with spec structs on the
  linked constructors; Python kwargs + `.pyi` parity; wasm stubs reject
  with the stable native-only error; the new native crates join
  `FORBIDDEN_WASM` in `scripts/wasm_package/check.py`.
- `finstack-ai-store-artifact` stays host-injected (as today), never an
  SDK default. Nothing here is required by the default graph (G-01).

## 9. Error handling summary

- All new codes are frozen `&'static str` (per layer above); response
  bodies never surface in errors; secrets redacted from all `Debug`
  surfaces (canary tests). Fail-closed everywhere: missing store, scope
  mismatch, digest mismatch, over-limit content, invalid spec/plan,
  budget violation — before any paid call where possible.

## 10. Testing strategy

- Scripted loopback HTTP fixtures (the media toolset's existing test
  helpers) for every OpenRouter interaction: submit with frame images,
  status transitions, content download, data-URI fallback, size bounds.
- `LocalArtifactStore` over tempdirs (or the runtime's
  `InProcessArtifactStore`) as the store in toolset and driver tests —
  no new fake needed.
- Compose: stub ffmpeg/ffprobe shell scripts asserting argv; real-ffmpeg
  smoke tests behind an opt-in marker.
- Resume goldens: kill after scene 1's download, re-attach over the same
  sqlite state, assert scene 1 skipped and scene 2 resumes; corrupt a
  stored clip, assert fail-closed demotion and re-run.
- Schema: valid/invalid MoviePlan fixtures under
  `fixtures/compatibility/movie-plan/`.
- Gate: `mise run ci-rust` (+ `ci-python` / `ci-wasm` for binding
  tasks).

## 11. Delivery order

1. Layer 1 extension of the media toolset (frame-image submit fields,
   download tool).
2. `finstack-ai-tools-video-compose`.
3. MoviePlan schema + pipeline crate (plan types, state, subtitles,
   driver, tools, resume goldens).
4. SDK/binding wiring + docs + full CI.

Each step is independently shippable. No storage work is needed: hosts
compose `LocalArtifactStore` / `S3ArtifactStore` with a raised ceiling.
