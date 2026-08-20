# ADR-051: Gemini native generateContent provider

## Status

Accepted

## Date

2026-08-20

## Accountable role

Ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

OpenRouter can already route text traffic to Gemini models, but the
OpenRouter leaf drops every surface that only exists on Gemini's native
`generateContent` protocol: thought signatures, Google Search grounding,
code execution, context caching, and native audio/video/file input.
ADR-040 already committed the workspace to one dedicated native leaf per
vendor protocol rather than a shared compatibility adapter; a native
Gemini leaf is consistent with that architecture, not an exception to
it.

Vertex AI serves the same `generateContent` wire body and the same SSE
stream shape as the Generative Language API. Only the URL path and the
credential header differ (API key vs. a host-supplied bearer token).
Treating Vertex as a fourth protocol would duplicate the request
translation, streaming assembly, and conformance suite for no wire
difference.

Thought-signature continuation follows the opaque
`ModelResponse.continuation_state` pattern ADR-040 established for
OpenAI Responses: the kernel must not interpret provider-specific
replay state. `functionCall.id` is optional on the wire, matching the
native Ollama precedent (ADR-040 clause 5) where an absent provider ID
is omitted rather than synthesized. ADR-045 froze the public
constructor naming convention (`Agent.<provider>`) this decision must
follow. ADR-047 retired the generic multi-protocol gateway crate and
kept `Agent::gateway` as a thin dispatcher over dedicated leaves, which
this decision extends with a new wire-protocol arm. ADR-048 established
the shared `CredentialStore`/`SecretString` boundary this leaf's Vertex
auth path reuses instead of minting its own OAuth handling. A companion
`MediaResolver` contract (ADR-049 on `main`, not yet present on this
branch) establishes how `BlobRef` values become `inlineData`/`fileData`
payloads; this leaf follows the same shape.

The object-store contract decision already claimed ADR-050 on `main`;
this record is ADR-051. ADR-049 and ADR-050 are not yet present on this
branch's copies of the ADR register and index — reconciling that gap is
left to the branch merge, not to this record.

## Decision

1. Gemini uses a dedicated native provider
   (`finstack-ai-provider-gemini`) and public factory `Agent.gemini`.
   The wire path is `POST {endpoint}:streamGenerateContent?alt=sse`,
   always streaming.
2. Vertex AI is an endpoint/auth variant of the same crate
   (`GeminiEndpoint::Vertex { project, location }`), not a fourth
   protocol. The wire body and stream assembly are shared; only URL
   construction and credential header shape differ. The crate never
   mints OAuth tokens; Vertex hosts supply ready tokens via
   `CredentialStore`.
3. Thought-signature continuation replays complete prior model-turn
   `Content` objects, including `thoughtSignature` fields, from opaque
   `ModelResponse.continuation_state` (envelope
   `gemini.generate-content` v1). Providers decode that blob; the
   kernel does not interpret it (ADR-040 clause 3 applies).
4. `functionCall.id` is preserved as `provider_call_id` when present
   and omitted otherwise (ADR-040 clause 5 precedent).
5. Google Search grounding and code execution activate via the
   settings keys `gemini.google_search` and `gemini.code_execution`;
   `tools` remains a reserved settings key. Grounding metadata and
   code-execution parts surface as `ContentBlock::Opaque` with
   `application/vnd.finstack.gemini.*` media types.
6. Context caching v1 is pass-through and accounting only:
   `gemini.cached_content` maps to `cachedContent`;
   `cachedContentTokenCount`/`thoughtsTokenCount` land in
   `Usage::extension_counters` as
   `gemini.cached_content_token_count`/`gemini.thoughts_token_count`.
   `cachedContents` CRUD, Files API upload, and Live API are out of
   scope.
7. `Agent::gateway` gains a `"gemini_generate_content"` wire-protocol
   arm.

## Consequences

- Gemini tool loops can preserve thought signatures and grounding
  output that OpenRouter's normalized surface discards.
- Vertex AI adopters get the native surface without a second crate or
  a second conformance suite; only endpoint/credential configuration
  changes.
