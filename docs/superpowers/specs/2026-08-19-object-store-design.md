# Object Store Service and S3 Backend — Design

Date: 2026-08-19
Status: Approved design, pre-implementation
Related: `docs/superpowers/specs/2026-08-19-media-pipeline-design.md` (MediaStore),
`docs/superpowers/specs/2026-08-19-knowledge-ingestion-design.md` (SourceStore),
`docs/implementation/adrs/ADR-048-shared-authority-and-provider-secret.md` (host-service precedent)

## 1. Problem and goals

Extensions need a durable place to put documents and other unstructured data.
Today the only shared blob surface is `ArtifactStore`
(`crates/finstack-ai-runtime/src/services/artifact.rs`): capped at 4 MiB per
artifact, `Bytes`-in/`Bytes`-out, and implemented only in memory
(`InProcessArtifactStore`). Real workloads carry 20–30 MB PDFs and larger
unstructured files, and two designed-but-unbuilt surfaces (`MediaStore`,
`SourceStore`) each independently scope an S3 backend.

Goals:

1. A general-purpose, host-supplied **`ObjectStore`** service trait that any
   extension can consume to store and retrieve unstructured objects.
2. An **S3-compatible backend** that works against AWS S3, MinIO, and Garage.
3. A **local-filesystem backend** for development and hosts without S3.
4. An **adapter implementing `ArtifactStore` over `ObjectStore`** so existing
   consumers (document ingest, context memory) gain durable storage
   immediately.
5. **Per-store artifact size limits** replacing the hard 4 MiB constant, so
   S3-backed artifact storage can accept large PDFs while the in-memory store
   keeps a small ceiling.
6. One shared S3 client implementation in the tree: the media-pipeline plan's
   `S3MediaStore` is respecified as an adapter over `ObjectStore`.

Non-goals (v1): multipart uploads, presigned PUT, server-side encryption
configuration, object versioning, lifecycle rules, WASM/Python binding
exposure (native-only, like `finstack-ai-store-sqlite`), and any new
`ComponentKind`.

## 2. Placement and architecture

`ObjectStore` is a **host-supplied service**, not a seventh registry port.
The six `ComponentKind`s stay closed; the trait lives in
`crates/finstack-ai-runtime/src/services/object.rs` beside `ArtifactStore`.
Hosts construct a backend and pass it as `Arc<dyn ObjectStore>`; extensions
receive it at construction time, the same way `finstack-ai-tools-document`
receives `Arc<dyn ArtifactStore>` today. There is no dynamic discovery and no
per-call registry lookup.

Wiring:

- `RuntimeServices` (`crates/finstack-ai/src/bundle/catalog.rs`) gains
  `pub object_store: Option<Arc<dyn ObjectStore>>`.
- `HostFeature` / `RequiredServices` gain the requirement string
  `"object_store"`, validated by `RuntimeServices::validate`, so bundles can
  declare the dependency declaratively.
- A new ADR `object-store-contract` (modeled on ADR-048) records the trait
  contract, frozen error codes, and scope-binding rules, with a row in
  `docs/implementation/adr-register.md`.

New crates, all trusted native leaves (T1) under `extensions/stores/`:

| Crate | Contents |
|---|---|
| `finstack-ai-store-object-s3` | `S3ObjectStore`, `S3ObjectStoreConfig`, `sigv4.rs` |
| `finstack-ai-store-object-local` | `LocalObjectStore` over a root directory |
| `finstack-ai-store-artifact-object` | `ObjectArtifactStore: ArtifactStore` adapter over any `Arc<dyn ObjectStore>` |

Shared test support in `crates/finstack-ai-test`: `FakeObjectStore`
(in-memory) plus a reusable contract-test suite run against all backends.

Workspace changes: three new members in the root `Cargo.toml`, entries in
`[workspace.dependencies]`, and exactly one new external dependency,
`hmac = { version = "0.12", default-features = false }` (the dependency the
media-pipeline plan already approved). Leaf crates use
`{ workspace = true }` deps only and copy the standard lint header from
`extensions/context/finstack-ai-context-memory/src/lib.rs`. The S3 crate
offers a `vendored-tls` feature (`reqwest/native-tls-vendored`) like the
other network leaves.

## 3. Trait and data model

