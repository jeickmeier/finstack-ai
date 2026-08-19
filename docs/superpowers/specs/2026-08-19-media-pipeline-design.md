# Media Generation Pipeline — Design

**Date:** 2026-08-19
**Status:** Approved design, pre-plan
**Depends on:** `docs/superpowers/plans/2026-08-19-openrouter-provider.md` (all
three phases run to completion first; this design builds on the finished
`finstack-ai-provider-openrouter` and `finstack-ai-tools-openrouter-media`
crates and does not amend that plan)

## 1. Goal

Add a composable, AI-plannable video generation pipeline to finstack-ai:

1. A text model authors per-scene descriptive prompts (start image, end
   image, video motion) — a prompt/skill concern, not code.
2. Images are generated through OpenRouter (`POST /api/v1/images`).
3. Videos are generated through OpenRouter's async video API
   (`POST /api/v1/videos` → poll → download), with optional first-frame,
   last-frame, and reference images, resolution, and duration.
4. Clips are merged into a final movie with a host-supplied `ffmpeg`
   binary driven by a declarative composition spec.

The AI controls the pipeline at two altitudes: calling the media tools
directly (scene-by-scene iteration), or authoring a versioned `MoviePlan`
document and submitting it to a single `render_movie` tool executed by a
deterministic, resumable pipeline driver.

Storage of intermediate and final media is host composition — local
filesystem or S3, chosen at construction. Agents never see or choose the
backend; they hold opaque refs.

## 2. Non-goals

- No webhook (`callback_url`) or `provider` passthrough exposure to agents
  in v1 (host infrastructure and uncontrolled cost surface respectively).
- No titles, overlays, subtitles, or Ken Burns effects in the v1
  composition spec (versioned for later addition).
- No moviepy or Remotion integration; composition is ffmpeg-only, native.
- No transcription-derived captions in v1: caption cues are plan-authored
  by the text model. Aligning generated audio to word-level timestamps is
  an agent-level flow over the existing `openrouter_transcribe_audio`
  tool, not pipeline machinery.
- No kernel or protocol changes. The kernel's six-port model is untouched;
  everything here is a runtime service contract plus opt-in leaves (G-01
  holds: the default graph depends on none of this).
- No exactly-once claims: recovery is at-least-once, consistent with the
  kernel's commit-before-effect story.

## 3. Architecture

Four layers, each an independently useful opt-in leaf:

```
Layer 4  extensions/workflow/finstack-ai-workflow-media-pipeline
         MoviePlan schema, deterministic executor, render_movie tool
Layer 3  extensions/toolsets/finstack-ai-tools-video-compose
         Declarative ffmpeg composition (merge, transitions, audio)
Layer 2  extensions/toolsets/finstack-ai-tools-openrouter-media
         (existing crate, extended) image gen + async video job tools
Layer 1  crates/finstack-ai-runtime          MediaStore contract (trait)
         extensions/stores/finstack-ai-store-media-local
         extensions/stores/finstack-ai-store-media-s3
```

Kernel and runtime never depend on any extension crate. The runtime gains
only the `MediaStore` trait and its ref/error types, introduced by ADR
following the ADR-048 / `MediaResolver` precedent (host-supplied object,
not a registered port).

## 4. Layer 1 — `MediaStore` runtime contract

### 4.1 Motivation

`ArtifactStore` stores exact bytes with a 4 MiB ceiling
(`MAX_ARTIFACT_BYTES`) — right for plans, prompts, and manifests, wrong
for video clips. `MediaStore` is its large-object sibling: a new adjacent
trait, not a modification of `ArtifactStore`.

### 4.2 Contract (in `finstack-ai-runtime`, no new dependencies)

