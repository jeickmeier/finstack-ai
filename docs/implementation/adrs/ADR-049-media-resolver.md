# ADR-049: MediaResolver contract for provider media input

## Status

Accepted

## Date

2026-08-19

## Accountable role

Core/runtime lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

The kernel already represents media content: `ContentBlock::Image`,
`ContentBlock::Audio`, and `ContentBlock::File` each wrap a `MediaRef`,
and `InputCapabilities` carries `images`/`audio`/`files` flags
(`crates/finstack-ai-kernel/src/content/`). `MediaRef` wraps a
`BlobRef`, and `BlobRef` is labels-only: an id, a media type, a declared
length, an optional digest, an optional name — no bytes, no URL. Its
doc comment is explicit: "The kernel does not fetch the bytes"
(`crates/finstack-ai-kernel/src/content/blob.rs:37`).

No runtime machinery exists to turn a `BlobRef` into bytes or a URL a
provider can put on the wire. Every in-tree provider currently rejects
media content blocks outright. The OpenRouter provider extension adds
image/audio/file input support and needs a way to resolve blob
references to deliverable payloads without teaching the kernel about
bytes, storage, or transport, and without adding a new registered port
for what is fundamentally a host-owned lookup.

ADR-048 already established the pattern for this shape of problem:
`CredentialStore` is a host-supplied object living in
`ports/model/provider_util/`, handed to provider configs directly,
rather than a new port that every host must implement and every
provider must be wired through the port registry to reach.

## Decision

- Add a `MediaResolver` trait to `finstack-ai-runtime`
  `ports/model/provider_util/`, following the `CredentialStore`
  precedent from ADR-048. It is **not** a seventh registered port —
  there is no `MediaResolver` entry in the port registry, no gateway
  dispatch, no wasm-host binding. Hosts construct an implementation and
  hand it to a provider's config as `Arc<dyn MediaResolver>` (mirroring
  `with_credential_store`), e.g. `with_media_resolver`.

  ```rust
  pub trait MediaResolver: PortObject {
      fn resolve(&self, blob: &BlobRef) -> PortFuture<Result<ResolvedMedia, MediaResolveError>>;
  }

  pub enum ResolvedMedia {
      Url(Arc<str>),
      Bytes { media_type: Arc<str>, bytes: Arc<[u8]> },
  }

  pub struct MediaResolveError {
      pub kind: MediaResolveErrorKind,
      pub message: Arc<str>,
  }

  pub enum MediaResolveErrorKind {
      NotFound,
      Unreadable,
      TooLarge,
      Unsupported,
  }
  ```

- Resolution happens inside `Model::request`'s async block, before the
  wire request is built — not during draft translation. Draft
  translation stays synchronous: it takes a pre-resolved map keyed by
  blob id (built by the async resolution step ahead of it) rather than
  calling the resolver itself. This keeps the existing sync
  draft-to-wire translation path unchanged in shape; only the input to
  it grows a resolved-media lookup.
- Missing resolver plus media content in the request fails closed: the
  provider returns its own `*_request_invalid` code (e.g.
  `openrouter_request_invalid`) rather than silently dropping the
  content block or guessing a fallback.
- Resolved byte payloads are bounded by the provider's existing stream
  size limits — a `ResolvedMedia::Bytes` payload does not get a
  separate, larger allowance than any other request body content the
  provider already caps.
- The kernel is untouched: no new `ContentBlock` variant, no change to
  `BlobRef`/`MediaRef`, no new `RecordBody`. wasm-host is unaffected —
  resolution is native-only, living alongside the native provider
  crates; there is no WIT surface for it.
- Wire mapping for resolved media (`input_image`/`input_file`/
  `input_audio` shapes, base64 vs. data-URI vs. raw URL) stays
  crate-local to each provider, per ADR-047's leaves-own-their-protocol
  philosophy. `MediaResolver` and `ResolvedMedia` are the only shared
  pieces.
- `base64` becomes a new workspace dependency if not already pinned,
  for providers that must inline resolved bytes as a data URI or an
  `input_audio` base64 field.

## Consequences

- Any provider crate can adopt media input by accepting
  `Arc<dyn MediaResolver>` on its config and writing its own wire
  mapping — no new port, no registry change, no host-facing contract
  beyond implementing one trait.
- Hosts own blob storage and lifetime entirely; the runtime never
  caches, persists, or re-resolves a blob on the provider's behalf.
  A host that wants caching implements it inside its `MediaResolver`.
- wasm-host and the kernel are unaffected by this record; no
  compatibility surface changes for either.
- Providers that lack a resolver, or whose API cannot accept a given
  modality, keep rejecting that content deterministically via their
  existing `*_request_invalid` code path — no partial/best-effort
  media handling.

## Rejected alternatives

**A seventh registered port (`MediaResolverPort`).** Rejected: this is
a per-provider-config dependency injected by the host, exactly like
`CredentialStore`, not a capability that needs port-registry discovery,
gateway dispatch, or a wasm-host binding. A new port would force every
host and every non-media provider to reason about a port they never
use.

**Async draft translation.** Rejected: draft-to-wire translation is
synchronous today across all providers; forcing it async to call the
resolver inline would ripple through every provider's translation
path. Resolving up front into a map keeps translation sync and
confines the async work to the one place (`Model::request`) that is
already async.

**A kernel-level `Video` content block for video input.** Rejected:
out of scope for this record. Video input rides the existing `File`
path with a `video/*` media type; adding a first-class kernel variant
is a separate, larger decision.

**Bridging `MediaResolver` into linked SDK constructors (Python/JS
callbacks).** Rejected for this effort: a resolver is a host-authored
Rust object called from the async request path. Bridging a foreign
callback across the FFI boundary with the right lifetime and error
semantics is future work, not required for the initial contract.

## Compatibility and schema-change classification

Additive: new `MediaResolver`, `ResolvedMedia`, `MediaResolveError`,
`MediaResolveErrorKind` types in `ports/model/provider_util/`, plus a
new `with_media_resolver`-shaped config method on providers that adopt
them. No kernel type changes, no journal `RecordBody` change, no WIT
world change, no remote-protocol meaning change. Pre-1.0 extensions may
adopt this without a major version bump.

## Security classification

- References: SEC-INV-004 (host owns externally-fetched content); TM-04
  (secrets/opaque references stay references, not inlined bytes,
  outside the trust boundary that resolves them)
- Threat Model review trigger: none invented by this record. A provider
  crate that implements resolution against a network-reachable blob
  store completes its own review when it lands.
- Residual: the runtime does not itself validate that a resolved URL or
  byte payload is safe to send to a third-party model API; that
  judgment stays with the host's `MediaResolver` implementation and the
  provider's own size/type bounds.

## Affected requirements, design, and delivery

- Affected Technical Design: Model port, provider batteries
  (`ports/model/provider_util/`). Planning files are amended alongside
  this record.
- Implementation: `MediaResolver`, `ResolvedMedia`, and the error types
  land in `provider_util` in a sibling task to this ADR. Per-provider
  `with_media_resolver` config methods and wire mapping land with each
  provider's media-input work.
- Unchanged: no seventh port; kernel untouched; wasm-host untouched.

## Supersession metadata

None. This ADR supersedes no prior ADR.

## Reconsideration conditions

May change only through a new superseding ADR. Promoting media
resolution to a registered port, moving resolution into draft
translation (making it async), or adding a kernel-level `Video`
content block each require that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to authorize the OpenRouter
  provider media-input effort
- Implementation evidence: Missing (local types only; no invented
  evidence or review id)