```rust
pub trait ObjectStore: PortObject {
    fn put(&self, scope: ObjectScope, key: ObjectKey, content: PutPayload,
           metadata: ObjectMetadata) -> PortFuture<Result<ObjectRef, ObjectError>>;
    fn get(&self, scope: ObjectScope, key: ObjectKey)
        -> PortFuture<Result<Bytes, ObjectError>>;
    fn get_to_file(&self, scope: ObjectScope, key: ObjectKey, dest: PathBuf)
        -> PortFuture<Result<ObjectRef, ObjectError>>;
    fn head(&self, scope: ObjectScope, key: ObjectKey)
        -> PortFuture<Result<ObjectRef, ObjectError>>;
    fn delete(&self, scope: ObjectScope, key: ObjectKey)
        -> PortFuture<Result<(), ObjectError>>;
    fn list(&self, scope: ObjectScope, prefix: Option<ObjectKey>, page: PageToken)
        -> PortFuture<Result<ObjectPage, ObjectError>>;
    fn presign_get(&self, scope: ObjectScope, key: ObjectKey, expiry: Duration)
        -> PortFuture<Result<PresignedUrl, ObjectError>>;
    fn limits(&self) -> ObjectStoreLimits;
}
```

### 3.1 Scoping (fail-closed)

```rust
pub struct ObjectScope {
    pub tenant_scope: Arc<str>,          // required, non-empty, no NUL
    pub session_id: Option<SessionId>,   // optional: objects may outlive sessions
    pub run_id: Option<RunId>,
    pub sensitivity: Sensitivity,
}
```

`ObjectScope::digest()` canonicalizes with `serde_json_canonicalizer` and
hashes with `Digest::domain_separated("object-scope", 1, ...)`, mirroring
`ArtifactScope::digest()`. Unlike `ArtifactScope`, `session_id` is optional
so tenant-lifetime documents are expressible.

Physical object key: `{key_prefix}/{scope_digest_16hex}/{logical_key}`.
Every `ObjectRef` carries `scope_digest`; any operation whose supplied scope
digest does not match the stored object's scope fails closed with
`OBJECT_SCOPE_MISMATCH`. `list` can only enumerate within the caller's scope
prefix by construction.

### 3.2 Keys, payloads, refs

- `ObjectKey::try_new(&str)` validates: 1–512 bytes, UTF-8, segments joined
  by `/`, charset `[A-Za-z0-9._-]` per segment, no empty segments, no `..`,
  no leading `/`. Violations → `OBJECT_INVALID_KEY`.
- `PutPayload::{Bytes(Bytes), File(PathBuf)}`. `File` streams from disk
  (reqwest streaming body) and `get_to_file` streams to disk, so large
  objects never require full in-memory materialization. This is what allows
  the spec'd `S3MediaStore` (streaming-by-path, never through memory) to be
  an adapter instead of a second S3 client.
- `ObjectRef { key, scope_digest, content_digest, length, media_type }`.
  `content_digest` is SHA-256 of the payload, computed during `put`, stored
  as object metadata (`x-amz-meta-fsai-digest` on S3; sidecar-free encoding
  in the local backend), and verified on `get`/`get_to_file`
  (`OBJECT_INTEGRITY_FAILURE` on mismatch).
- `ObjectMetadata { media_type, name: Option, attributes: Metadata }` —
  bounded, non-secret, non-authoritative, same rules as `ArtifactMetadata`.
- `ObjectPage { entries: Vec<ObjectEntry>, next: Option<PageToken> }` with
  backend-opaque `PageToken`. `ObjectEntry { key, length }` only — S3
  `ListObjectsV2` does not return user metadata, so listings carry what the
  listing API provides; callers needing the digest or media type follow up
  with `head`.

### 3.3 Limits

`ObjectStoreLimits { max_object_bytes: u64 }`. Default for the S3 backend:
**5 GiB** (the S3 single-PUT protocol ceiling — the natural cap until
multipart lands), configurable downward per host. Local backend defaults the
same. Oversize puts are rejected, never truncated (`OBJECT_TOO_LARGE`).

### 3.4 Frozen error codes

`OBJECT_UNAVAILABLE`, `OBJECT_NOT_FOUND`, `OBJECT_SCOPE_MISMATCH`,
`OBJECT_INTEGRITY_FAILURE`, `OBJECT_TOO_LARGE`, `OBJECT_INVALID_KEY`,
`OBJECT_INVALID_METADATA`, `OBJECT_UNSUPPORTED`, `OBJECT_IO_FAILURE` — a
`thiserror` enum with a `code()` accessor, same style as `ArtifactError`.
`OBJECT_UNSUPPORTED` exists because not every backend supports every
operation (the local backend cannot presign).