- The crate depends on a host-supplied `CredentialStore` for Vertex
  bearer tokens; it does not implement OAuth token minting or refresh.
- `cachedContents` CRUD, Files API upload, and the Live API remain
  unimplemented; adopters needing those must build them as host-side
  concerns.
- `Agent::gateway` callers can select Gemini via
  `wire_protocol = "gemini_generate_content"` without depending on the
  leaf crate directly.
- Continuation JSON may contain opaque thought signatures and must not
  be logged, placed in telemetry, or copied into model-visible text.

## Rejected alternatives

**Route Gemini exclusively through OpenRouter.** Rejected: OpenRouter's
normalized surface drops thought signatures, Search grounding, code
execution, context caching, and native audio/video/file input — the
features this decision exists to expose.

**Treat Vertex AI as a fourth protocol with its own crate.** Rejected:
the wire body and stream assembly are identical to the Generative
Language API; only URL construction and the credential header shape
differ. A separate crate would duplicate translation and conformance
work for no protocol difference.

**Let the kernel interpret thought signatures directly.** Rejected:
ADR-040 clause 3 already establishes that continuation state is opaque
provider state the kernel must not decode. Gemini follows the same
rule.

**Represent grounding metadata and code-execution results as synthetic
tool calls.** Rejected: these are provider-executed surfaces, not
caller-invoked tools. They surface as `ContentBlock::Opaque`, the same
precedent used for Anthropic thinking signatures.

**Implement `cachedContents` CRUD, Files API upload, or the Live API in
this leaf.** Rejected for v1: context caching is pass-through and
accounting only; the remaining surfaces are host concerns outside the
Model port's scope.

**Mint or refresh Vertex OAuth tokens inside the crate.** Rejected:
ADR-048 established that credential material and its refresh lifecycle
belong to the host-supplied `CredentialStore`, not to a provider leaf.

## Compatibility and schema-change classification

Additive public provider-adapter and binding-factory surface: a new
leaf crate, a new `Agent.gemini` factory, and a new
`"gemini_generate_content"` arm on `Agent::gateway`. No existing public
type or factory changes shape. `ContentBlock::Opaque` and
`Usage::extension_counters` are pre-existing extension points; new
media types and counter keys under them are additive. Continuation
state stays opaque `RawJson` already present on `ModelResponse`.
Journal record kinds are unchanged. Workspace crate/wheel/npm version
fields are not published by this ADR.

## Security classification

- References: SEC-INV-005; TM-04
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: thought signatures and continuation
  blobs are secret-bearing provider state. They must not appear in
  logs, observer payloads, or generated fixtures; fixtures use
  fabricated signatures. HTTPS is required whenever a credential or
  secret header is present; keyless loopback HTTP is the only
  plaintext path. Vertex bearer tokens are host-supplied via
  `CredentialStore` and are never minted or refreshed by the crate. No
  new kernel port or trust boundary is added.

## Affected requirements, design, and delivery

- Affected requirements: FR-MDL
- Affected Technical Design: Technical Design §14 (reference provider)
  and workspace layout §2
- Architecture Specification §6.1 and §25
- Design: [Gemini Native Provider — Design](../../superpowers/specs/2026-08-20-gemini-provider-design.md)
- Implementation Plan: [Gemini Native Provider Implementation Plan](../../superpowers/plans/2026-08-20-gemini-provider.md)
- Related decisions: ADR-040 (native leaf-per-protocol precedent and
  opaque continuation state), ADR-045 (frozen `Agent.<provider>`
  constructor naming), ADR-047 (`Agent::gateway` stays a thin
  dispatcher), ADR-048 (shared `CredentialStore`/`SecretString`
  boundary), ADR-049 (`MediaResolver` contract for blob resolution)

## Supersession metadata

This ADR supersedes no prior ADR. It is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of
every affected primary planning document. Reopening a Chat-Completions
style compatibility path for Gemini, promoting Vertex to a separate
protocol/crate, or implementing `cachedContents` CRUD, Files API
upload, or the Live API requires that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to execute the native
  Gemini `generateContent` leaf as designed
- Implementation evidence: Missing (decision record only; leaf crate
  implementation is later tasks in the same plan; no evidence id)
