# Media Generation Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an AI-plannable video pipeline: frame-image video submission and a download tool on the existing OpenRouter media toolset, a declarative ffmpeg composition toolset with plan-authored captions, and a resumable tick-based `MoviePlan` driver exposed as `render_movie`/`advance_render`/`get_render_status` tools — all storing media as `ArtifactRef`s through the existing artifact system.

**Architecture:** No storage work and no kernel/runtime changes. Layer 1 extends `extensions/toolsets/finstack-ai-tools-openrouter-media` (which already ships async `openrouter_generate_video`, polling `openrouter_get_video`, and artifact-staged image/speech results). Layer 2 is a new T1 compose toolset shelling out to host-supplied ffmpeg. Layer 3 is a new workflow crate with adapter-owned sqlite state (workflow-local cron-table precedent). Hosts inject `LocalArtifactStore`/`S3ArtifactStore` from `extensions/artifacts/finstack-ai-store-artifact` with `with_max_artifact_bytes` raised for video.

**Tech Stack:** Rust workspace (edition/lints inherited), workspace-pinned `reqwest`, `tokio`, `serde`/`serde_json`, `rusqlite`, `base64`, `tempfile`, `thiserror`, `futures-util`. **No new dependencies.**

**Spec:** `docs/superpowers/specs/2026-08-19-media-pipeline-design.md`

## Global Constraints

- **Baseline (verified in-tree):** `finstack-ai-tools-openrouter-media` is module-per-tool (`config.rs`, `http.rs`, `image.rs`, `video.rs`, `speech.rs`, `transcribe.rs`, `url.rs`); its config carries `api_key`, `endpoint`, `referer`, `title`, `max_result_bytes`, and an optional store via `with_artifact_store(Arc<dyn ArtifactStore>)`; `http.rs` provides `send_json`, `send_bytes` (returns `(Vec<u8>, Option<String>)` body + content type), `fetch_bytes_bounded`, `deliver_media`, `parse_arguments`, `invalid_arguments`, `tool_error`, `timeout_error`, `wait_deadline`, all bounded/interruptible via `finstack-ai-net-guard`. Its test module has `tool_context()`, `find_spec()`, and scripted `respond` fixtures — reuse them.
- **Import conventions (migrated runtime API):** kernel types from `finstack_ai_kernel::{...}`; runtime port types from `finstack_ai_runtime::ports::PortFuture`, `finstack_ai_runtime::ports::model::{ApprovalMetadata, ApprovalRequirement, SideEffectClass, ToolDeferralSupport, ToolSpec}`, `finstack_ai_runtime::ports::tool::{ToolCallContext, ToolError, ToolEventStream, ToolResult, ToolStreamItem, Toolset, ToolsetDescriptor, verify_authority}`; artifact service from `finstack_ai_runtime::artifact::{ArtifactMetadata, ArtifactScope, ArtifactStore, stage_required_artifact, validate_retrieved_artifact}`; `finstack_ai_runtime::Bytes`. There are no flat root re-exports.
- **Enum spellings:** `SideEffectClass::ReadOnly` (not `Read`), `ApprovalRequirement::NotRequired` (not `Never`), `RetrySafety::AtMostOnce` for every tool including read-only ones (in-tree precedent: `openrouter_get_video`).
- **Scope convention:** artifact scopes are built per call from the context, exactly as `deliver_media` does:

  ```rust
  ArtifactScope {
      tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
      session_id: ctx.run.locator.session_id,
      run_id: Some(ctx.run.locator.run_id),
      sensitivity: Sensitivity::Internal,
  }
  ```

- Workspace layout is contractual: toolsets in `extensions/toolsets/`, workflow drivers in `extensions/workflow/`; kernel/runtime never depend on an extension crate. `extensions/stores/` is journal stores and `extensions/artifacts/` is artifact storage — this plan creates crates in neither.
- Extensions never read environment variables; binary paths, roots, and credentials are explicit construction inputs. All `Debug` impls redact secrets. Response bodies never surface in errors.
- Frozen error codes (each `&'static str`):
  - openrouter media addition: `openrouter_media_store_required` (all existing `openrouter_media_*` codes reused)
  - compose: `video_compose_config_invalid`, `video_compose_invalid_arguments`, `video_compose_spec_invalid`, `video_compose_ffmpeg_failed`, `video_compose_timeout`, `video_compose_media_failure`, `video_compose_limit_exceeded`
  - pipeline: `media_pipeline_config_invalid`, `media_pipeline_invalid_arguments`, `media_pipeline_plan_invalid`, `media_pipeline_budget_exceeded`, `media_pipeline_store_failure`, `media_pipeline_stage_failed`, `media_pipeline_not_found`
- Frozen tool identities: existing `finstack.tools.openrouter_generate_video`/`openrouter_generate_video` and `finstack.tools.openrouter_get_video`/`openrouter_get_video` are kept; new: `finstack.tools.openrouter_download_video`/`openrouter_download_video`, `finstack.tools.compose_video`/`compose_video`, `finstack.tools.probe_media`/`probe_media`, `finstack.tools.render_movie`/`render_movie`, `finstack.tools.advance_render`/`advance_render`, `finstack.tools.get_render_status`/`get_render_status`.
- **Schema convention:** the media toolset's input schemas list every property in `required` and mark optional ones with `"type": ["...", "null"]` (agents must pass explicit nulls). New fields on its tools follow that convention. The new compose/pipeline crates use plain optional properties (`additionalProperties: false`, optionals omitted from `required`) — pick per crate, never mixed within one.
- Every new crate copies the lint header block verbatim from `extensions/toolsets/finstack-ai-sandbox-e2b/src/lib.rs:7-26` and carries `[lints] workspace = true`.
- Media bytes never enter tool-result JSON when a store is configured; results carry `{"artifact": <ArtifactRef JSON>, "media_type", "byte_length"}` exactly like the existing image/speech tools. Store ceilings (`ArtifactStore::limits().max_artifact_bytes`) bound every media object; oversized content is rejected, never truncated.
- Verification gate per task: the task's test command; for the plan: `mise run ci-rust` (Task 12 adds `ci-python`/`ci-wasm`).
- Commit style: plain imperative sentences (match `git log`), each ending with the trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Wire-shape re-verification: before Task 1, fetch `https://openrouter.ai/docs/guides/overview/multimodal/video-generation` and confirm `frame_images` (items `{type:"image_url", image_url:{url}, frame_type:"first_frame"|"last_frame"}`), `input_references` (same, no `frame_type`), `size`, `seed`, `generate_audio`, and `GET /api/v1/videos/{id}/content`. Tool surfaces are fixed; only private wire DTOs may move.

---

## Phase A — OpenRouter media toolset extension

### Task 1: Frame images and generation controls on `openrouter_generate_video`

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-openrouter-media/src/video.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-openrouter-media/src/lib.rs` (video ToolSpec input schema + dispatch passes the store)

**Interfaces:**
- Consumes: the crate's existing `send_json`, `parse_arguments`, `invalid_arguments`, `tool_error`, config store (`artifact_store: Option<Arc<dyn ArtifactStore>>`), and `OPENROUTER_MEDIA_LIMIT_EXCEEDED`/`OPENROUTER_MEDIA_INVALID_ARGUMENTS`.
- Produces (in `video.rs`, `pub(crate)`): `struct ImageInput { url: Option<String>, artifact: Option<ArtifactRef> }` (serde, `deny_unknown_fields`, both `#[serde(default)]`), `const MAX_INLINE_IMAGE_BYTES: usize = 8 * 1_048_576;`, `async fn resolve_image_input(store: Option<&Arc<dyn ArtifactStore>>, ctx: &ToolCallContext, input: &ImageInput) -> Result<String, ToolError>`, and `fn artifact_scope(ctx: &ToolCallContext) -> ArtifactScope` (the Global Constraints scope, shared with Task 2). `handle_video_submit` gains a `store: Option<&Arc<dyn ArtifactStore>>` parameter. Result shape unchanged: `{"id","status"}`.

- [ ] **Step 1: Extend `VideoArguments` and the wire body**

