# ADR-050: Object store contract and S3/local backends

## Status

Accepted

## Date

2026-08-20

## Accountable role

Core/runtime lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

Extensions need a durable place to put documents and other large
unstructured data. The only shared blob surface today is `ArtifactStore`
(`crates/finstack-ai-runtime/src/services/artifact.rs`): capped at 4 MiB
per artifact, `Bytes`-in/`Bytes`-out, and implemented only in memory
(`InProcessArtifactStore`). Real workloads carry 20–30 MB PDFs and larger
unstructured files, well past that ceiling.

Two designed-but-unbuilt surfaces already assume an S3 backend:
`MediaStore` (`docs/superpowers/specs/2026-08-19-media-pipeline-design.md`)
and `SourceStore` (`docs/superpowers/specs/2026-08-19-knowledge-ingestion-design.md`).
Left alone, each would grow its own S3 client, its own SigV4 signer, and
its own scoping rules — three copies of the same transport and isolation
logic for what is fundamentally one problem: putting and getting bytes
under a caller's scope, durably, from a backend the host chooses.

ADR-048 already established the pattern for a host-supplied service that
is not a seventh registry port: `CredentialStore` lives in
`ports/model/provider_util/`, constructed by the host and handed to
consumers directly. `ObjectStore` follows the same shape.

## Decision

- `ObjectStore` is a host-supplied service trait, not a seventh registry
  port. The six `ComponentKind`s stay closed. The trait lives in
  `crates/finstack-ai-runtime/src/services/object.rs`, beside
  `ArtifactStore`. Hosts construct a backend and pass it as
  `Arc<dyn ObjectStore>`; extensions receive it at construction time, the
  same way `finstack-ai-tools-document` receives `Arc<dyn ArtifactStore>`
  today. There is no dynamic discovery and no per-call registry lookup.
  `RuntimeServices` gains `pub object_store: Option<Arc<dyn ObjectStore>>`,
  and `HostFeature`/`RequiredServices` gain the requirement string
  `"object_store"` so bundles can declare the dependency declaratively.

- **Scoped, fail-closed keys.** `ObjectScope { tenant_scope, session_id:
  Option<SessionId>, run_id: Option<RunId>, sensitivity }` differs from
  `ArtifactScope` only in that `session_id` is optional, so objects may
  outlive a session. `ObjectScope::digest()` canonicalizes with
  `serde_json_canonicalizer` and hashes with
  `Digest::domain_separated("object-scope", 1, ...)`, mirroring
  `ArtifactScope::digest()`. The physical backend key is composed as
  `{prefix}/{scope-digest-16-hex}/{key}` (`physical_object_key`). Every
  `ObjectRef` carries the `scope_digest` it was written under; any
  operation whose supplied scope digest does not match the stored
  object's scope fails closed with `OBJECT_SCOPE_MISMATCH`. `list` can
  only enumerate within the caller's scope prefix by construction, and
  `ObjectKey::try_new` rejects empty keys, keys over 512 bytes, leading
  `/`, empty segments, `.`/`..` segments, and any byte outside
  `[A-Za-z0-9._-]` (→ `OBJECT_INVALID_KEY`).

- **Frozen `object_*` codes**, exactly as landed in
  `crates/finstack-ai-runtime/src/services/object.rs`:
  `object_unavailable`, `object_not_found`, `object_scope_mismatch`,
  `object_integrity_failure`, `object_too_large`, `object_invalid_key`,
  `object_invalid_metadata`, `object_unsupported`, `object_io_failure`.
  `ObjectError` is a `thiserror` enum with a `const fn code(&self)
  -> &'static str` accessor, same style as `ArtifactError`.
  `OBJECT_UNSUPPORTED` exists because not every backend supports every
  operation — the local backend cannot presign.