## 4. S3 backend (`finstack-ai-store-object-s3`)

### 4.1 Configuration

```rust
S3ObjectStoreConfig::try_new(endpoint: Url, bucket: &str, region: &str)
```

Builder methods: `with_key_prefix`, `with_addressing(Addressing::{Path,
VirtualHost})` (default `Path` — MinIO/Garage convention; `VirtualHost` for
AWS), `with_credentials(access_key_id, secret_access_key: SecretString)`,
`with_timeout`, `with_max_object_bytes`, `with_presign_expiry_max`.

Validation mirrors `AnthropicConfig::try_new`: reject endpoint URLs carrying
credentials, query, or fragment; scheme must be HTTP(S). All secret-bearing
types get hand-written `Debug` printing `[REDACTED]`. **No environment
variable reads anywhere** — the host supplies credentials explicitly, per
the credential rules in
`crates/finstack-ai-runtime/src/ports/model/provider_util/credentials.rs`.

### 4.2 SigV4 signing (`sigv4.rs`)

Hand-rolled AWS Signature Version 4 over the existing `sha2` plus the new
`hmac` dependency: canonical request → string-to-sign → derived signing key.
Header signing for normal operations with a signed, exact
`x-amz-content-sha256` (the payload digest is computed anyway for
integrity); query-string presigning (`X-Amz-*` parameters) for
`presign_get`. Unit-tested against AWS's published SigV4 known-answer test
vector.

### 4.3 S3-compatibility requirements

- Path-style addressing with arbitrary endpoint and port (MinIO
  `http://minio:9000`, Garage custom ports).
- Region is an arbitrary string (Garage commonly uses `garage`).
- Operations used: `PutObject`, `GetObject`, `HeadObject`, `DeleteObject`,
  `ListObjectsV2` (with `prefix` + `continuation-token` pagination),
  presigned `GetObject`.
- No reliance on AWS-only response fields; user metadata via
  `x-amz-meta-*` headers only.

## 5. Local backend (`finstack-ai-store-object-local`)

`LocalObjectStore::try_new(root_dir)` mirrors the physical key layout as
directories under `root_dir`. Writes go to a `tempfile` in the same
directory followed by an atomic rename; the content digest and metadata are
encoded alongside the object (metadata sidecar file written before the
rename that publishes the object). `presign_get` returns
`OBJECT_UNSUPPORTED`. Path traversal is impossible by construction because
`ObjectKey` validation excludes `..` and absolute segments.

## 6. ArtifactStore integration

### 6.1 Per-store artifact limits (replaces the hard 4 MiB constant)

`ArtifactStore` gains:

```rust
fn limits(&self) -> ArtifactStoreLimits;   // default impl: 4 MiB
pub struct ArtifactStoreLimits { pub max_artifact_bytes: usize }
```

- The default trait implementation returns the current 4 MiB, so existing
  implementations keep working unchanged.
- Size enforcement in `stage_put` paths compares against `self.limits()`
  instead of the constant. `MAX_ARTIFACT_BYTES` remains exported as the
  default value (public-API baselines regenerated).
- Consumers that hard-code the constant switch to the live limit:
  - `extensions/toolsets/finstack-ai-tools-document/src/parser.rs` — parser
    input ceiling becomes configurable, defaulting to the store's limit.
    This is the line that currently blocks 20–30 MB PDFs.
  - `extensions/toolsets/finstack-ai-tools-filesystem/src/lib.rs` /
    `operation.rs` — file-size gate likewise.
- `InProcessArtifactStore` keeps the 4 MiB default and gains
  `with_max_artifact_bytes(...)` for hosts that want more in development.

Raising the ceiling is safe because artifact bytes never enter the journal:
the stage-before-journal rule journals only `ArtifactRef`s. The remaining
memory consequence — an attachment transits one protocol message and is
materialized during parsing — is inherent to parsing and acceptable on
native hosts; WASM/browser hosts keep small limits via per-store
configuration.

### 6.2 Adapter (`finstack-ai-store-artifact-object`)

`ObjectArtifactStore::new(Arc<dyn ObjectStore>, options)` implements
`ArtifactStore`:

- `stage_put` → `ObjectStore::put` with `PutPayload::Bytes`, mapping
  `ArtifactScope` → `ObjectScope` (session required, sensitivity carried
  through) and `ArtifactMetadata` → `ObjectMetadata`; logical keys derive
  from the artifact id.
