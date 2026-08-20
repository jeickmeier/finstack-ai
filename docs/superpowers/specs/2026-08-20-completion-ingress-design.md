# External-Completion Ingress (`finstack-ai-completion-ingress`) — Design

**Date:** 2026-08-20
**Status:** Approved design, pre-implementation
**Path:** `extensions/interop/finstack-ai-completion-ingress`

## 1. Problem

The runtime already classifies authenticated external completions:
`ExternalCompletionRouter::route` (crates/finstack-ai-runtime/src/driver/ingress/completion.rs:70)
validates locator identity, authorization, target existence, expiry, and
duplicate/conflict semantics, emits `SecurityAuditGate` events, and returns
`ExternalRouteOutcome::{Committed, Idempotent, Rejected}` / the opaque
`ExternalRouteError::IngressRejected` (types.rs:9-45). But its doc comment is
explicit: *"Route one fully authenticated command without accepting raw
callback tokens."* Nothing in `extensions/` performs the step the Technical
Design assigns to the ingress adapter (docs/planning/03-finstack-ai-technical-design.md:2512):

> The ingress adapter authenticates the caller and binds any signed opaque
> callback token to the complete locator before invoking the runtime router.

That adapter is the wakeup path for every deferred job — background provider
responses, deferred tool effects, E2B sandbox jobs, and workflow webhooks
(future-capabilities §3.3, §10.4). This crate is that adapter. It is **also
the workflow webhook**: the workflow driver/worker mints and delivers through
the same crate; no second crate is designed.

## 2. Decisions

1. **One crate, host-embedded, transport-agnostic.** The leaf exposes
   `mint` (token issuance at deferral time) and `deliver` (token + body +
   `submitted_at` → routed completion). It does **not** listen on a socket
   and adds **no HTTP dependency** — the workspace has none (no axum/hyper;
   `finstack-ai-server` is a CBOR-framed reference server that never touches
   the ingress routers). Whatever terminates HTTP/TCP for the host calls
   `deliver`. This is the same posture as `finstack-ai-remote-child`:
   explicit construction, no discovery, no environment reads.
2. **Trust level T4.** Callers are remote principals — "untrusted until
   authenticated and authorized for a precisely scoped action"
   (06-security-threat-model.md:114; docs/site/security-trust-levels.md:13).
   Stricter than remote-child's T1. Not compiled into `wasm-host`.
3. **Token = signed, not encrypted.** Claims are the durable locator plus
   principal/authorization evidence and expiry; the architecture already
   declares the external handle non-secret (02-architecture-spec.md:718).
   HMAC-SHA256 with workspace `hmac` + `sha2`; base64url via workspace
   `base64`; canonical claim bytes via workspace `serde_json_canonicalizer`.
   No new dependencies.
4. **Authorization evidence is frozen at mint time.** The token embeds the
   `PrincipalRef` and `AuthorizationEvidence` captured from the accepted run
   when the deferral was dispatched. `route` then enforces
   `authorization_matches` against the recovered run state
   (shared.rs:85-96), so a token minted for one run can never complete
   another, and a policy re-decision that changes evidence invalidates
   outstanding tokens — fail closed.
5. **Fail closed on conflict is delegated, not reimplemented.** Duplicate
   with equal digest → `Idempotent`; conflicting duplicate → the router's
   durable `ExternalCommandRejected` evidence and
   `Rejected { reason_code: "conflicting_or_invalid_completion" }`
   (completion.rs:218-242). The leaf maps outcomes 1:1 and never converts a
   rejection into success.
6. **One opaque failure for every pre-router rejection.** Malformed token,
   bad signature, unknown key id, expired token, wrong kind, oversize or
   undecodable body — all audit first, then return the single
   `IngressError::Rejected`, revealing nothing about target existence,
   mirroring `ExternalRouteError::IngressRejected` (tests.rs:286-350).
   An audit-write failure also rejects (completion.rs:267-271 pattern).
7. **Leaf owns the two unused audit categories.** The runtime router only
   emits `UnknownLocator` and `ScopeMismatch`; `MalformedToken` and
   `AuthenticationFailure` (services/audit.rs:12-25) exist precisely for
   this adapter and are emitted only here.
8. **Interaction resolutions are out of scope** (v1). The token claim set
   carries `kind: "effect_completion"`; a future version can add an
   interaction kind and drive `InteractionRouter` without a format break.

## 3. Trust and threat mapping (TM-10)

| TM-10 control | Owner |
|---|---|
| Authenticated router | leaf: HMAC verify + kid lookup; runtime: `authorization_matches` |
| effect/session/tenant scope | runtime: completion.rs:109-132 identity check |
| original `EffectId` | token claim + runtime `known_effect` (shared.rs:58) |
| completion identity + normalized digest | `ExternalEffectCompletion.completion_id` + router digests |
| expiry/cancellation | leaf: token `expires_at` vs `submitted_at`; runtime: `IdempotencyHorizon` + kernel cancellation |
| idempotent duplicate | runtime: `ExternalRouteOutcome::Idempotent` |
| fail-closed conflict | runtime: durable `ExternalCommandRejected` |