- **Per-store `ArtifactStoreLimits`**, replacing the hard 4 MiB constant.
  `ArtifactStore` gains `fn limits(&self) -> ArtifactStoreLimits` with a
  default implementation returning the current 4 MiB
  (`ArtifactStoreLimits { max_artifact_bytes: usize }`), so existing
  implementations keep working unchanged. Size enforcement in `stage_put`
  paths compares against `self.limits()` instead of the constant.
  `MAX_ARTIFACT_BYTES` remains exported as the default value.
  `InProcessArtifactStore` keeps the 4 MiB default and gains
  `with_max_artifact_bytes(...)`. Consumers that hard-coded the constant
  (`finstack-ai-tools-document`'s parser input ceiling,
  `finstack-ai-tools-filesystem`'s file-size gate) switch to the live
  limit.

- **`ObjectStoreLimits { max_object_bytes: u64 }`**, defaulting to
  **5 GiB** for both the S3 backend and the local backend — the S3
  single-PUT protocol ceiling, and the natural cap until multipart lands.
  Oversize puts are rejected, never truncated (`OBJECT_TOO_LARGE`).

- **`ObjectArtifactStore`** (`finstack-ai-store-artifact-object`) adapts
  any `Arc<dyn ObjectStore>` into `ArtifactStore`. Its default
  `max_artifact_bytes` is **64 MiB**, clamped to the wrapped store's
  `limits().max_object_bytes`. This is deliberately far below the object
  cap because `ArtifactStore` is `Bytes`-in/`Bytes`-out — everything
  through it is fully materialized in memory by consumers. 64 MiB covers
  20–30 MB PDFs with headroom; larger payloads belong on the streaming
  `ObjectStore`/`MediaStore` path directly.

- **Hand-rolled SigV4** in `finstack-ai-store-object-s3`'s `sigv4.rs`:
  canonical request → string-to-sign → derived signing key, over the
  existing `sha2` dependency plus one new dependency,
  `hmac = { version = "0.12", default-features = false }` — the *only*
  new external dependency this record introduces. Header signing (exact
  `x-amz-content-sha256`) for normal operations; query-string presigning
  (`X-Amz-*` parameters) for `presign_get`. Unit-tested against AWS's
  published SigV4 known-answer vector. `finstack-ai-store-object-local`
  mirrors the physical key layout as directories under an explicit root,
  with a tempfile-then-atomic-rename write path; it returns
  `OBJECT_UNSUPPORTED` for `presign_get`.

- **`ObjectError` → `ArtifactError` mapping**, fixed by this record for
  `ObjectArtifactStore`:

  | `ObjectError` | `ArtifactError` |
  | --- | --- |
  | `NotFound` | `NotFound` |
  | `TooLarge { len, max }` | `TooLarge` (usize-cast) |
  | `ScopeMismatch { .. }` | `ScopeMismatch` |
  | `Integrity { .. }` | `Integrity` |
  | `InvalidKey { .. }` | `InvalidMetadata` |
  | `InvalidMetadata { .. }` | `InvalidMetadata` |
  | `Unavailable { .. }` | `Unavailable` |
  | `Io { .. }` | `Unavailable` |
  | `Unsupported { .. }` | `Unavailable` |

## Consequences

- One SigV4 implementation in the tree: the media-pipeline plan's
  standalone `sigv4.rs` (Phase E, Tasks 14–15) is superseded — `S3MediaStore`
  becomes an adapter over `Arc<dyn ObjectStore>` instead of a second S3
  client. The knowledge-ingestion spec's future S3 `SourceStore` backend
  adapts the same way when built.
- Host wires `S3ObjectStore` → `ObjectArtifactStore` → document-ingest
  middleware and stages 30 MB PDFs durably with no change to ingest code.
- Multipart upload is deferred: the 5 GiB default is the S3 single-PUT
  ceiling, not a design limit. Objects near or over that size are out of
  scope until multipart lands.
- `presign_get` is unsupported on the local backend
  (`OBJECT_UNSUPPORTED`); consumers that need a presigned URL for local
  development must fall back to `get`/`get_to_file`.
- `finstack-ai-store-object-s3` alone takes the `hmac` dependency
  (and the network-client dependencies already carried by other S3-style
  leaves); the runtime and other extensions stay dependency-clean.

## Rejected alternatives

**A seventh registered port (`ObjectStorePort`).** Rejected: this is a
per-consumer dependency injected by the host, exactly like
`CredentialStore` (ADR-048) and `MediaResolver` (ADR-049), not a
capability that needs port-registry discovery, gateway dispatch, or a
wasm-host binding. A new port would force every host and every consumer
that never touches object storage to reason about a port they never use,
and it would violate the closed six-`ComponentKind` boundary.

**`aws-sdk-s3`.** Rejected: the official SDK pulls a large dependency
graph (credential providers, XML/JSON codecs, retry/middleware stacks,
tokio-rustls or hyper transport pinning) into a leaf that only needs
`PutObject`/`GetObject`/`HeadObject`/`DeleteObject`/`ListObjectsV2` and
presigned GET. That is disproportionate to the minimal-graph philosophy
already applied to every other native leaf, and it would fight the
repo's existing `reqwest` + hand-rolled-signing pattern used elsewhere.

**`opendal`.** Rejected: `opendal` is a general storage abstraction layer
with its own trait hierarchy, feature-flag matrix, and backend registry —
exactly the kind of extra abstraction the repo's leaf-crate philosophy
avoids. `ObjectStore` is already the abstraction; adding a second one
underneath it duplicates the boundary for no benefit, and it would pull
in a dependency graph shaped by backends this repo does not use.

**Raw unscoped keys (consumer-supplied full paths).** Rejected: without a
frozen scope-binding scheme, every consumer would re-implement tenant
isolation itself, with no shared fail-closed guarantee and no common
`OBJECT_SCOPE_MISMATCH` behavior. The `{prefix}/{scope-digest-16-hex}/{key}`
layout and the `scope_digest` carried on every `ObjectRef` make isolation
a property of the trait, not a convention consumers must remember to
follow.

## Compatibility and schema-change classification

Additive: new `ObjectStore`, `ObjectScope`, `ObjectKey`, `PutPayload`,
`ObjectMetadata`, `ObjectRef`, `ObjectEntry`, `PageToken`, `ObjectPage`,
`PresignedUrl`, `ObjectStoreLimits`, and `ObjectError` types in
`crates/finstack-ai-runtime/src/services/object.rs`; new
`RuntimeServices::object_store` field; new `"object_store"`
`RequiredServices` string; new `ArtifactStore::limits()` default method
and `ArtifactStoreLimits` type (default-implemented, so existing
`ArtifactStore` implementers are source-compatible without change); three
new extension crates (`finstack-ai-store-object-s3`,
`finstack-ai-store-object-local`, `finstack-ai-store-artifact-object`).
No kernel type changes, no journal `RecordBody` change, no WIT world
change, no remote-protocol meaning change. Pre-1.0 extensions may adopt
this without a major version bump.

## Security classification

- References: SEC-INV-004 (host owns externally-fetched/stored content);
  TM-04 (secrets stay references — credentials never read from the
  environment, never echoed in `Debug`/error output, never present in
  signing material leaked through errors); TM-16 (content-integrity
  digest verified on every read); TM-20 (blob/object isolation across
  tenant scope)
- Threat Model review trigger: none invented by this record. The S3
  backend's own review (fixture-based, never live AWS/MinIO/Garage)
  lands with its implementation task; the secret-safety canary test
  (`debug_and_errors_never_leak_the_secret_key`) is part of that
  evidence.
- Residual: the runtime does not itself validate that a backend's
  network endpoint or credentials are trustworthy; that judgment stays
  with the host that constructs the `S3ObjectStore`/`LocalObjectStore`
  and hands it in.

## Affected requirements, design, and delivery

- Affected Technical Design: runtime services
  (`crates/finstack-ai-runtime/src/services/`), `ArtifactStore`,
  `extensions/stores/`. Design:
  `docs/superpowers/specs/2026-08-19-object-store-design.md`. Planning
  files are amended alongside this record, including the media-pipeline
  spec §4.3 and plan Phase E (Tasks 14–15), which this record supersedes.
- Implementation: `ObjectStore` and its data model land in
  `finstack-ai-runtime`; `S3ObjectStore`/`LocalObjectStore`/
  `ObjectArtifactStore` land as sibling extension-crate tasks to this
  ADR, per `docs/superpowers/plans/2026-08-19-object-store.md`.
- Unchanged: no seventh port; no `ComponentKind` change; no kernel or
  wasm-host surface change.

## Supersession metadata

None. This ADR supersedes no prior ADR. It amends (without superseding)
`docs/superpowers/specs/2026-08-19-media-pipeline-design.md` §4.3 and
`docs/superpowers/plans/2026-08-19-media-pipeline.md` Phase E
(Tasks 14–15), replacing their standalone SigV4/S3-client design with the
`ObjectStore` adapter shape recorded here.

## Reconsideration conditions

May change only through a new superseding ADR. Adding multipart upload
when objects need to exceed the 5 GiB single-PUT ceiling, or introducing
a content-addressed shared namespace across `ObjectStore` consumers when
`SourceStore` lands, each require that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to authorize the object-store
  effort (`docs/superpowers/plans/2026-08-19-object-store.md`)
- Implementation evidence: Missing (local types only; no invented
  evidence or review id)