In `video.rs`:

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VideoArguments {
    model: String,
    prompt: String,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    aspect_ratio: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    seed: Option<i64>,
    #[serde(default)]
    generate_audio: Option<bool>,
    #[serde(default)]
    first_frame: Option<ImageInput>,
    #[serde(default)]
    last_frame: Option<ImageInput>,
    #[serde(default)]
    reference_images: Option<Vec<ImageInput>>,
}
```

`resolve_image_input` — exactly one of `url`/`artifact`; artifact inputs require the store and become bounded data URIs:

```rust
pub(crate) async fn resolve_image_input(
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    input: &ImageInput,
) -> Result<String, ToolError> {
    match (&input.url, &input.artifact) {
        (Some(url), None) => Ok(url.clone()),
        (None, Some(artifact)) => {
            let Some(store) = store else {
                return Err(tool_error(
                    OPENROUTER_MEDIA_STORE_REQUIRED,
                    ErrorCategory::Configuration,
                    "openrouter media artifact frame inputs require an artifact store",
                ));
            };
            let scope = artifact_scope(ctx);
            let bytes = store
                .get(scope.clone(), artifact.clone())
                .await
                .map_err(|_| tool_error(
                    OPENROUTER_MEDIA_TRANSPORT_FAILED,
                    ErrorCategory::Tool,
                    "openrouter media frame artifact read failed",
                ))?;
            validate_retrieved_artifact(&scope, artifact, &bytes).map_err(|_| {
                tool_error(
                    OPENROUTER_MEDIA_TRANSPORT_FAILED,
                    ErrorCategory::Tool,
                    "openrouter media frame artifact failed verification",
                )
            })?;
            if bytes.len() > MAX_INLINE_IMAGE_BYTES {
                return Err(tool_error(
                    OPENROUTER_MEDIA_LIMIT_EXCEEDED,
                    ErrorCategory::Limit,
                    "openrouter media frame image exceeds the inline data-URI ceiling",
                ));
            }
            Ok(format!(
                "data:{};base64,{}",
                artifact.blob().media_type(),
                BASE64_STANDARD.encode(&bytes)
            ))
        }
        _ => Err(invalid_arguments(
            "openrouter media image input must set exactly one of url or artifact",
        )),
    }
}
```

(`BASE64_STANDARD` and the error helpers are imported the way `http.rs` imports them; `ErrorCategory` from `finstack_ai_kernel`. If `blob().media_type()` is not the accessor pair on the current `ArtifactRef`, open `crates/finstack-ai-kernel/src/primitives/handles.rs` and use the real one — the media toolset's own `deliver_media` result construction shows the current shape.)

In `handle_video_submit` (now also taking `store`), after the existing optional inserts, build and insert the wire arrays:

```rust
let mut frame_images = Vec::new();
for (input, frame_type) in [
    (arguments.first_frame.as_ref(), "first_frame"),
    (arguments.last_frame.as_ref(), "last_frame"),
] {
    if let Some(input) = input {
        let url = resolve_image_input(store, ctx, input).await?;
        frame_images.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": url },
            "frame_type": frame_type,
        }));
    }
}
let mut input_references = Vec::new();
for input in arguments.reference_images.iter().flatten() {
    if input_references.len() >= 4 {
        return Err(invalid_arguments(
            "openrouter media accepts at most four reference images",
        ));
    }
    let url = resolve_image_input(store, ctx, input).await?;
    input_references.push(serde_json::json!({
        "type": "image_url",
        "image_url": { "url": url },
    }));
}
```

then insert `size`/`seed`/`generate_audio` like the existing optional fields, and `frame_images`/`input_references` only when non-empty.

- [ ] **Step 2: Update the ToolSpec input schema in `lib.rs`**

Extend the video spec's schema following the crate's required-with-null convention — every new key joins `required` with a null-able type. New properties (descriptions in the same style as the existing ones):

```text
"size":             {"type": ["string","null"],  "description": "Pixel dimensions as WIDTHxHEIGHT. Null uses the model default."}
"seed":             {"type": ["integer","null"], "description": "Deterministic generation seed. Null lets the provider choose."}
"generate_audio":   {"type": ["boolean","null"], "description": "Generate audio when the model supports it. Null uses the model default."}
"first_frame":      image-input-or-null
"last_frame":       image-input-or-null
"reference_images": {"type": ["array","null"], "items": <image input>, "maxItems": 4, "description": "Style or content reference images."}
```

where `<image input>` is:

```json
{"additionalProperties":false,"properties":{"artifact":{"description":"Artifact reference to a stored image, e.g. from openrouter_generate_image.","type":"object"},"url":{"description":"Publicly fetchable image URL.","minLength":1,"type":"string"}},"type":"object"}
```

(`first_frame`/`last_frame` as `{"anyOf":[<image input>,{"type":"null"}]}` — if `ToolSpec::validate()` rejects `anyOf`, use `{"type":["object","null"], ...properties...}` instead.) `required` becomes the alphabetical list of all properties. Update the tool description to mention frame images: append `" Optional first/last frame and reference images accept a URL or a stored artifact reference."`

The dispatch arm for `VIDEO_TOOL_NAME` passes `artifact_store.as_ref()` through to `handle_video_submit` (the closure already clones `artifact_store` for the image arm).

- [ ] **Step 3: Tests**

Using the existing test helpers (`tool_context()`, `find_spec`, scripted `respond`; store = `finstack_ai_runtime::artifact::InProcessArtifactStore` — its constructor is in `crates/finstack-ai-runtime/src/services/artifact_in_process.rs`; stage a small PNG into it first with `stage_required_artifact` under the `tool_context()` scope — tenant `tenant-a`, session `[1;16]`, run `[3;16]`, `Sensitivity::Internal`; check the helper's actual locator bytes and mirror them):

1. `video_submit_maps_generation_controls_and_frame_images` — args with `seed: 42`, `size: "1280x720"`, `generate_audio: false`, `first_frame: {"artifact": <staged ref>}`, `last_frame: {"url": "https://example.test/last.png"}`, nulls for the rest; scripted 202 `{"id":"vid-1","status":"pending"}`. Assert the captured body has `seed == 42`, `size == "1280x720"`, `generate_audio == false`, `frame_images[0].frame_type == "first_frame"`, `frame_images[0].image_url.url` starting `"data:image/png;base64,"`, `frame_images[1].frame_type == "last_frame"` with the literal URL, and no `input_references` key.
2. `video_submit_without_store_rejects_artifact_frames` — toolset built without a store; `first_frame: {"artifact": ...}` → error code `openrouter_media_store_required`; nothing reaches the fixture.
3. `video_submit_rejects_ambiguous_image_input` — `{"url": ..., "artifact": ...}` → `openrouter_media_invalid_arguments`.
4. `oversized_frame_artifact_fails_closed` — stage an artifact larger than a test-shrunk inline ceiling: route the limit through `fn inline_image_ceiling() -> usize` returning `MAX_INLINE_IMAGE_BYTES` normally and `1_024` under `#[cfg(test)]`; stage 4 KiB → `openrouter_media_limit_exceeded`.
5. `video_submit_rejects_a_fifth_reference_image` — five `{"url"}` reference images → `openrouter_media_invalid_arguments`.

Also update any existing video-submit test whose argument JSON now fails `deny_unknown_fields` (none should — new fields are additive and optional at the serde layer).

- [ ] **Step 4: Run**

Run: `cargo test -p finstack-ai-tools-openrouter-media`
Expected: PASS — new tests green, all existing tests untouched and green.

- [ ] **Step 5: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-openrouter-media
git commit -m "Add frame images, references, seed, size, and audio to video submission"
```

---

### Task 2: `openrouter_download_video`

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-openrouter-media/src/config.rs` (new error code)
- Modify: `extensions/toolsets/finstack-ai-tools-openrouter-media/src/video.rs` (download handler)
- Modify: `extensions/toolsets/finstack-ai-tools-openrouter-media/src/lib.rs` (ToolSpec + dispatch)