```rust
/// Opaque, serializable, journal-safe reference to stored media.
pub struct MediaRef {
    id: Arc<str>,        // backend-scoped key; never a raw path or URL
    digest: Digest,      // content digest; verified on materialize
    length: u64,
    media_type: Arc<str>,
    scope_digest: Digest, // ArtifactScope binding, same as artifacts
}

pub trait MediaStore: PortObject {
    /// Ingest a local file's exact bytes into the store.
    fn put_file(&self, scope: ArtifactScope, local_path: PathBuf,
                metadata: MediaMetadata)
        -> PortFuture<Result<MediaRef, MediaError>>;

    /// Produce a readable local file for a ref; digest-verified.
    fn materialize(&self, scope: ArtifactScope, media: &MediaRef)
        -> PortFuture<Result<MaterializedMedia, MediaError>>;

    /// Time-limited HTTPS GET URL when the backend supports it.
    fn presign_get(&self, scope: ArtifactScope, media: &MediaRef)
        -> PortFuture<Result<Option<Url>, MediaError>>;

    /// Permanently remove stored content.
    fn delete(&self, scope: ArtifactScope, media: &MediaRef)
        -> PortFuture<Result<(), MediaError>>;
}
```

- **Streaming by path, not bytes.** `put_file` reads from a local file;
  `materialize` returns `MaterializedMedia { local_path, _guard }` — the
  stored path itself for the local backend (zero-copy), a scoped temp
  download for S3 (guard deletes it on drop). Multi-GB files never pass
  through memory or the journal.
- **Scope binding** reuses `ArtifactScope` verbatim (tenant scope, session,
  optional run, sensitivity). All operations verify the ref's
  `scope_digest`; mismatch fails closed (`media_scope_mismatch`).
- **Integrity**: `materialize` re-hashes content and fails closed on
  digest mismatch (`media_integrity_failure`). This is what makes resumed
  pipelines trustworthy.
- **`presign_get`** is `Ok(None)` for backends without presigning (local).
  Callers needing an outbound URL fall back to a bounded `data:` URI from
  `materialize`.
- **Frozen error codes**: `media_unavailable`, `media_not_found`,
  `media_scope_mismatch`, `media_integrity_failure`, `media_too_large`,
  `media_invalid_metadata`, `media_io_failure`.

### 4.3 Backends (extension leaves under `extensions/stores/`)

- **`finstack-ai-store-media-local`**: explicit root directory at
  construction (never from env), content-addressed layout
  (`{root}/{scope-digest-prefix}/{content-digest}`), atomic
  write-then-rename ingest, optional total-bytes quota. No new
  dependencies.
- **`finstack-ai-store-media-s3`**: bucket + key prefix + region +
  credentials (explicit, ADR-048 style) + presign TTL. This crate alone
  takes the AWS SDK dependency — quarantined in one leaf so the runtime
  and other extensions stay dependency-clean. `materialize` streams to a
  temp file under an explicit scratch dir; `presign_get` returns presigned
  HTTPS GET URLs (which is how generated frames are fed to OpenRouter's
  video API without inlining bytes).

### 4.4 Relationship to `ArtifactStore`

Small documents (MoviePlans, scene manifests, prompt sets) remain
`ArtifactStore` artifacts. Media bytes live in `MediaStore`. Tool results
carry `MediaRef`s; the journal record of the tool effect is the durable
link between the two. No pointer-manifest indirection is needed because
`MediaRef` itself carries digest, length, media type, and scope.

## 5. Layer 2 — extending `finstack-ai-tools-openrouter-media`

Applied after the OpenRouter plan completes. The crate constructor gains
`media_store: Option<Arc<dyn MediaStore>>` plus the `ArtifactScope` to
operate under. Absent store ⇒ current inline-bounded behavior is
preserved (minimal graphs keep working). The crate is unpublished, so the
one-shot `openrouter_generate_video` tool is removed and replaced —
no deprecation shim.