TDD constraint (03-technical-design.md:3925): the application-configured
idempotency horizon must be **at least as long as callback-token validity**.
The leaf cannot enforce this globally; the README and rustdoc state it, and
`deliver` passes any configured horizon to `with_horizon` so post-horizon
tokens are `expired_locator` regardless of their own expiry.

ENG-SEM-005 and SEC-INV-003 are satisfied by delegation to the router.
Landing this crate triggers a Threat Model §18 review entry for TM-10
(and touches TM-14/TM-19 by reference).

## 4. Public surface

Kept small (remote-child's baseline is 12 lines; target similar).

```rust
pub use config::{CompletionIngressConfig, CompletionIngressConfigError};
pub use ingress::{CompletionGrant, CompletionIngress, IngressError, MintError};
pub use token::CallbackToken;
// plus re-export for callers: pub use finstack_ai_runtime::ExternalRouteOutcome;  (NOT re-exported; callers import from runtime)
```

```rust
/// Explicit signing configuration. Never reads environment variables.
#[derive(Clone)]
pub struct CompletionIngressConfig {
    /// Active signing key id (label rules; stamped into minted tokens).
    pub key_id: String,
    /// Active signing key (>= 32 bytes after `expose()`).
    pub key: SecretString,
    /// Additional accepted verification keys for rotation: (key_id, key).
    pub additional_verification_keys: Vec<(String, SecretString)>,
}

/// What a minted token authorizes: one effect on one run, until expiry.
#[derive(Clone)]
pub struct CompletionGrant {
    pub locator: OperationLocator,
    pub principal: PrincipalRef,
    pub authorization: AuthorizationEvidence,
    pub effect_id: EffectId,
    pub expires_at: Timestamp,
}

/// Opaque signed callback token (`fcit1.<claims>.<tag>`).
#[derive(Clone, PartialEq, Eq)]
pub struct CallbackToken(/* private String */);
impl CallbackToken { pub fn as_str(&self) -> &str; }

pub struct CompletionIngress { /* store, audit, config-derived keys, horizon */ }
impl CompletionIngress {
    pub fn try_new(
        store: Arc<dyn JournalStore>,
        audit: Arc<SecurityAuditGate>,
        config: CompletionIngressConfig,
    ) -> Result<Self, CompletionIngressConfigError>;
    #[must_use]
    pub fn with_horizon(self, horizon: IdempotencyHorizon) -> Self;
    pub fn mint(&self, grant: &CompletionGrant) -> Result<CallbackToken, MintError>;
    pub async fn deliver(
        &self,
        token: &str,
        body: &[u8],
        submitted_at: Timestamp,
    ) -> Result<ExternalRouteOutcome, IngressError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IngressError {
    /// One non-existence-revealing response for every token/body/audit failure.
    #[error("external completion rejected")]
    Rejected,
    /// Store/commit/id-allocation failure on a known authorized target.
    #[error("external completion ingress unavailable: {reason_code}")]
    Unavailable { reason_code: &'static str },
}
```

`deliver` returns the runtime's `ExternalRouteOutcome` unmapped — hosts map
`Committed`/`Idempotent` to success, `Rejected { reason_code }` to a
conflict-class response, `IngressError::Rejected` to one uniform opaque
status (e.g. HTTP 404), and `Unavailable` to a retryable 5xx.

## 5. Token format

```
fcit1 . base64url_nopad(claims_json) . base64url_nopad(hmac_sha256(key, "fcit1." + claims_b64))
```

- Claims struct (serde, strict wire decode reusing kernel deserializers —
  every field type already has bounded `deny_unknown_fields` deserialization):

```rust
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    v: u32,                       // must equal 1
    kid: String,                  // key id, label rules
    kind: String,                 // must equal "effect_completion"
    locator: OperationLocator,
    principal: PrincipalRef,
    authorization: AuthorizationEvidence,
    effect_id: EffectId,
    expires_at: Timestamp,
}
```

- Mint serializes claims with `serde_json_canonicalizer::to_vec` (JCS).
- Verify MACs the **received** `"fcit1." + claims_b64` bytes as-is (no
  re-canonicalization), compares with `Mac::verify_slice` (constant time),
  then strictly decodes claims, then checks `v`, `kind`, `kid` known, and
  `expires_at > submitted_at`.
- Bounds (consts): `MAX_TOKEN_BYTES = 4_096`, `MAX_BODY_BYTES = 1_048_576`,
  `MIN_KEY_BYTES = 32`.

## 6. Delivery body

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryBody {
    #[serde(default)]
    completion_id: Option<String>,     // label rules; default = effect_id canonical string
    outcome: ExternalEffectOutcome,    // kernel strict bounded deserializer
}
```

Defaulting `completion_id` to `effect_id.to_canonical_string()` makes plain
retries idempotent and a second, different result a fail-closed conflict —
the same convention as the in-process deferred bridge
(crates/finstack-ai/src/agent/deferred/bridge.rs:266-301). Callers with
provider-native delivery ids may supply their own.

## 7. Delivery pipeline (order is normative)

1. `token.len() > MAX_TOKEN_BYTES` or shape/prefix/base64 failure →
   audit `MalformedToken` / `"malformed_token"` → `Rejected`.
2. Unknown `kid` → `AuthenticationFailure` / `"unknown_key"`; MAC mismatch →
   `AuthenticationFailure` / `"bad_signature"`; claims decode or `v`/`kind`
   mismatch → `MalformedToken` / `"malformed_token"`; `expires_at <=
   submitted_at` → `AuthenticationFailure` / `"expired_token"`. All →
   `Rejected`.
3. `body.len() > MAX_BODY_BYTES` → `MalformedToken` / `"oversize_body"`
   (principal now known, included in the event) → `Rejected`.
4. Strict body decode failure, or `ExternalEffectCompletion::try_new` /
   `ExternalEffectCompletionCommand::try_new` failure →
   `MalformedToken` / `"invalid_body"` → `Rejected`.
5. `ExternalCompletionRouter::new(store, audit)` (+ `with_horizon` when
   configured) `.route(command, submitted_at)`.
6. Error mapping: `IngressRejected` → `Rejected`; `IdAllocation` →
   `Unavailable { "id_allocation" }`; `Runtime(_)` →
   `Unavailable { "runtime" }`; `InvalidNormalizedCommand` → `Rejected`.
7. Any leaf audit-write failure → `Rejected` (fail closed; no distinct
   signal).

Audit events mirror the runtime's derivation (ingress/shared.rs:109-133):
event id = hex of `Digest::domain_separated("security-audit-event", 1,
canonical_json((token_digest, body_digest, submitted_at, reason_code)))`,
where `token_digest`/`body_digest` are domain-separated digests of the raw
received bytes under leaf domains `"completion-ingress-token"` /
`"completion-ingress-body"` (version 1). Identical replayed garbage
therefore produces one idempotent audit event.

## 8. Composition (who calls what)

- **Mint side:** the component dispatching a deferred job (workflow driver,
  provider adapter host, E2B toolset host) builds a `CompletionGrant` from
  the accepted run's locator/security context and the `EffectDeferred`'s
  `effect_id`/`expires_at`, and hands `CallbackToken::as_str()` to the
  external system as (part of) its callback URL or job metadata.
- **Deliver side:** the host's HTTP terminator (or the workflow worker's
  inbox, or a polling reconciler that fetched the provider result) calls
  `deliver(token, body, now)`. The workflow webhook is exactly this call.
- Construction requires a real `SecurityAuditGate` (`SecurityAuditGate::
  enable(sink, deadline)`); there is deliberately **no** `trusted()`
  constructor on this crate — a T4 surface never runs with a no-op audit.

## 9. Explicit exclusions

- No HTTP/TCP listener, no TLS, no framing — host-owned.
- No interaction resolution (`InteractionRouter`) — future token kind.
- No polling/reconciliation loop and no durable inbox — that is the
  workflow-worker design (docs/superpowers/specs/2026-08-19-workflow-worker-design.md §7),
  which becomes a *caller* of this crate.
- No SDK facade mirror (`finstack-ai` constructor) in v1; hosts construct
  the leaf directly. ADR-045 wiring can follow as an additive change.
- No environment variable reads, no peer discovery, no key generation.

## 10. Testing

Unit (inline per module): config validation and Debug redaction; token
mint/verify round-trip, tamper (flip one claims byte, one tag byte),
truncation, oversize, wrong prefix/version/kind, unknown kid, rotation
(verify with retired key succeeds, sign always with active), expiry
boundary (`expires_at == submitted_at` rejects).

Integration (`tests/deliver.rs`, dev-deps `finstack-ai` + `finstack-ai-test`
+ `finstack-ai-store-memory`, mirroring
crates/finstack-ai-test/tests/deferred_bridge/): drive a real agent to
`AwaitingExternal` with a `ScriptedToolset` deferral, mint a token, then
assert — Committed on first delivery; Idempotent on byte-identical replay;
Rejected(conflict) on a different outcome; opaque `Rejected` for a token
minted for a nonexistent session (same error object as a garbage token, and
exactly one audit event per identical replay, via a recording sink);
`expired_locator` behavior through `with_horizon`.

## 11. Repo integration checklist

- Root `Cargo.toml`: `members` += `extensions/interop/finstack-ai-completion-ingress`
  (next to remote-child, line ~41) and `[workspace.dependencies]` entry
  pinned `version = "1.0.0"`.
- `scripts/wasm_package/check.py:19`:
  add `"finstack-ai-completion-ingress"` to `FORBIDDEN_WASM`.
- Public-api baseline: `scripts/compat/public_api.py --write` generates
  `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-completion-ingress.txt`
  (crate auto-discovered; `--check` fails until the baseline exists).
- README stating: never reads environment variables, never discovers peers,
  T4 native adapter, not compiled into `wasm-host`, horizon ≥ token
  validity, one construction example.
- CHANGELOG entry.