**Interfaces:**
- Consumes: `send_bytes`, `deliver_media`, `percent_encode_path_segment` (already in `video.rs`), Task 1's `artifact_scope`.
- Produces: tool `openrouter_download_video` — input `{"id": string}`; result `{"artifact": <ArtifactRef JSON>, "media_type", "byte_length"}` (exactly `deliver_media`'s store-path shape); `pub const OPENROUTER_MEDIA_STORE_REQUIRED: &str = "openrouter_media_store_required";` in `config.rs` next to the other codes. The Task 9 driver relies on these names.

- [ ] **Step 1: Error code and handler**

`config.rs` gains the constant with the same doc-comment style as its siblings. `video.rs`:

```rust
pub(crate) const VIDEO_DOWNLOAD_TOOL_ID: &str = "finstack.tools.openrouter_download_video";
pub(crate) const VIDEO_DOWNLOAD_TOOL_NAME: &str = "openrouter_download_video";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VideoDownloadArguments {
    id: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_video_download(
    client: &reqwest::Client,
    authorization: &HeaderValue,
    referer: Option<&str>,
    title: Option<&str>,
    endpoint: &str,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    arguments: &[u8],
) -> Result<serde_json::Value, ToolError> {
    let arguments: VideoDownloadArguments = parse_arguments(arguments)?;
    if arguments.id.is_empty() {
        return Err(invalid_arguments("openrouter media video id is empty"));
    }
    let Some(store) = store else {
        return Err(tool_error(
            OPENROUTER_MEDIA_STORE_REQUIRED,
            ErrorCategory::Configuration,
            "openrouter media video download requires an artifact store",
        ));
    };
    let cap = store.limits().max_artifact_bytes;
    let url = format!(
        "{endpoint}/api/v1/videos/{}/content",
        percent_encode_path_segment(&arguments.id)
    );
    let (bytes, content_type) = send_bytes(
        client,
        authorization,
        referer,
        title,
        reqwest::Method::GET,
        &url,
        None,
        ctx,
        cap,
    )
    .await?;
    let media_type = content_type.unwrap_or_else(|| "video/mp4".to_owned());
    let delivered = deliver_media(
        bytes,
        &media_type,
        "openrouter-video",
        Some(store),
        ctx,
        usize::MAX,
    )
    .await?;
    Ok(delivered.value)
}
```

(`deliver_media`'s `max_result_bytes` argument only bounds the no-store inline path, which this handler never takes — pass `usize::MAX` and note it, or the crate's ceiling if `deliver_media` asserts otherwise; read its body and match.)

- [ ] **Step 2: ToolSpec and dispatch in `lib.rs`**

New spec next to the video specs — title `"OpenRouter download video"`, description `"Download one completed OpenRouter video job's content into the configured artifact store and return the artifact reference. Requires openrouter_get_video to report status completed first."`; input schema:

```rust
br#"{"additionalProperties":false,"properties":{"id":{"description":"Job id returned by openrouter_generate_video.","minLength":1,"type":"string"}},"required":["id"],"type":"object"}"#
```

output schema:

```rust
br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference holding the video bytes.","type":"object"},"byte_length":{"type":"integer"},"media_type":{"type":"string"}},"required":["artifact","media_type","byte_length"],"type":"object"}"#
```

metadata: `Sequential`, `NonIdempotentWrite`, `AtMostOnce`, the crate's existing `paid_approval` clone, `max_result_bytes` same as the other specs, `deferral: Never`. Register the `ToolId`, add the dispatch branch mirroring the others, and add the tool to the `tools` array.

- [ ] **Step 3: Tests**

1. `download_video_stages_the_content` — store-backed toolset (`InProcessArtifactStore`); scripted 200 with `Content-Type: video/mp4` and a 1 KiB body; assert the request line contains `get /api/v1/videos/vid-1/content`, the result has `media_type == "video/mp4"`, `byte_length == 1024`, and an `artifact` object; then `store.get` the returned ref (deserialize it back to `ArtifactRef`) and assert the bytes round-trip.
2. `download_video_without_store_fails_closed` — no store → `openrouter_media_store_required`; nothing reaches the fixture.
3. `download_video_bounds_the_body_by_store_limits` — store with `limits().max_artifact_bytes` small (if `InProcessArtifactStore` limits are not configurable, wrap it in a 20-line test newtype forwarding everything but `limits()`); scripted body above the cap → `openrouter_media_limit_exceeded`.
4. `download_video_encodes_the_job_id_path` — `{"id":"../etc"}`; assert the captured request line contains the percent-encoded segment (`%2F`) and not a raw `../`.

- [ ] **Step 4: Run and commit**

Run: `cargo test -p finstack-ai-tools-openrouter-media`
Expected: PASS.

```bash
git add extensions/toolsets/finstack-ai-tools-openrouter-media
git commit -m "Download completed OpenRouter videos into the artifact store"
```

---

## Phase B — `finstack-ai-tools-video-compose`

### Task 3: Crate scaffolding, composition-spec types, validation

**Files:**
- Modify: `Cargo.toml` (workspace root — members + `finstack-ai-tools-video-compose = { path = "extensions/toolsets/finstack-ai-tools-video-compose", version = "1.0.0" }` in workspace deps)
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/Cargo.toml`
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/README.md`
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/src/lib.rs`
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/src/spec.rs`

**Interfaces:**
- Consumes: `finstack_ai_kernel::{ArtifactRef, Digest, ...}`, `finstack_ai_runtime::artifact::ArtifactStore`, the namespaced port imports from Global Constraints.
- Produces (spec.rs, `pub(crate)` unless noted): `CompositionSpec { version: u32, clips: Vec<ClipSpec>, transitions: Option<Vec<TransitionSpec>>, audio: Option<AudioSpec>, subtitles: Option<SubtitlesSpec>, output: OutputSpec }`, `ClipSpec { artifact: ArtifactRef, trim: Option<TrimSpec> }`, `TrimSpec { start_s: f64, end_s: f64 }`, `TransitionKind { Cut, Crossfade, FadeToBlack }`, `TransitionSpec { kind: TransitionKind (field name "type"), duration_s: Option<f64> }`, `AudioSpec { artifact: ArtifactRef, mode: AudioMode, gain_db: Option<f64> }`, `AudioMode { Replace, Mix }`, `SubtitlesSpec { artifact: ArtifactRef, mode: SubtitleMode, style: Option<SubtitleStyle> }`, `SubtitleMode { BurnIn, Mux }`, `SubtitleStyle { font_size: Option<u32>, margin_v: Option<u32> }`, `OutputSpec { container: Container, resolution: Option<String>, fps: Option<u32> }`, `Container { Mp4, Webm }`, and `fn validate_spec(&CompositionSpec) -> Result<(), &'static str>`. Serde: `deny_unknown_fields` everywhere, enums `#[serde(rename_all = "snake_case")]`.
- Produces (lib.rs): the seven `VIDEO_COMPOSE_*` code constants, `pub struct VideoComposeConfig { pub ffmpeg_path: PathBuf, pub ffprobe_path: PathBuf, pub artifact_store: Arc<dyn ArtifactStore>, pub scratch_dir: PathBuf, pub render_timeout: Duration }`, `pub enum VideoComposeError { ConfigInvalid { reason: &'static str } }`, `pub struct VideoComposeToolset` with `try_new(VideoComposeConfig) -> Result<Self, VideoComposeError>` publishing two `ToolSpec`s. `Toolset::call` arrives in Task 5 (skeleton returns `VIDEO_COMPOSE_INVALID_ARGUMENTS` until then).

- [ ] **Step 1: Scaffolding**

Manifest: dependency set mirroring the E2B manifest minus `reqwest` (no HTTP), with `finstack-ai-kernel = { workspace = true }` added and `tokio` carrying `features = ["process", "io-util", "time"]`; dev-dependencies `tempfile = { workspace = true }` and `tokio` with `["macros", "rt"]` added. Description `"Declarative ffmpeg composition toolset for finstack-ai"`. README: declarative spec → host-supplied ffmpeg; agents can never pass flags or paths; requires an injected `ArtifactStore` (ceilings bound render sizes); v1 transitions cut/crossfade/fade_to_black; subtitles burn-in (SRT) or mp4 `mov_text` mux. Lint header verbatim. Register in the workspace root next to the other toolsets.

- [ ] **Step 2: Failing spec tests**

In `src/spec.rs` (helper `artifact_ref()` builds a small valid `ArtifactRef` by hand — copy the construction shape from the runtime's `services/artifact.rs` test module's `artifact()` helper, which shows the current `BlobRef::try_new`/`ArtifactRef::try_new` signatures):

```rust
    fn minimal(clips: usize, transitions: Option<Vec<TransitionSpec>>) -> CompositionSpec {
        CompositionSpec {
            version: 1,
            clips: (0..clips)
                .map(|_| ClipSpec { artifact: artifact_ref(), trim: None })
                .collect(),
            transitions,
            audio: None,
            subtitles: None,
            output: OutputSpec { container: Container::Mp4, resolution: None, fps: None },
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
            Some(vec![TransitionSpec { kind: TransitionKind::Cut, duration_s: None }]),
        );
        assert!(validate_spec(&mismatched).is_err(), "transitions must be clips-1");
        let bad_duration = minimal(
            2,
            Some(vec![TransitionSpec {
                kind: TransitionKind::Crossfade,
                duration_s: Some(30.0),
            }]),
        );
        assert!(validate_spec(&bad_duration).is_err(), "transition <= 5s");
        let mut bad_trim = minimal(1, None);
        bad_trim.clips[0].trim = Some(TrimSpec { start_s: 5.0, end_s: 2.0 });
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
            style: Some(SubtitleStyle { font_size: Some(4), margin_v: None }),
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
        assert!(validate_spec(&bad_res).is_err(), "resolution is WIDTHxHEIGHT");
        let mut bad_fps = minimal(1, None);
        bad_fps.output.fps = Some(500);
        assert!(validate_spec(&bad_fps).is_err(), "fps in [1, 120]");
    }
```

- [ ] **Step 3: Run to verify failure, then implement `spec.rs`**

Run: `cargo test -p finstack-ai-tools-video-compose spec` — FAIL to compile. Implement the types and:

```rust
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
        if let Some(trim) = &clip.trim {
            if !trim.start_s.is_finite()
                || !trim.end_s.is_finite()
                || trim.start_s < 0.0
                || trim.end_s <= trim.start_s
            {
                return Err("clip trim must satisfy 0 <= start < end");
            }
        }
    }
    if let Some(audio) = &spec.audio {
        if let Some(gain) = audio.gain_db {
            if !gain.is_finite() || !(-60.0..=12.0).contains(&gain) {
                return Err("audio gain must be between -60 and 12 dB");
            }
        }
    }
    if let Some(subtitles) = &spec.subtitles {
        if matches!(subtitles.mode, SubtitleMode::Mux)
            && !matches!(spec.output.container, Container::Mp4)
        {
            return Err("muxed subtitles require the mp4 container");
        }
        if let Some(style) = &subtitles.style {
            if style.font_size.is_some_and(|v| !(8..=96).contains(&v))
                || style.margin_v.is_some_and(|v| v > 400)
            {
                return Err("subtitle style values are out of bounds");
            }
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
    if let Some(fps) = spec.output.fps {
        if !(1..=120).contains(&fps) {
            return Err("output fps must be between 1 and 120");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: `lib.rs` config, codes, tool catalog**

Module doc `//! T1 declarative ffmpeg composition Toolset. Agents submit a bounded spec; the toolset owns every ffmpeg argument.` The seven code constants; `VideoComposeConfig` (derive nothing; hand-write `Debug` listing paths and timeout — nothing secret, but `artifact_store` renders as presence only); `try_new` validates absolute binary paths, `render_timeout` in `(0, 1 hour]`, creates `scratch_dir` (`create_dir_all`), and builds the specs:

- `compose_video`: description `"Merge stored clips into one movie with declarative transitions, audio, and subtitles."`; `Sequential`, `NonIdempotentWrite`, `AtMostOnce`, approval `ApprovalMetadata { requirement: ApprovalRequirement::Policy, reason: Some(Arc::from("local media rendering")), attributes: Metadata::empty() }`, `max_result_bytes: 262_144`, `deferral: Never`. Input schema mirrors the serde types (plain-optional convention: required `["version","clips","output"]`; clips items require `artifact`; transition items require `type` with `enum ["cut","crossfade","fade_to_black"]`; audio requires `artifact` and `mode` (`enum ["replace","mix"]`); subtitles requires `artifact` and `mode` (`enum ["burn_in","mux"]`); output requires `container` (`enum ["mp4","webm"]`); `additionalProperties: false` at every level; `artifact` properties are `{"type":"object"}`). Output schema:

```rust
br#"{"additionalProperties":false,"properties":{"artifact":{"type":"object"},"byte_length":{"type":"integer"},"duration_s":{"type":"number"}},"required":["artifact","duration_s","byte_length"],"type":"object"}"#
```

- `probe_media`: description `"Probe duration, dimensions, and streams of one stored media artifact."`; `Sequential`, `SideEffectClass::ReadOnly`, `RetrySafety::AtMostOnce`, approval `NotRequired` (reason `None`), `max_result_bytes: 65_536`. Input `{"additionalProperties":false,"properties":{"artifact":{"type":"object"}},"required":["artifact"],"type":"object"}`; output:

```rust
br#"{"additionalProperties":false,"properties":{"duration_s":{"type":"number"},"fps":{"type":"number"},"has_audio":{"type":"boolean"},"height":{"type":"integer"},"media_type":{"type":"string"},"width":{"type":"integer"}},"required":["duration_s","has_audio"],"type":"object"}"#
```

- [ ] **Step 5: Run and commit**

Run: `cargo test -p finstack-ai-tools-video-compose`
Expected: PASS.

```bash
git add Cargo.toml Cargo.lock extensions/toolsets/finstack-ai-tools-video-compose
git commit -m "Scaffold the video compose toolset with a validated composition spec"
```

---

### Task 4: Pure ffmpeg argument builder

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/src/graph.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-video-compose/src/lib.rs` (add `mod graph;`)

**Interfaces:**
- Consumes: Task 3's spec types.
- Produces: `pub(crate) struct ClipInput { pub path: PathBuf, pub duration_s: f64 }` and `pub(crate) fn build_ffmpeg_args(spec: &CompositionSpec, clips: &[ClipInput], audio: Option<&Path>, subtitles: Option<&Path>, output: &Path) -> Result<Vec<std::ffi::OsString>, &'static str>`. Pure — no I/O.

- [ ] **Step 1: Failing tests**

(`minimal`/`artifact_ref` are local copies of the Task 3 test helpers; `clip(path, dur)` builds a `ClipInput`; `rendered(args)` joins lossily to `Vec<String>`.)

```rust
    #[test]
    fn cut_only_composition_uses_concat() {
        let spec = minimal(2, None);
        let clips = [clip("/in/a.mp4", 4.0), clip("/in/b.mp4", 6.0)];
        let args = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/out/movie.mp4"))
            .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.starts_with("-nostdin -y"));
        assert!(text.contains("-i /in/a.mp4"));
        assert!(text.contains("-i /in/b.mp4"));
        assert!(text.contains("concat=n=2:v=1:a=1"));
        assert!(text.ends_with("/out/movie.mp4"));
        assert!(!text.contains("xfade"));
    }

    #[test]
    fn crossfade_composition_chains_xfade_with_running_offsets() {
        let spec = minimal(
            3,
            Some(vec![
                TransitionSpec { kind: TransitionKind::Crossfade, duration_s: Some(1.0) },
                TransitionSpec { kind: TransitionKind::Crossfade, duration_s: Some(0.5) },
            ]),
        );
        let clips = [
            clip("/in/a.mp4", 4.0),
            clip("/in/b.mp4", 6.0),
            clip("/in/c.mp4", 5.0),
        ];
        let args = build_ffmpeg_args(&spec, &clips, None, None, Path::new("/out/movie.mp4"))
            .expect("args");
        let text = rendered(&args).join(" ");
        // first xfade offset = 4.0 - 1.0; second = (4.0 + 6.0 - 1.0) - 0.5
        assert!(text.contains("xfade=transition=fade:duration=1:offset=3"));
        assert!(text.contains("xfade=transition=fade:duration=0.5:offset=8.5"));
        assert!(text.contains("acrossfade=d=1"));
    }

    #[test]
    fn trims_scale_fps_and_audio_are_encoded() {
        let mut spec = minimal(1, None);
        spec.clips[0].trim = Some(TrimSpec { start_s: 1.0, end_s: 3.5 });
        spec.output.resolution = Some("1280x720".into());
        spec.output.fps = Some(24);
        spec.audio = Some(AudioSpec {
            artifact: artifact_ref(),
            mode: AudioMode::Replace,
            gain_db: Some(-6.0),
        });
        let clips = [clip("/in/a.mp4", 10.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            Some(Path::new("/in/music.mp3")),
            None,
            Path::new("/out/movie.mp4"),
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("trim=start=1:end=3.5"));
        assert!(text.contains("scale=1280:720"));
        assert!(text.contains("fps=24"));
        assert!(text.contains("-i /in/music.mp3"));
        assert!(text.contains("volume=-6dB"));
        assert!(text.contains("-shortest"));
    }

    #[test]
    fn burned_in_subtitles_apply_the_clamped_style() {
        let mut spec = minimal(1, None);
        spec.subtitles = Some(SubtitlesSpec {
            artifact: artifact_ref(),
            mode: SubtitleMode::BurnIn,
            style: Some(SubtitleStyle { font_size: Some(42), margin_v: Some(80) }),
        });
        let clips = [clip("/in/a.mp4", 4.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            Some(Path::new("/scratch/subs.srt")),
            Path::new("/out/movie.mp4"),
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("subtitles=/scratch/subs.srt:force_style='FontSize=42,MarginV=80'"));
    }

    #[test]
    fn muxed_subtitles_add_an_input_and_the_mov_text_codec() {
        let mut spec = minimal(1, None);
        spec.subtitles = Some(SubtitlesSpec {
            artifact: artifact_ref(),
            mode: SubtitleMode::Mux,
            style: None,
        });
        let clips = [clip("/in/a.mp4", 4.0)];
        let args = build_ffmpeg_args(
            &spec,
            &clips,
            None,
            Some(Path::new("/scratch/subs.srt")),
            Path::new("/out/movie.mp4"),
        )
        .expect("args");
        let text = rendered(&args).join(" ");
        assert!(text.contains("-i /scratch/subs.srt"));
        assert!(text.contains("-c:s mov_text"));
        assert!(!text.contains("subtitles="));
    }

    #[test]
    fn clip_count_and_duration_mismatch_is_rejected() {
        let spec = minimal(2, None);
        assert!(
            build_ffmpeg_args(&spec, &[clip("/in/a.mp4", 4.0)], None, None, Path::new("/o.mp4"))
                .is_err()
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-tools-video-compose graph` — FAIL.

- [ ] **Step 3: Implement**

Build order (all args as `OsString`; float rendering via `fn fmt_f64(v: f64) -> String` that renders `3.0` as `3` — Rust's `format!("{v}")` on `3.0_f64` yields `"3"`, verify with a unit assertion inside the test module):

1. Global flags: `-nostdin -y -hide_banner -loglevel error`.
2. One `-i <clip.path>` per clip; `-i <audio>` when present (input index `clips.len()`); for `Mux` subtitles, `-i <subtitles>` as the last input.
3. One `-filter_complex` string:
   - Per clip `i`: `[i:v]` + `trim=start=..:end=..,setpts=PTS-STARTPTS,` (when trimmed) + `scale=W:H,` (when resolution set) + `fps=N,` (when set) + `setsar=1[v{i}]`; audio lane `[i:a]` + `atrim=...,asetpts=PTS-STARTPTS,` (when trimmed) + `anull[a{i}]`.
   - Effective per-clip duration = `trim.end_s - trim.start_s` when trimmed else `ClipInput.duration_s`.
   - No transitions or all `cut`: `[v0][a0]...concat=n=N:v=1:a=1[vc][ac]`.
   - Any non-cut transition: pairwise fold with running `offset = duration(0)`; join `k` uses `xfade=transition={fade|fadeblack}:duration=D:offset={offset - D}` + `acrossfade=d=D` (a `cut` mixed in folds as `xfade` with `duration=0.01` — documented v1 simplification); after each join `offset += duration(k+1) - D`; final labels `[vc]`/`[ac]`.
   - `BurnIn` subtitles (path required when `spec.subtitles` is `Some`, else `Err("subtitles path is required by the spec")`): append `[vc]subtitles={path}:force_style='FontSize={fs},MarginV={mv}'[vs]` (style clause only when set; escape `'` and `:` in the path with backslashes) and map `[vs]`.
   - Audio `replace`: `[N:a]volume={G}dB[music]` (volume only when set), map video + `[music]`, add `-shortest`. `mix`: `[ac][music]amix=inputs=2:duration=first[am]`, map `[am]`. Neither: map `[ac]`.
4. Output flags: mp4 → `-c:v libx264 -pix_fmt yuv420p -c:a aac -movflags +faststart` (+ `-c:s mov_text` for `Mux`); webm → `-c:v libvpx-vp9 -c:a libopus`. Then the output path.

Return `Err("clip inputs must match the spec")` when `clips.len() != spec.clips.len()` or any duration is non-finite/non-positive.

- [ ] **Step 4: Run and commit**

Run: `cargo test -p finstack-ai-tools-video-compose`
Expected: PASS.

```bash
git add extensions/toolsets/finstack-ai-tools-video-compose
git commit -m "Build ffmpeg filtergraph arguments from validated composition specs"
```

---

### Task 5: Process execution, artifact I/O, `Toolset::call`

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-video-compose/src/exec.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-video-compose/src/lib.rs` (real `Toolset::call`, `mod exec;`)

**Interfaces:**
- Consumes: Tasks 3–4; `finstack_ai_runtime::artifact::{stage_required_artifact, validate_retrieved_artifact, InProcessArtifactStore (dev)}`.
- Produces (exec.rs, `pub(crate)`): `async fn run_bounded(binary: &Path, args: &[OsString], timeout: Duration, cancellation: &CancellationSignal) -> Result<std::process::Output, ToolError>`; `struct ProbeResult { duration_s: f64, width: Option<u32>, height: Option<u32>, fps: Option<f64>, has_audio: bool }`; `async fn probe(ffprobe: &Path, target: &Path, timeout: Duration, cancellation: &CancellationSignal) -> Result<ProbeResult, ToolError>`.
- Produces (lib.rs, `pub(crate)`, shared with nothing outside this crate): `fn artifact_scope(ctx: &ToolCallContext) -> ArtifactScope` (Global Constraints shape), `async fn fetch_artifact_to_file(store: &Arc<dyn ArtifactStore>, scope: &ArtifactScope, artifact: &ArtifactRef, dir: &Path, stem: &str) -> Result<PathBuf, ToolError>` (store `get` → `validate_retrieved_artifact` → write `dir/{stem}-{first 16 digest hex}` — map store errors to `VIDEO_COMPOSE_MEDIA_FAILURE`), `async fn stage_output(store, scope, path: &Path, media_type: &str) -> Result<(ArtifactRef, u64), ToolError>` (read file → `stage_required_artifact` with kind `"tool-output"`; `ArtifactError::TooLarge` → `VIDEO_COMPOSE_LIMIT_EXCEEDED`, else `VIDEO_COMPOSE_MEDIA_FAILURE`).
- Tool results: `compose_video` → `{"artifact","duration_s","byte_length"}`; `probe_media` → `{"duration_s","width"?,"height"?,"fps"?,"has_audio","media_type"}`.

- [ ] **Step 1: Implement `exec.rs`**

`run_bounded`: `tokio::process::Command::new(binary).args(args).kill_on_drop(true).stdout(piped).stderr(piped)`, then `tokio::select!` over `child.wait_with_output()`, `cancellation.cancelled()`, and `tokio::time::sleep(timeout)`; cancel/timeout → `VIDEO_COMPOSE_TIMEOUT`; spawn failure → `VIDEO_COMPOSE_CONFIG_INVALID` ("compose binary failed to start"); non-zero exit → `VIDEO_COMPOSE_FFMPEG_FAILED` whose message carries the last 2 KiB of stderr lossily UTF-8 (bounded — build the `ToolError` with `ToolError::try_new(code, category, false, message, Metadata::empty())` where message is a `String` truncated to the tail; if `try_new` requires `&'static str` messages, put the tail into the error's `Metadata` under key `"stderr_tail"` instead and keep a static message — check `ToolError::try_new`'s current signature and pick accordingly).

`probe` runs `ffprobe -v error -print_format json -show_format -show_streams <target>` through `run_bounded` and parses:

```rust
#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    format: Option<FfprobeFormat>,
    #[serde(default)]
    streams: Vec<FfprobeStream>,
}
#[derive(Deserialize)]
struct FfprobeFormat {
    #[serde(default)]
    duration: Option<String>,
}
#[derive(Deserialize)]
struct FfprobeStream {
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
}
```

`duration_s` parses `format.duration` (missing/unparseable → `VIDEO_COMPOSE_MEDIA_FAILURE`); `fps` parses `"num/den"` (den 0 → `None`); `has_audio` = any `codec_type == "audio"` stream.

- [ ] **Step 2: Implement `Toolset::call`**

Both arms: `verify_authority(&ctx)?`, identity check (E2B shape). `probe_media`: parse `{artifact: ArtifactRef}` (`deny_unknown_fields`); `fetch_artifact_to_file`; `probe`; result plus `"media_type": artifact.blob().media_type()` (same accessor caveat as Task 1); best-effort remove the scratch file. `compose_video`:

1. Parse `CompositionSpec`; `validate_spec` failure → `VIDEO_COMPOSE_SPEC_INVALID` (reason as the message).
2. Fetch every clip, the audio artifact, and the subtitles artifact to scratch files (`clip-{i}`, `audio`, `subs` stems; the subtitles file gets an `.srt` suffix so ffmpeg's subtitle demuxer engages).
3. `probe` each clip → `ClipInput { path, duration_s }`.
4. Output path `scratch_dir.join(format!("render-{:x?}.{ext}", ctx.run.effect_id))` — derive uniqueness from the effect id (render its `Debug`/hex form; never wall time), ext by container.
5. `build_ffmpeg_args` with the subtitle path (error → `VIDEO_COMPOSE_SPEC_INVALID`); `run_bounded(ffmpeg, ...)`.
6. `probe` the output for real duration; `stage_output` (media type `video/mp4`/`video/webm`); best-effort remove scratch files; result `{"artifact": <serde_json::to_value(&artifact)>, "duration_s", "byte_length"}`.

- [ ] **Step 3: Tests (unix-only stub binaries)**

Stub helper (`#[cfg(unix)]`, mode `0o755`):

```rust
    fn write_stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("stub");
        let mut permissions = std::fs::metadata(&path).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod");
        path
    }
```

ffprobe stub echoes `{"format":{"duration":"4.0"},"streams":[{"codec_type":"video","width":640,"height":360,"avg_frame_rate":"24/1"},{"codec_type":"audio"}]}`. ffmpeg stub writes its argv next to itself and creates a one-byte output file: `d=$(dirname "$0"); printf '%s\n' "$@" > "$d/ffmpeg-args.txt"; eval last=\"\${$#}\"; printf 'x' > "$last"`. Store = `InProcessArtifactStore`; `tool_context()` copied from the E2B test module (scope: `tenant-a`, session `[1;16]`, run `[3;16]`, `Sensitivity::Internal` when staging fixtures).

1. `compose_renders_through_the_stub_and_stores_the_output` — stage two tiny clips; spec with one crossfade; assert `ffmpeg-args.txt` contains `xfade=transition=fade` and both scratch clip paths; result `artifact` round-trips through the store (one byte).
2. `burned_in_subtitles_reach_ffmpeg` — stage a small SRT artifact; `subtitles: {artifact, mode: burn_in}`; assert `ffmpeg-args.txt` contains `subtitles=` with a path ending `.srt`.
3. `ffmpeg_failure_surfaces_bounded_stderr` — stub `echo "boom: filter parse error" >&2; exit 1` → `video_compose_ffmpeg_failed`; the error carries `filter parse error` (message or metadata) bounded ≤ 2 KiB.
4. `render_timeout_kills_the_child` — stub `sleep 30`, `render_timeout` 200 ms → `video_compose_timeout` promptly.
5. `probe_media_maps_ffprobe_json` — `duration_s == 4.0`, `width == 640`, `fps == 24.0`, `has_audio`.
6. `invalid_spec_is_rejected_before_any_process_runs` — 3 clips + 1 transition; ffmpeg stub writes a marker; assert `video_compose_spec_invalid` and no marker.
7. `oversized_render_output_fails_closed` — wrap the store in a test newtype whose `limits().max_artifact_bytes` is 0-ish small if `InProcessArtifactStore` is not configurable; ffmpeg stub writes a larger output → `video_compose_limit_exceeded`.

- [ ] **Step 4: Run and commit**

Run: `cargo test -p finstack-ai-tools-video-compose`
Expected: PASS.

```bash
git add extensions/toolsets/finstack-ai-tools-video-compose
git commit -m "Execute bounded ffmpeg renders and probes behind the compose tools"
```

---

## Phase C — MoviePlan and the pipeline driver

### Task 6: MoviePlan schema, Rust types, fixtures

**Files:**
- Create: `schemas/movie-plan/movie-plan.v1.json`
- Modify: `schemas/schema-families.toml` (add the `movie-plan` family)
- Modify: `Cargo.toml` (workspace root — members + `finstack-ai-workflow-media-pipeline = { path = "extensions/workflow/finstack-ai-workflow-media-pipeline", version = "1.0.0" }`)
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/Cargo.toml`
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/README.md`
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs`
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/plan.rs`
- Create: `fixtures/compatibility/movie-plan/valid-two-scene.json`
- Create: `fixtures/compatibility/movie-plan/invalid-missing-prompt.json`
- Create: `fixtures/compatibility/movie-plan/invalid-ambiguous-frame.json`

**Interfaces:**
- Produces (plan.rs, all `pub`): `MoviePlan { version: u32, title: Option<String>, defaults: PlanDefaults, scenes: Vec<SceneSpec>, transitions: Option<Vec<PlanTransition>>, audio: Option<PlanAudio>, output: PlanOutput }`, `PlanDefaults { image_model: String, video_model: String, resolution: Option<String>, aspect_ratio: Option<String>, scene_duration_s: u32 }`, `SceneSpec { id: String, video_prompt: String, start_frame: FrameSource, end_frame: Option<FrameSource>, reference_images: Option<Vec<FrameSource>>, duration_s: Option<u32>, seed: Option<i64>, captions: Option<Vec<CaptionCue>>, overrides: Option<SceneOverrides> }`, `CaptionCue { text: String, start_s: f64, end_s: f64 }`, `SceneOverrides { image_model: Option<String>, video_model: Option<String>, resolution: Option<String> }`, `FrameSource` — untagged enum of `Prompt { prompt: String }`, `Artifact { artifact: ArtifactRef }`, `Url { url: String }` (each inner struct `deny_unknown_fields`), `PlanTransition { after: String, kind: TransitionKindName (field name "type"), duration_s: Option<f64> }` (`TransitionKindName { Cut, Crossfade, FadeToBlack }`, snake_case), `PlanAudio { artifact: ArtifactRef, mode: String }`, `PlanOutput { container: String, fps: Option<u32>, captions: Option<CaptionsMode> }`, `CaptionsMode { None, Sidecar, BurnIn }` (snake_case).
- Produces: `PlanLimits { max_scenes: usize, max_total_video_s: u64, max_concurrent_jobs: usize }`, `PlanBudget { scene_count: usize, total_video_s: u64 }`, `pub fn validate_plan(plan: &MoviePlan, limits: &PlanLimits) -> Result<PlanBudget, &'static str>`.

- [ ] **Step 1: Schema family registration and JSON schema**

Append to `schemas/schema-families.toml` (matching the existing entry format):

```toml
[families."movie-plan"]
owner = "me@jeickmeier.com"
reviewer = "me@jeickmeier.com"
source_root = "schemas/movie-plan/"
format = "json-schema-2020-12"
stability = "candidate-v1"
compatibility_profile = "strict-reject-unknown"
fixture_root = "fixtures/compatibility/movie-plan/"
status = "active"
```

Write `schemas/movie-plan/movie-plan.v1.json` as JSON Schema 2020-12 mirroring the Rust types: `$id: "https://finstack.ai/schemas/movie-plan/v1"`, top-level required `["version","defaults","scenes","output"]`, `version` `const: 1`, `scenes` `minItems: 1`, `additionalProperties: false` at every object level; `$defs.frame_source` = `oneOf` of the three single-required-key objects (`{"prompt"}`, `{"artifact"}`, `{"url"}`); `artifact` values `{"type":"object"}`; scene `id` `pattern: "^[a-z0-9-]{1,64}$"`; scene `captions` array `maxItems: 32` of required `{text, start_s, end_s}` with `text` `maxLength: 200`; transition `type` enum `["cut","crossfade","fade_to_black"]`; audio `mode` enum `["replace","mix"]`; output `container` enum `["mp4","webm"]`, `captions` enum `["none","sidecar","burn_in"]`.

- [ ] **Step 2: Crate scaffolding**

```toml
[package]
name = "finstack-ai-workflow-media-pipeline"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "MoviePlan media pipeline driver and tools for finstack-ai"
readme = "README.md"

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
futures-util = { workspace = true }
rusqlite = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tempfile = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
finstack-ai-test = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt"] }

[lints]
workspace = true
```

README: MoviePlan pipeline — tick-based, resumable, adapter-owned sqlite state in the workflow-local mold; tools `render_movie`/`advance_render`/`get_render_status`; hosts bound spend via `PlanLimits`; media as `ArtifactRef`s through the host's artifact store; T1 native, not isolated. `lib.rs`: lint header, module doc `//! MoviePlan pipeline driver: adapter-journaled, tick-based, resumable.`, `mod plan;` + re-exports.

- [ ] **Step 3: Failing plan tests**

(Helper `two_scene_plan()` mirrors the valid fixture in Rust; `limits()` = `{ max_scenes: 10, max_total_video_s: 120, max_concurrent_jobs: 2 }`.)

```rust
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
        for name in ["invalid-missing-prompt.json", "invalid-ambiguous-frame.json"] {
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
        let tight = PlanLimits { max_scenes: 1, ..limits() };
        assert!(validate_plan(&plan, &tight).is_err(), "scene ceiling");
        let tight = PlanLimits { max_total_video_s: 5, ..limits() };
        assert!(validate_plan(&plan, &tight).is_err(), "seconds ceiling");

        let mut plan = two_scene_plan();
        plan.scenes[0].captions = Some(vec![CaptionCue {
            text: "x".repeat(300),
            start_s: 0.0,
            end_s: 2.0,
        }]);
        assert!(validate_plan(&plan, &limits()).is_err(), "caption text bound");

        let mut plan = two_scene_plan();
        plan.scenes[0].captions = Some(vec![
            CaptionCue { text: "one".into(), start_s: 0.0, end_s: 4.0 },
            CaptionCue { text: "two".into(), start_s: 3.0, end_s: 6.0 },
        ]);
        assert!(validate_plan(&plan, &limits()).is_err(), "overlapping cues");
    }
```

`valid-two-scene.json` (a literal `ArtifactRef` is awkward in a fixture, so the valid fixture pins scene-02's start frame with a URL, not an artifact):

```json
{
  "version": 1,
  "title": "Two scene test",
  "defaults": {
    "image_model": "test/image-model",
    "video_model": "test/video-model",
    "resolution": "720p",
    "scene_duration_s": 6
  },
  "scenes": [
    {
      "id": "scene-01",
      "video_prompt": "camera glides across a harbor at dawn",
      "start_frame": { "prompt": "wide shot of a harbor at dawn, golden light" },
      "end_frame": { "prompt": "close-up of a moored fishing boat" },
      "duration_s": 8,
      "seed": 42,
      "captions": [
        { "text": "Harbors wake up slowly.", "start_s": 0.0, "end_s": 3.5 },
        { "text": "Then all at once.", "start_s": 3.5, "end_s": 7.5 }
      ]
    },
    {
      "id": "scene-02",
      "video_prompt": "gulls lift off the pier",
      "start_frame": { "url": "https://example.test/pier.png" }
    }
  ],
  "transitions": [
    { "after": "scene-01", "type": "crossfade", "duration_s": 0.5 }
  ],
  "output": { "container": "mp4", "fps": 24, "captions": "burn_in" }
}
```

`invalid-missing-prompt.json`: scene-01's `start_frame` is `{}`. `invalid-ambiguous-frame.json`: scene-01's `start_frame` is `{"prompt": "x", "url": "https://example.test/x.png"}`. **Check** serde's `untagged` + `deny_unknown_fields` interaction: if the two-key object accidentally parses, replace `FrameSource`'s derive with a manual `Deserialize` that inspects the key set and accepts exactly one known key — the fixture test is the oracle.

- [ ] **Step 4: Implement `plan.rs`**

Types as specified, plus:

```rust
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
    Ok(PlanBudget { scene_count: plan.scenes.len(), total_video_s })
}
```

- [ ] **Step 5: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline`
Expected: PASS.

```bash
git add Cargo.toml Cargo.lock schemas extensions/workflow/finstack-ai-workflow-media-pipeline fixtures/compatibility/movie-plan
git commit -m "Add the MoviePlan v1 schema, fixtures, and validated plan types"
```

---

### Task 7: Render state and its stores

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/state.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs` (`mod state;` + re-exports)

**Interfaces:**
- Produces (all serde `deny_unknown_fields`, enums snake_case):

```rust
pub enum SceneStage { PendingStartFrame, PendingEndFrame, PendingSubmit, Polling, PendingDownload, Done, Failed }
pub struct SceneState {
    pub scene_id: String,
    pub stage: SceneStage,
    pub start_frame_artifact: Option<ArtifactRef>,
    pub start_frame_url: Option<String>,
    pub end_frame_artifact: Option<ArtifactRef>,
    pub end_frame_url: Option<String>,
    pub job_id: Option<String>,
    pub clip_artifact: Option<ArtifactRef>,
    pub failure: Option<String>,
    pub resubmitted: bool,
}
pub enum RenderStatus { Running, Composing, Completed, Failed }
pub struct RenderState {
    pub tenant_scope: Arc<str>,
    pub render_id: Arc<str>,           // "render-" + first 24 hex of the plan digest (deterministic; resubmission = resume)
    pub plan_json: String,
    pub plan_digest: Digest,
    pub status: RenderStatus,
    pub scenes: Vec<SceneState>,
    pub final_artifact: Option<ArtifactRef>,
    pub transcript_srt_artifact: Option<ArtifactRef>,
    pub transcript_vtt_artifact: Option<ArtifactRef>,
    pub revision: u64,
}
pub trait RenderStateStore: Send + Sync {
    fn insert(&self, state: &RenderState) -> Result<(), StateError>;
    fn load(&self, tenant_scope: &str, render_id: &str) -> Result<Option<RenderState>, StateError>;
    fn update(&self, state: &RenderState) -> Result<bool, StateError>;   // CAS on revision; persists revision+1; false = lost race
}
pub struct MemoryRenderStateStore;   // BTreeMap under Mutex
pub struct SqliteRenderStateStore;   // fn open(path: impl AsRef<Path>) -> Result<Self, StateError>
pub enum StateError { Unavailable { code: &'static str }, Integrity { code: &'static str } }
```

- [ ] **Step 1: Failing tests**

```rust
    #[test]
    fn sqlite_store_round_trips_and_cas_guards_updates() {
        let dir = tempfile::tempdir().expect("dir");
        let store = SqliteRenderStateStore::open(dir.path().join("journal.sqlite")).expect("open");
        let mut state = sample_state(); // helper: tenant-a, one Running scene, revision 0
        store.insert(&state).expect("insert");
        assert!(store.insert(&state).is_err(), "duplicate render_id");
        assert_eq!(
            store.load("tenant-a", state.render_id.as_ref()).expect("load").expect("row").revision,
            0
        );
        state.status = RenderStatus::Composing;
        assert!(store.update(&state).expect("update"), "first CAS wins");
        assert!(!store.update(&state).expect("update"), "stale revision loses");
        let reloaded = store
            .load("tenant-a", state.render_id.as_ref())
            .expect("load")
            .expect("row");
        assert_eq!(reloaded.revision, 1);
        assert!(matches!(reloaded.status, RenderStatus::Composing));
        assert!(store.load("tenant-b", state.render_id.as_ref()).expect("load").is_none());
    }

    #[test]
    fn memory_store_matches_sqlite_semantics() { /* same assertions on MemoryRenderStateStore */ }

    #[test]
    fn two_sqlite_handles_share_one_table() {
        // open two stores on one path; insert via first, load via second,
        // update via second, assert first sees revision 1.
    }
```

- [ ] **Step 2: Implement**

Mirror `extensions/workflow/finstack-ai-workflow-local/src/store.rs` structurally (`Mutex<Connection>`, `busy_timeout(1s)`, WAL off `:memory:`, `TransactionBehavior::Immediate`). DDL:

```sql
CREATE TABLE IF NOT EXISTS finstack_workflow_media_pipeline (
  tenant_scope TEXT NOT NULL,
  render_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  state_json TEXT NOT NULL,
  PRIMARY KEY (tenant_scope, render_id)
);
```

`insert`: plain `INSERT` (constraint violation → `Integrity { code: "render_exists" }`). `update`: serialize a clone carrying `revision + 1`, then `UPDATE ... SET revision = revision + 1, state_json = ?json WHERE tenant_scope = ? AND render_id = ? AND revision = ?expected` in an immediate transaction; `changes() == 1` is the verdict. `load`: deserialize `state_json` (corrupt → `Integrity { code: "state_json_invalid" }`).

- [ ] **Step 3: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline state`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Persist render state with optimistic concurrency in adapter-owned sqlite"
```

---

### Task 8: Caption timeline and SRT/VTT serialization

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/subtitles.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs` (`mod subtitles;`)

**Interfaces:**
- Produces (`pub(crate)`): `struct TimedCue { text: String, start_s: f64, end_s: f64 }` (absolute movie-timeline seconds), `fn cue_timeline(plan: &MoviePlan) -> Vec<TimedCue>`, `fn to_srt(cues: &[TimedCue]) -> String`, `fn to_vtt(cues: &[TimedCue]) -> String`.

- [ ] **Step 1: Failing tests**

(`two_scene_plan()` is a local copy of the plan.rs test helper.)

```rust
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
            TimedCue { text: "Harbors wake up slowly.".into(), start_s: 0.0, end_s: 3.5 },
            TimedCue { text: "Then all at once.".into(), start_s: 3.5, end_s: 7.5 },
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
        let cues = vec![TimedCue { text: "late".into(), start_s: 3_661.25, end_s: 3_662.0 }];
        assert!(to_srt(&cues).contains("01:01:01,250 --> 01:01:02,000"));
    }
```

- [ ] **Step 2: Implement**

`cue_timeline`: walk scenes in order with `offset = 0.0`; emit each cue at `offset + start_s` / `offset + end_s`; after scene k, `offset += effective_duration(k) - overlap(k)` where `effective_duration` = `duration_s.unwrap_or(defaults.scene_duration_s)` as f64 and `overlap(k)` = the plan transition after scene k's `duration_s.unwrap_or(0.5)` for `crossfade`/`fade_to_black`, `0.0` for `cut`/absent. (Mirrors the compose filtergraph's xfade offsets over *planned* durations; actual clip drift is the documented v1 tolerance.) Timestamps via `fn stamp(seconds: f64, millis_sep: char) -> String` producing `HH:MM:SS{sep}mmm` from `(seconds * 1000.0).round() as u64`.

- [ ] **Step 3: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline subtitles`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Flatten plan-authored caption cues onto the movie timeline as SRT and VTT"
```

---

### Task 9: `MediaPipelineDriver` — submit and tick

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/driver.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs` (`mod driver;` + re-exports)

**Interfaces:**
- Consumes: Tasks 6–8; the toolsets purely as `Arc<dyn Toolset>`; `finstack_ai_runtime::artifact::{ArtifactStore, stage_required_artifact}`.
- Produces:

```rust
pub struct MediaPipelineConfig {
    pub media_tools: Arc<dyn Toolset>,      // finstack-ai-tools-openrouter-media
    pub compose_tools: Arc<dyn Toolset>,    // finstack-ai-tools-video-compose
    pub state: Arc<dyn RenderStateStore>,
    pub artifact_store: Option<Arc<dyn ArtifactStore>>, // required by caption plans (transcripts)
    pub limits: PlanLimits,
}
pub struct MediaPipelineDriver { /* the fields above */ }
impl MediaPipelineDriver {
    pub fn try_new(config: MediaPipelineConfig) -> Result<Self, PipelineError>;
    pub async fn submit_plan(&self, ctx: &ToolCallContext, plan_json: &[u8]) -> Result<RenderState, ToolError>;
    pub async fn advance(&self, ctx: &ToolCallContext, render_id: &str) -> Result<RenderState, ToolError>;
    pub fn status(&self, tenant_scope: &str, render_id: &str) -> Result<RenderState, ToolError>;
}
pub enum PipelineError { ConfigInvalid { reason: &'static str } }
```

plus crate-internal `fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError` (copy from `extensions/toolsets/finstack-ai-sandbox-e2b/src/lib.rs:516` verbatim), `fn stage_error(message: &'static str) -> ToolError` (wraps `MEDIA_PIPELINE_STAGE_FAILED`, `ErrorCategory::Tool`), the seven `MEDIA_PIPELINE_*` constants, and `async fn invoke_tool(toolset: &Arc<dyn Toolset>, tool_name: &str, arguments: serde_json::Value, ctx: &ToolCallContext) -> Result<serde_json::Value, ToolError>`.

- [ ] **Step 1: Implement `invoke_tool`**

Find the spec in `toolset.tools()` by `model_name == tool_name` (unknown → `stage_error`), then build the call exactly as the E2B test module builds `ValidatedToolCall` (`ToolCallBlock::try_new(ctx.tool_call_id, tool_name, RawJson::parse(serde_json::to_vec(&arguments)...)...)`, `tool_id: spec.id.clone()`, `component: None`, `output_contract: EffectOutputContract { kind: EffectOutputKind::ToolResult, schema_version: 1, schema_digest: Digest::raw_json(b"{}") }`, `retry_safety: spec.retry_safety`, `deadline: ctx.run.deadline`, `execution: spec.execution`, `failure_policy: ToolFailurePolicy::ReturnToModel`). `toolset.call(ctx.clone(), call).await?` (`ToolCallContext` is `Clone`); drain the stream to `Completed`; `is_error: true` → `stage_error` carrying the inner code in the message where static (else generic); parse `result.output.as_bytes()` to `serde_json::Value`. This composes the leaf tools in-process: the kernel-journaled effects are the wrapping `render_movie`/`advance_render` calls; per-stage durability is the adapter state store (workflow-local cron precedent).

- [ ] **Step 2: Implement `submit_plan`**

1. Parse `MoviePlan` (failure → `MEDIA_PIPELINE_PLAN_INVALID`). When any scene has cues or `output.captions` is `sidecar`/`burn_in`, require `artifact_store` (absent → `MEDIA_PIPELINE_PLAN_INVALID`, "captions require an artifact store").
2. `validate_plan` (the two ceiling messages → `MEDIA_PIPELINE_BUDGET_EXCEEDED`, others → `MEDIA_PIPELINE_PLAN_INVALID` — match on the message strings).
3. `plan_digest = Digest::raw_json(plan_json)`; `render_id = format!("render-{}", plan_digest.to_hex().get(..24)...)`; tenant from `ctx.run.locator.tenant_scope`.
4. Initial `SceneState` per scene: pinned `FrameSource::Artifact`/`Url` prefill the artifact/url fields; stage = `PendingStartFrame` when `start_frame` is a prompt, else `PendingEndFrame` when `end_frame` is a prompt, else `PendingSubmit`.
5. `state.insert(...)`; a `render_exists` integrity error is **not** a failure — load and return the persisted state (idempotent resubmission = resume).

- [ ] **Step 3: Implement `advance` (one bounded tick)**

Load (missing → `MEDIA_PIPELINE_NOT_FOUND`); terminal states return as-is; reparse `plan_json`. One pass over scenes, at most one tool call per scene, `Polling` count capped by `limits.max_concurrent_jobs`:

- `PendingStartFrame` / `PendingEndFrame`: `invoke_tool(media_tools, "openrouter_generate_image", ...)` with the scene-or-default image model and the frame prompt (pass the image tool's other schema keys per its live schema — read the image ToolSpec and satisfy its required-with-null convention). Store the result's `artifact` (absent → scene `Failed`: the pipeline requires a store-backed media toolset); advance to the next pending stage.
- `PendingSubmit` (while under the cap): `invoke_tool(media_tools, "openrouter_generate_video", ...)` — arguments must satisfy the required-with-null schema: `model` (override-or-default), `prompt` = `video_prompt`, `duration` = effective scene duration, `resolution`/`aspect_ratio` from overrides-or-defaults (null when unset), `size: null`, `seed`, `generate_audio: null`, `first_frame`/`last_frame` as `{"artifact": ...}` or `{"url": ...}` from state (null when absent), `reference_images` from the plan (null when absent). Record `job_id` from the result's `id`; stage → `Polling`.
- `Polling`: `invoke_tool(media_tools, "openrouter_get_video", {"id": job_id, "wait_seconds": 15})` — a short in-call wait so one tick makes progress without blocking the batch. Result `status`: `completed` → `PendingDownload`; `failed` → if `!resubmitted` set `resubmitted = true`, clear `job_id`, stage → `PendingSubmit`, else stage → `Failed` ("video job failed"); `pending`/`in_progress` → no change.
- `PendingDownload`: `invoke_tool(media_tools, "openrouter_download_video", {"id": job_id})`; record the result's `artifact` as `clip_artifact`; stage → `Done`.

A per-scene tool error marks that scene `Failed` (bounded `failure` = the error's code/message) instead of aborting the tick. After the pass:

- any scene `Failed` and none in-flight → `RenderStatus::Failed`;
- all `Done` → **first**, when any scene has cues: `cue_timeline` → `to_srt`/`to_vtt` → `stage_required_artifact` both into `artifact_store` (kind `"tool-output"`, media types `application/x-subrip` / `text/vtt`, scope per Global Constraints), record `transcript_*_artifact`, persist via `state.update` **before** composing (a crash between transcript and compose resumes without regenerating). **Then** build the `compose_video` spec: clips in scene order via `clip_artifact`, plan transitions mapped positionally (a transition `after: scene-k` becomes the spec transition at index k; gaps filled with `cut`), audio/output copied; `output.captions == burn_in` → `subtitles: {artifact: srt, mode: "burn_in"}`; `sidecar` on mp4 → `mode: "mux"`; `sidecar` on webm → no subtitles (refs alone are the deliverable). `invoke_tool(compose_tools, "compose_video", ...)`; record `final_artifact`; → `Completed` (compose failure → `Failed`, except: a compose failure whose message carries `video_compose_media_failure` demotes every scene whose `clip_artifact` no longer verifies back to `PendingDownload` when it has a `job_id` else `PendingSubmit`, and sets status back to `Running` — the at-least-once re-run);
- otherwise `Running`.

Persist with `state.update`; a lost CAS (`false`) → reload and return the newer state (no retry). Honor `ctx.run.cancellation` between scenes (persist progress, return current state).

- [ ] **Step 4: Driver tests**

Read `crates/finstack-ai-test/src/scripted/toolset.rs` first; if `ScriptedToolset`'s plan vocabulary cannot express per-call queued JSON results cleanly, write a local `QueueToolset` double (a `Toolset` whose `tools()` returns hand-built minimal specs for the five tool names and whose `call` pops `(expected_tool_name, result_json)` from a `Mutex<VecDeque>`, panicking on mismatch). `tool_context()` copied from the E2B test module.

1. `happy_path_two_scene_render_reaches_completed` — fixture plan; queue: image gen ×2 (scene-01 start/end), submit ×2, poll pending, poll completed ×2, download ×2, compose. Loop `advance` until `Completed`; assert stage progression, `final_artifact`, `transcript_srt_artifact`/`transcript_vtt_artifact` present, the staged SRT bytes equal the Task 8 known answer for the plan's cues, and the compose call's spec JSON carried 2 clips, 1 crossfade, and `subtitles.mode == "burn_in"`.
2. `concurrency_cap_holds_back_submissions` — 3-scene plan, `max_concurrent_jobs: 1`; after two ticks exactly one scene has a `job_id`.
3. `failed_job_is_resubmitted_once_then_fails_the_scene`.
4. `budget_violation_rejects_before_any_tool_call` — `max_scenes: 1` vs 2 scenes → `media_pipeline_budget_exceeded`, queue untouched.
5. `resubmitting_the_same_plan_resumes_instead_of_duplicating`.
6. `caption_plan_without_a_store_is_rejected_at_submit` — `artifact_store: None` → `media_pipeline_plan_invalid`, queue untouched.

- [ ] **Step 5: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Drive MoviePlan renders through media tools with bounded resumable ticks"
```

---

### Task 10: Pipeline toolset — `render_movie`, `advance_render`, `get_render_status`

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/tools.rs`
- Modify: `extensions/workflow/finstack-ai-workflow-media-pipeline/src/lib.rs` (`mod tools;` + `pub use tools::MediaPipelineToolset;`)

**Interfaces:**
- Produces: `pub struct MediaPipelineToolset` (`try_new(driver: Arc<MediaPipelineDriver>) -> Result<Self, PipelineError>`, `impl Toolset`, descriptor name `finstack-media-pipeline`). Shared result JSON:

```json
{"render_id":"...","status":"running","per_scene":[{"id":"scene-01","stage":"polling","job_id":"vid-1","clip_artifact":{...}}],"final_artifact":{...},"transcript_srt_artifact":{...},"transcript_vtt_artifact":{...}}
```

- [ ] **Step 1: Tool specs**

`render_movie` — description `"Validate and start one MoviePlan render, then run one pipeline tick."`; `Sequential`, `NonIdempotentWrite`, `AtMostOnce`, approval `Policy` reason `"paid multi-scene media generation"`, `max_result_bytes: 262_144`, `deferral: Never`; input `{"additionalProperties":false,"properties":{"plan":{"type":"object"}},"required":["plan"],"type":"object"}` (the plan object is validated by `validate_plan`, not the ToolSpec — the authoritative schema lives in `schemas/movie-plan/`).
`advance_render` — description `"Run one bounded tick of a submitted render (generate frames, submit and poll jobs, download clips, compose when done)."`; same paid metadata; input `{"render_id": string}` required.
`get_render_status` — `ReadOnly`, `AtMostOnce`, approval `NotRequired`; input `{"render_id": string}`.
Shared output schema:

```rust
br#"{"additionalProperties":false,"properties":{"final_artifact":{"type":"object"},"per_scene":{"items":{"additionalProperties":false,"properties":{"clip_artifact":{"type":"object"},"failure":{"type":"string"},"id":{"type":"string"},"job_id":{"type":"string"},"stage":{"type":"string"}},"required":["id","stage"],"type":"object"},"type":"array"},"render_id":{"type":"string"},"status":{"type":"string"},"transcript_srt_artifact":{"type":"object"},"transcript_vtt_artifact":{"type":"object"}},"required":["render_id","status","per_scene"],"type":"object"}"#
```

- [ ] **Step 2: Dispatch**

`verify_authority` + identity checks (E2B shape). `render_movie`: re-serialize the `plan` argument to bytes, `driver.submit_plan`, then one `driver.advance`. `advance_render`: `driver.advance`. `get_render_status`: `driver.status` with the tenant from `ctx.run.locator.tenant_scope`. One shared `fn render_state_json(state: &RenderState) -> serde_json::Value` (stage/status via their serde snake_case names; omit `None` fields).

- [ ] **Step 3: Tests**

1. `render_movie_submits_and_ticks_once` — queued doubles from Task 9; result `status == "running"` with one tick of progress.
2. `advance_render_progresses_to_completion` — loop until `"completed"`; assert `final_artifact`, `transcript_srt_artifact`, `transcript_vtt_artifact` present.
3. `get_render_status_is_read_only` — call twice after completion; queue untouched; identical results.
4. `unknown_render_id_maps_to_not_found` — → `media_pipeline_not_found`.

- [ ] **Step 4: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Expose the media pipeline as render, advance, and status tools"
```

---

### Task 11: Resume goldens

**Files:**
- Create: `extensions/workflow/finstack-ai-workflow-media-pipeline/tests/resume.rs`

- [ ] **Step 1: Write the golden tests**

1. `killed_after_scene_one_download_resumes_at_scene_two` — sqlite state store in a tempdir; drive until scene-01 `Done`, scene-02 `Polling`; drop driver + doubles (simulated crash); fresh driver over the same sqlite path with doubles seeded only with scene-02's remaining calls; `advance` to completion; assert scene-01's `clip_artifact` survived unchanged and no scene-01 call was consumed.
2. `corrupted_clip_fails_closed_then_reruns_the_scene` — complete both downloads against an `InProcessArtifactStore`; make the compose double return the `video_compose_media_failure`-coded error; `advance`; assert scene-01 demoted (to `PendingDownload` — it has a `job_id`) and status `Running`; seed a re-download + compose; assert `Completed`.
3. `resubmitted_plan_after_crash_returns_the_persisted_render` — `submit_plan`, crash, fresh driver, `submit_plan` same bytes; the returned state carries pre-crash progress.

- [ ] **Step 2: Run and commit**

Run: `cargo test -p finstack-ai-workflow-media-pipeline --test resume`
Expected: PASS.

```bash
git add extensions/workflow/finstack-ai-workflow-media-pipeline
git commit -m "Prove pipeline resume and fail-closed re-run with golden tests"
```

---

## Phase D — SDK wiring, docs, CI

### Task 12: Composition surface, bindings, docs, full CI

**Files:**
- Modify: `crates/finstack-ai/Cargo.toml` — optional deps + features mirroring the existing `tool-openrouter-media = ["native-tokio", "dep:finstack-ai-tools-openrouter-media"]` pattern (~line 31): add `tool-video-compose` and `workflow-media-pipeline`; add both to the `linked-tools` aggregate if that matches its intent (read the feature's doc comment first).
- Modify: `crates/finstack-ai/src/agent/linked.rs` — spec structs following the in-file `OpenRouterMediaToolsSpec`/`register_openrouter_media` idioms (~lines 106, 260): `VideoComposeSpec { ffmpeg_path, ffprobe_path, scratch_dir, render_timeout_s }` and `MediaPipelineSpec { limits: (max_scenes, max_total_video_s, max_concurrent_jobs), sqlite_state_path }`; the pipeline registration wires `MediaPipelineToolset` over the constructed media/compose toolsets and the host's artifact store, erroring cleanly when a required piece is missing.
- Modify: `scripts/wasm_package/check.py` — add `finstack-ai-tools-video-compose` and `finstack-ai-workflow-media-pipeline` to `FORBIDDEN_WASM` (line ~19).
- Modify: `bindings/finstack-ai-python/src/agent.rs` + `bindings/finstack-ai-python/python/finstack_ai/_finstack_ai.pyi` — kwargs mirroring the new spec structs on the same factories that carry the openrouter-media kwargs; a pytest asserting construction succeeds with the pipeline enabled and fails with the mapped configuration error when `ffmpeg_path` is missing.
- Modify: `bindings/finstack-ai-wasm/src/agent/agent.rs` — new fields rejected with the existing stable native-only error (mirror the E2B handling).
- Modify: `crates/finstack-ai/README.md` (add a "Media pipeline" pointer; the standalone docs site the plan originally named no longer exists, so the SDK crate guide is the live index) and expand `extensions/workflow/finstack-ai-workflow-media-pipeline/README.md` — usage guide: host composition snippet (`LocalArtifactStore::try_new(root).with_max_artifact_bytes(256 * 1024 * 1024)` → media toolset `with_artifact_store` → compose toolset → pipeline driver + tools); the MoviePlan contract with a link to `schemas/movie-plan/movie-plan.v1.json`; scene-prompt authoring guidance (concrete nouns, camera language, consistent style tokens across a scene's two frames, motion described relative to the start frame); caption authoring guidance (short cues of at most ~7 words for short-form, cue timing aligned to scene beats, `output.captions: "burn_in"` recommended for muted autoplay platforms).
- Modify: `CHANGELOG.md` — one entry per touched/new crate, matching the file's convention.

- [ ] **Step 1: SDK + packaging wiring** (as the file list; keep an E2B-style guard test in the compose crate asserting it stays off the `wasm-host` feature line)
- [ ] **Step 2: Bindings** (Python kwargs + `.pyi` + pytest; wasm stubs)
- [ ] **Step 3: Docs and changelog**
- [ ] **Step 4: Full verification gate**

```bash
mise run ci-rust
mise run ci-python
mise run ci-wasm
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/finstack-ai scripts/wasm_package/check.py bindings CHANGELOG.md extensions
git commit -m "Wire the media pipeline stack through the SDK, bindings, and docs"
```

---

## Execution order and independence

Tasks run 1 → 12. Tasks 1–2 (media toolset) and Tasks 3–5 (compose) are independent of each other; Tasks 6–11 need both; Task 12 needs everything. Every task ends green and committed; the layers are independently shippable in the spec's delivery order (spec §11). No storage tasks exist: hosts compose `LocalArtifactStore` / `S3ArtifactStore` from `extensions/artifacts/finstack-ai-store-artifact` with a raised `max_artifact_bytes`.