Wire contract (from
<https://openrouter.ai/docs/guides/overview/multimodal/video-generation>):
`POST /api/v1/videos` accepts `model`, `prompt`, and optional `duration`
(seconds), `resolution` (`480p`…`4K`), `aspect_ratio`, `size`
(`WIDTHxHEIGHT`), `seed`, `generate_audio`, `frame_images` (items
`{type:"image_url", image_url:{url}, frame_type:"first_frame"|"last_frame"}`),
`input_references` (same shape, no `frame_type`). Response is `202` with
`{id, polling_url, status}`; `GET /api/v1/videos/{id}` polls
(`pending|in_progress|completed|failed`, `unsigned_urls`, `usage.cost`);
`GET /api/v1/videos/{id}/content` serves bytes. Wire DTOs are re-verified
against live docs at implementation time; the tool surface below is fixed.

Tool surface (existing image/speech/transcribe tools unchanged except as
noted; all paid tools keep `ApprovalRequirement::Policy` with reason
"paid OpenRouter media generation"):

- **`openrouter_generate_image`** (behavior extension): with a
  `MediaStore` wired, `b64_json` responses are decoded and stored via
  `put_file`; the result becomes `{media_ref, media_type, url?}`. Image
  size is no longer bounded by the 256 KiB result cap. Without a store,
  behavior is exactly as the OpenRouter plan specifies.
- **`openrouter_submit_video`** (new; replaces `openrouter_generate_video`):
  input `model`, `prompt`, optional `duration_s` (integer), `resolution`,
  `aspect_ratio`, `size`, `seed`, `generate_audio`, `first_frame`,
  `last_frame`, `reference_images[]` (each image input is a tagged union:
  `{"url": string}` passed through, or `{"media_ref": string}` resolved via
  `presign_get`, falling back to a `data:` URI from `materialize`, bounded
  at 8 MiB pre-encoding — over-limit fails closed with
  `openrouter_media_limit_exceeded`). Output
  `{job_id, status, polling_url}`. `NonIdempotentWrite`, `AtMostOnce`.
- **`openrouter_get_video_job`** (new): `{job_id}` →
  `{job_id, status, unsigned_urls?, cost?}`. Read-only, idempotent, no
  approval gate — the cheap polling primitive.
- **`openrouter_download_video`** (new): `{job_id}` → streams
  `/content` to a temp file, then `put_file` →
  `{media_ref, media_type, length}`. Requires a `MediaStore`; fails closed
  with `openrouter_media_store_required` when absent. Never buffers the
  clip in memory.

New frozen error code: `openrouter_media_store_required`. All other codes
from the OpenRouter plan are reused.

## 6. Layer 3 — `finstack-ai-tools-video-compose`

T1 native toolset. Construction takes an explicit ffmpeg binary path and
ffprobe binary path (never `$PATH` discovery, never env), a required
`Arc<dyn MediaStore>` + `ArtifactScope`, a scratch directory, and a
wall-clock timeout. ffmpeg only ever receives paths produced by
`materialize()` and an output path inside the scratch dir; agents can
never inject flags or paths.

- **`compose_video`** — declarative composition spec in, rendered movie
  ref out:

  ```json
  {
    "version": 1,
    "clips": [{"media_ref": "...", "trim": {"start_s": 0, "end_s": 5}}],
    "transitions": [{"type": "cut|crossfade|fade_to_black", "duration_s": 0.5}],
    "audio": {"media_ref": "...", "mode": "replace|mix", "gain_db": -6},
    "subtitles": {"media_ref": "...", "mode": "burn_in|mux",
                  "style": {"font_size": 42, "margin_v": 80}},
    "output": {"container": "mp4", "resolution": "1920x1080", "fps": 24}
  }
  ```

  Validation is JSON-Schema-enforced at the tool layer plus semantic
  checks (`transitions.len() == clips.len() - 1` when present, bounded
  durations and gain, known enums, ≥1 clip). The toolset builds the
  filtergraph itself (concat / xfade / afade / amix), normalizes clip
  parameters (scale/fps/SAR) before concat, runs ffmpeg under timeout with
  bounded stderr-tail capture into errors, ingests the output via
  `put_file`, and returns `{media_ref, duration_s, length}`.
  `NonIdempotentWrite`, `AtMostOnce`, approval `Policy`
  ("local media rendering").
- **`probe_media`** — `{media_ref}` →
  `{duration_s, width, height, fps, has_audio, media_type}` via ffprobe
  JSON output. Idempotent, no approval.

Frozen error codes: `video_compose_config_invalid`,
`video_compose_invalid_arguments`, `video_compose_spec_invalid`,
`video_compose_ffmpeg_failed`, `video_compose_timeout`,
`video_compose_media_failure`, `video_compose_limit_exceeded`.

## 7. Layer 4 — `MoviePlan` and the pipeline executor

### 7.1 Schema

`schemas/movie-plan/movie-plan.v1.json` (registered in
`schemas/schema-families.toml`), shipped in-repo so hosts, drivers, and
prompt assets share one contract:

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
    "reference_images": [{"media_ref": "…"}],
    "duration_s": 8,
    "seed": 42,
    "captions": [{"text": "Harbors wake up slowly.",
                  "start_s": 0.0, "end_s": 3.5}],
    "overrides": {"video_model": "…", "resolution": "…"}
  }],
  "transitions": [{"after": "scene-01", "type": "crossfade",
                   "duration_s": 0.5}],
  "audio": {"media_ref": "…", "mode": "replace"},
  "output": {"container": "mp4", "fps": 24,
             "captions": "none|sidecar|burn_in"}
}
```

A frame is a tagged union: `{"prompt"}` means generate it;
`{"media_ref"}` or `{"url"}` means use it as-is. `end_frame`,
`reference_images`, `duration_s`, `seed`, `resolution` are optional —
matching the video API's optional surface. Pinning refs is what makes
single-scene regeneration and human-in-the-loop editing natural:
resubmit the plan with one scene's prompt changed and all other frames
pinned to previously produced refs.

Scene-prompt authoring guidance (how a text model writes good start/end
image prompts and motion prompts) ships as a prompt asset / skill
document alongside the crate, not as code.

### 7.2 Executor crate: `extensions/workflow/finstack-ai-workflow-media-pipeline`

A runtime driver in the `finstack-ai-workflow-local` mold — drives
sessions, owns adapter state, never owns a kernel DAG.

- **Stage graph.** Per scene:
  `gen_start_frame → gen_end_frame → submit_video → poll → download`
  (frame stages skipped when pinned). Scenes fan out under a configurable
  concurrency cap (default 2 in-flight video jobs). One global stage
  follows: `compose → final MediaRef`.
- **Single execution path, tick-based.** Every stage executes by invoking
  the same Layer 2/3 tools an agent would call, via `Toolset::call`
  composed in-process. The kernel-journaled effects are the wrapping
  `render_movie`/`advance_render` tool calls; per-stage durability is the
  adapter state store. Each `advance` is one bounded tick (at most one
  tool call per scene) driven by the caller — an agent loop or a host
  loop — so there is no background task and no internal polling cadence
  to schedule.
- **Adapter-owned state.** Driver state (plan digest, per-scene stage →
  produced `MediaRef` / job id / status, optimistic revision counter)
  lives in an adapter table (`finstack_workflow_media_pipeline`) in the
  same `JournalStore` sqlite file, scoped by tenant — exactly the
  workflow-local cron-table precedent.
- **Resume.** On re-attach: stages with recorded outputs verify their
  `MediaRef` digests via `materialize` and are skipped; verification
  failure re-runs the stage; an in-flight video job resumes at `poll`
  using the recorded job id. At-least-once, never exactly-once.
- **Budget guard.** Plan validation computes scene count and total
  requested seconds; the driver enforces host-set ceilings (max scenes,
  max total video seconds, max concurrent jobs, max poll duration per
  job) before submitting anything. Agents author plans; hosts bound
  spend. Violations fail closed before the first paid call.
- **Failure policy.** A failed scene stage retries within the video API's
  own retry semantics (poll `failed` ⇒ one resubmit if the host allows,
  else the scene is marked failed); `render_movie` completes with
  per-scene statuses rather than aborting the whole movie on one bad
  scene, and `compose` runs only when every scene succeeded.

### 7.3 Tool wrapper

The same crate exposes a three-tool toolset (shared result shape:
`{render_id, status, per_scene: [{id, stage, job_id?, clip_ref?,
failure?}], final_media_ref?}`):

- **`render_movie`** — input: a MoviePlan (validated against the plan
  contract), submits the render and runs one tick. Approval `Policy`
  ("paid multi-scene media generation"). Resubmitting identical plan
  bytes resumes the existing render instead of duplicating it.
- **`advance_render`** — `{render_id}`; runs one bounded tick (submit due
  jobs, poll in-flight jobs once, download completed clips, compose when
  all scenes are done). Same paid approval metadata; this is how an agent
  or host loop drives a long render to completion without blocking.
- **`get_render_status`** — `{render_id}` → same shape, read-only.
  Idempotent, no approval.

Frozen error codes: `media_pipeline_config_invalid`,
`media_pipeline_invalid_arguments`, `media_pipeline_plan_invalid`,
`media_pipeline_budget_exceeded`, `media_pipeline_store_failure`,
`media_pipeline_stage_failed`, `media_pipeline_not_found`.

### 7.4 Captions and transcripts (plan-authored)

Short-form video is routinely played muted, so the pipeline treats
captions as a first-class deliverable — authored by the same text model
that writes the plan, never derived from audio in v1.

- **Authoring**: each scene may carry `captions` — up to 32 cues of
  `{text (1–200 chars), start_s, end_s}` timed relative to the scene's
  own start, validated as ordered, non-overlapping, and inside the scene
  duration. `output.captions` selects delivery: `none` (default),
  `sidecar`, or `burn_in`.
- **Transcript assembly** (pipeline driver, before compose): scene-local
  cues are flattened onto the movie timeline — scene offset = cumulative
  planned durations minus crossfade/fade overlaps, mirroring the
  filtergraph's xfade offset math over planned durations (actual clip
  drift is an accepted v1 tolerance). Whenever any scene has cues, the
  driver renders `transcript.srt` and `transcript.vtt`, stores both via
  `MediaStore`, and records `transcript_srt_ref`/`transcript_vtt_ref` in
  the render state and every tool result — the standalone upload
  deliverable for platforms that accept subtitle files.
- **Delivery**: `burn_in` passes the SRT to `compose_video`'s
  `subtitles: {media_ref, mode: burn_in, style?: {font_size, margin_v}}`
  (ffmpeg `subtitles=` filter, clamped declarative style — recommended
  for muted autoplay); `sidecar` on mp4 muxes a `mov_text` track
  (`mode: mux`); `sidecar` on webm ships the refs alone. Plans using
  captions require a configured `MediaStore`; the driver rejects them at
  submit otherwise.

## 8. Composition and bindings

- SDK wiring follows the E2B / OpenRouter-media precedent: optional
  workspace dependencies behind the `native-tokio` feature, spec structs
  on the linked constructors, Python kwargs + `.pyi` parity, wasm-bindgen
  stubs that reject with a stable "native only" error (ffmpeg and S3 do
  not exist on wasm).
- Nothing here is required by the default graph (G-01). A host opts in
  per layer: store backend, media tools, compose tools, pipeline driver.

## 9. Error handling summary

- Every crate's error codes are frozen `&'static str` (listed per layer
  above); response bodies are never surfaced in errors; secrets are
  redacted from all `Debug` surfaces (canary tests mandatory, matching
  the OpenRouter plan's constraint).
- All fail-closed: missing store, scope mismatch, digest mismatch,
  over-limit inline media, invalid spec/plan, budget violation — before
  any paid call where possible.

## 10. Testing strategy

- **Unit + scripted loopback fixtures** (E2B `serve_scripted` style) for
  every OpenRouter endpoint interaction: submit → 202, poll transitions
  (`pending → in_progress → completed`, and `failed`), content download
  streaming, frame-image data-URI fallback, size bounds.
- **Fake `MediaStore`** in `finstack-ai-test` for toolset and driver
  tests; local-backend crate tested against tempdirs (atomicity, digest
  verification, quota); S3 backend tested against a scripted HTTP fixture
  (presign shape, streaming), not live AWS.
- **Compose**: a stub ffmpeg/ffprobe binary asserting argv shape for unit
  tests; real-ffmpeg smoke tests behind an opt-in feature/ignored marker.
- **Resume goldens**: kill the driver after scene 1's download, re-attach,
  assert scene 1 is skipped via digest verification and scene 2 resumes
  at the recorded stage; corrupt a stored file, assert fail-closed re-run.
- **Schema**: valid/invalid MoviePlan and composition-spec fixtures under
  `fixtures/`, exercised from both Rust validation and the JSON Schema.
- Verification gate: `mise run ci-rust` (plus `ci-python` / `ci-wasm` for
  binding tasks).

## 11. Delivery order

1. ADR + `MediaStore` contract in the runtime + fake store in
   `finstack-ai-test`.
2. `finstack-ai-store-media-local`.
3. Layer 2 extension of `finstack-ai-tools-openrouter-media`
   (video tool trio + store integration).
4. `finstack-ai-tools-video-compose`.
5. MoviePlan schema + `finstack-ai-workflow-media-pipeline` driver +
   `render_movie`/`get_render_status` tools.
6. `finstack-ai-store-media-s3`.
7. SDK/binding wiring + docs + full CI.

Each step is independently shippable; S3 lands last because every prior
layer is testable against the local backend.