- `get` → `ObjectStore::get`, re-verifying digest and scope.
- Default `max_artifact_bytes`: **64 MiB** (configurable). Deliberately far
  below the object cap because the `ArtifactStore` trait is
  `Bytes`-in/`Bytes`-out — everything through it is fully materialized in
  memory by consumers. 64 MiB covers 20–30 MB PDFs with headroom; larger
  payloads belong on the streaming `ObjectStore`/`MediaStore` path.
- Error mapping table `ObjectError` → `ArtifactError` fixed in the ADR
  (e.g. `OBJECT_NOT_FOUND` → `ARTIFACT_NOT_FOUND`, `OBJECT_TOO_LARGE` →
  `ARTIFACT_TOO_LARGE`, everything transport-ish → `ARTIFACT_UNAVAILABLE`).

Result: host wires `S3ObjectStore` → `ObjectArtifactStore` → document-ingest
middleware stages 30 MB PDFs durably with no change to ingest code.

### 6.3 Reconciliation with MediaStore and SourceStore

- `docs/superpowers/specs/2026-08-19-media-pipeline-design.md` §4.3 and the
  plan's Phase E (Tasks 14–15) are amended: `S3MediaStore` becomes an
  adapter over `Arc<dyn ObjectStore>` (its streaming contract is satisfied
  by `PutPayload::File` / `get_to_file`; its presigning by `presign_get`),
  and the planned standalone `sigv4.rs` moves here. One SigV4
  implementation in the tree.
- The knowledge-ingestion spec's future S3 `SourceStore` backend likewise
  adapts over `ObjectStore` when built.

## 7. Data flow

Put (S3 backend): consumer supplies scope + logical key + payload →
key/metadata validation → size check against limits → physical key
composed from scope digest → SHA-256 computed (streaming for `File`
payloads) → SigV4-signed `PutObject` with digest in `x-amz-meta-fsai-digest`
→ `ObjectRef` returned to the consumer, who journals it (never the bytes).

Get: scope digest recomputed from the caller's scope → physical key → signed
`GetObject` → digest verified against stored metadata → `Bytes` (or file at
`dest`) returned; scope or digest mismatch fails closed.

## 8. Error handling

All failures map to the frozen codes in §3.4; transport errors (DNS, TLS,
timeouts, 5xx) surface as `OBJECT_UNAVAILABLE` with a non-secret detail
string; 403 from the service surfaces as `OBJECT_UNAVAILABLE` (never echoes
signing material); 404 → `OBJECT_NOT_FOUND`. Error `Display`/`Debug` output
never contains the secret key, the authorization header, or presigned query
strings — enforced by a canary test.

## 9. Testing

- **Contract suite** in `finstack-ai-test`: put/get/head/delete round-trip,
  `get_to_file` round-trip, scope isolation (two scopes, cross-reads fail
  closed), list pagination, not-found semantics, oversize rejection,
  integrity-failure injection, key validation. Run against
  `FakeObjectStore`, `LocalObjectStore`, and `S3ObjectStore`-over-loopback.
- **S3 loopback fixtures**: in-process scripted HTTP listener (tokio `net`
  dev-feature, per the Anthropic-provider fixture pattern) asserting exact
  request shape — canonical headers, signed payload hash, path-style vs
  virtual-host URLs, ListObjectsV2 continuation. Never live AWS/MinIO.
- **SigV4 known-answer test** against AWS's published vector; presign
  known-answer test.
- **Secret-safety canary**: `debug_and_errors_never_leak_the_secret_key`.
- **Adapter tests**: `ObjectArtifactStore` over `FakeObjectStore`, including
  the existing `validate_staged_artifact` helpers and the raised limit.
- **Consumer regression**: document-ingest lane
  (`crates/finstack-ai-test/tests/lanes/document_ingest.rs`) extended with an
  S3-adapter variant staging a >4 MiB fixture.
- Public-API baselines regenerated (`mise run check-public-api`);
  `cargo-deny` passes with the `hmac` addition; minimal-graph fixtures
  unaffected (new deps quarantined to leaf crates).

## 10. Open items carried to the implementation plan

- ADR number assignment for `object-store-contract` (next free in
  `docs/implementation/adr-register.md`).
- Exact amendment text for the media-pipeline spec/plan Phase E.
- Whether the SDK facade (`crates/finstack-ai`) gains optional feature flags
  for the new leaves or hosts depend on them directly (default: direct
  dependency, per `extensions/stores/README.md`).
