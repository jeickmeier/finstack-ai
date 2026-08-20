# External-Completion Ingress Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A production extension leaf `finstack-ai-completion-ingress` that mints signed opaque callback tokens at deferral time and, on delivery, binds a token + outcome body to an authenticated `ExternalEffectCompletionCommand` routed through the runtime's `ExternalCompletionRouter` — the wakeup path for deferred model, tool, E2B, and workflow-webhook jobs.

**Architecture:** Host-embedded, transport-agnostic service in `extensions/interop/` (same trust posture as `finstack-ai-remote-child`: explicit construction, no discovery, no env reads, fail closed). The leaf owns callback-token mint/verify (HMAC-SHA256 over JCS-canonical claims), bounded body decode, and the `MalformedToken`/`AuthenticationFailure` audit categories; everything after command construction is delegated to `ExternalCompletionRouter::route`. Trust level T4; forbidden in `wasm-host`.

**Tech Stack:** Rust; workspace deps only: `hmac` 0.12, `sha2`, `base64` (URL_SAFE_NO_PAD), `serde`, `serde_json`, `serde_json_canonicalizer`, `thiserror`, `finstack-ai-kernel`, `finstack-ai-runtime` (native-tokio).

**Spec:** `docs/superpowers/specs/2026-08-20-completion-ingress-design.md`

## Global Constraints

- Crate path/name: `extensions/interop/finstack-ai-completion-ingress`, package `finstack-ai-completion-ingress`, workspace `version = "1.0.0"`.
- lib.rs lint prelude copied byte-for-byte from `extensions/interop/finstack-ai-remote-child/src/lib.rs` (`#![warn(missing_docs)]`, `#![forbid(unsafe_code)]`, `deny(clippy::unwrap_used/expect_used/panic/unreachable)`, test-mode allowances, doc-test allow).
- Every public item documented (`missing_docs` warns; workspace lints deny on CI).
- Consts: `MAX_TOKEN_BYTES = 4_096`, `MAX_BODY_BYTES = 1_048_576`, `MIN_KEY_BYTES = 32`, token prefix `"fcit1."`, claim `v = 1`, claim `kind = "effect_completion"`.
- One opaque `IngressError::Rejected` for every token/body/audit failure; never reveal target existence; audit before responding; audit-write failure also rejects.
- No `trusted()`/no-op-audit constructor. No environment reads. No HTTP dependency.
- Public-api freeze gate: any change to public names requires regenerating `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-completion-ingress.txt` in the same change (Task 7).
- Run tests with `cargo test -p finstack-ai-completion-ingress` and check exit codes explicitly (do not rely on rtk summarizers for gating).

---

### Task 1: Crate scaffold, workspace wiring, and `CompletionIngressConfig`

**Files:**
- Modify: `Cargo.toml` (root — two edits: `members` list, insert `"extensions/interop/finstack-ai-completion-ingress",` directly next to the existing `"extensions/interop/finstack-ai-remote-child",` entry at line ~41; and `[workspace.dependencies]` at line ~149, add `finstack-ai-completion-ingress = { path = "extensions/interop/finstack-ai-completion-ingress", version = "1.0.0" }`)
- Modify: `scripts/wasm_package/check.py:19` (add `"finstack-ai-completion-ingress",` to `FORBIDDEN_WASM`)
- Create: `extensions/interop/finstack-ai-completion-ingress/Cargo.toml`
- Create: `extensions/interop/finstack-ai-completion-ingress/src/lib.rs`
- Create: `extensions/interop/finstack-ai-completion-ingress/src/config.rs`
- Create: `extensions/interop/finstack-ai-completion-ingress/README.md`

**Interfaces:**
- Produces: `CompletionIngressConfig { key_id: String, key: SecretString, additional_verification_keys: Vec<(String, SecretString)> }` (Clone; Debug redacts keys), `CompletionIngressConfigError` enum, `pub(crate) fn validated_keys(config: &CompletionIngressConfig) -> Result<ResolvedKeys, CompletionIngressConfigError>` where `pub(crate) struct ResolvedKeys { active_id: Arc<str>, keys: Vec<(Arc<str>, SecretString)> }` (active key first, all keys verifiable), consts `MIN_KEY_BYTES`.

- [ ] **Step 1: Crate manifest and workspace wiring**

`extensions/interop/finstack-ai-completion-ingress/Cargo.toml`:

```toml
[package]
name = "finstack-ai-completion-ingress"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Signed callback-token ingress for authenticated external effect completions"
readme = "README.md"

[dependencies]
base64 = { workspace = true }
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
hmac = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
serde_json_canonicalizer = { workspace = true }
sha2 = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
finstack-ai = { workspace = true, default-features = false, features = ["native-tokio"] }
finstack-ai-store-memory = { workspace = true }
finstack-ai-test = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros", "time"] }

[lints]
workspace = true
```

Make both root `Cargo.toml` edits and both `FORBIDDEN_WASM` edits listed above. If `serde`, `serde_json`, `hmac`, `sha2`, `base64`, or `serde_json_canonicalizer` lack `workspace = true` entries usable from extensions, use the exact versions already pinned in the root `[workspace.dependencies]` (they exist at root Cargo.toml lines 177, 178, 181, 186).

`src/lib.rs` (prelude copied from remote-child, then):

```rust
//! Signed callback-token ingress for authenticated external effect completions.

mod config;

pub use config::{CompletionIngressConfig, CompletionIngressConfigError};
```

`README.md` (extend in Task 7; initial content):

```markdown
# finstack-ai-completion-ingress

Host-embedded ingress that mints signed opaque callback tokens for deferred
effects and delivers authenticated external completions to the runtime's
`ExternalCompletionRouter`. This is the wakeup path for deferred model,
tool, and sandbox jobs, and it is the workflow webhook.

This crate never reads environment variables and never discovers peers.
Callers are T4 remote principals; every token, signature, or body failure
is audited and answered with one non-existence-revealing rejection. The
configured idempotency horizon must be at least as long as callback-token
validity. This crate is a T4 native adapter. It is not compiled into
`wasm-host`.
```

- [ ] **Step 2: Write the failing config tests** (bottom of `src/config.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_runtime::SecretString;

    fn key(byte: u8) -> SecretString {
        SecretString::try_new(String::from_utf8(vec![byte; 32]).expect("ascii")).expect("secret")
    }

    #[test]
    fn accepts_active_key_and_rotation_keys() {
        let config = CompletionIngressConfig {
            key_id: "k-active".to_owned(),
            key: key(b'a'),
            additional_verification_keys: vec![("k-old".to_owned(), key(b'b'))],
        };
        let resolved = validated_keys(&config).expect("valid");
        assert_eq!(resolved.active_id.as_ref(), "k-active");
        assert_eq!(resolved.keys.len(), 2);
        assert_eq!(resolved.keys[0].0.as_ref(), "k-active");
    }

    #[test]
    fn rejects_short_key_bad_id_and_duplicate_id() {
        let short = CompletionIngressConfig {
            key_id: "k1".to_owned(),
            key: SecretString::try_new("tooshort").expect("secret"),
            additional_verification_keys: vec![],
        };
        assert_eq!(validated_keys(&short).expect_err("short"), CompletionIngressConfigError::KeyTooShort);
        let bad_id = CompletionIngressConfig {
            key_id: String::new(),
            key: key(b'a'),
            additional_verification_keys: vec![],
        };
        assert_eq!(validated_keys(&bad_id).expect_err("id"), CompletionIngressConfigError::InvalidKeyId);
        let dup = CompletionIngressConfig {
            key_id: "k1".to_owned(),
            key: key(b'a'),
            additional_verification_keys: vec![("k1".to_owned(), key(b'b'))],
        };
        assert_eq!(validated_keys(&dup).expect_err("dup"), CompletionIngressConfigError::DuplicateKeyId);
    }

    #[test]
    fn debug_never_prints_key_material() {
        let config = CompletionIngressConfig {
            key_id: "k1".to_owned(),
            key: key(b'a'),
            additional_verification_keys: vec![],
        };
        let printed = format!("{config:?}");
        assert!(!printed.contains('a'.to_string().repeat(32).as_str()));
        assert!(printed.contains("REDACTED"));
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p finstack-ai-completion-ingress` — expected: compile error (`CompletionIngressConfig` not defined).

- [ ] **Step 4: Implement `src/config.rs`**

```rust
//! Explicit signing configuration for the completion ingress.

use std::fmt;
use std::sync::Arc;

use finstack_ai_runtime::SecretString;
use thiserror::Error;

/// Minimum accepted signing-key length in bytes.
pub const MIN_KEY_BYTES: usize = 32;

const KEY_ID_MAX_BYTES: usize = 128;

/// Explicit signing configuration. Never read from the environment.
#[derive(Clone)]
pub struct CompletionIngressConfig {
    /// Active signing key id, stamped into minted tokens.
    pub key_id: String,
    /// Active signing key (at least [`MIN_KEY_BYTES`] bytes).
    pub key: SecretString,
    /// Additional accepted verification keys for rotation.
    pub additional_verification_keys: Vec<(String, SecretString)>,
}

impl fmt::Debug for CompletionIngressConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompletionIngressConfig")
            .field("key_id", &self.key_id)
            .field("key", &"[REDACTED]")
            .field(
                "additional_verification_keys",
                &self
                    .additional_verification_keys
                    .iter()
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Why a [`CompletionIngressConfig`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CompletionIngressConfigError {
    /// A key id was empty, oversize, or contained NUL/non-ASCII bytes.
    #[error("invalid key id")]
    InvalidKeyId,
    /// A key was shorter than [`MIN_KEY_BYTES`].
    #[error("signing key shorter than the 32-byte minimum")]
    KeyTooShort,
    /// Two keys shared one id.
    #[error("duplicate key id")]
    DuplicateKeyId,
}

pub(crate) struct ResolvedKeys {
    pub(crate) active_id: Arc<str>,
    /// Active key first; all entries accepted for verification.
    pub(crate) keys: Vec<(Arc<str>, SecretString)>,
}

fn valid_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= KEY_ID_MAX_BYTES
        && id.is_ascii()
        && !id.bytes().any(|byte| byte == 0)
}

pub(crate) fn validated_keys(
    config: &CompletionIngressConfig,
) -> Result<ResolvedKeys, CompletionIngressConfigError> {
    let mut keys: Vec<(Arc<str>, SecretString)> = Vec::new();
    let mut push = |id: &str, key: &SecretString| {
        if !valid_key_id(id) {
            return Err(CompletionIngressConfigError::InvalidKeyId);
        }
        if key.expose().len() < MIN_KEY_BYTES {
            return Err(CompletionIngressConfigError::KeyTooShort);
        }
        if keys.iter().any(|(existing, _)| existing.as_ref() == id) {
            return Err(CompletionIngressConfigError::DuplicateKeyId);
        }
        keys.push((Arc::from(id), key.clone()));
        Ok(())
    };
    push(&config.key_id, &config.key)?;
    for (id, key) in &config.additional_verification_keys {
        push(id, key)?;
    }
    Ok(ResolvedKeys {
        active_id: Arc::from(config.key_id.as_str()),
        keys,
    })
}
```

(If the borrow checker rejects the closure capturing `keys`, rewrite `push` as a plain loop body — behavior over form.)

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p finstack-ai-completion-ingress` — expected: 3 passed. Also run `python3 scripts/wasm_package/check.py` (expect pass) and `cargo check -p finstack-ai-completion-ingress` exit 0.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml scripts/wasm_package/check.py extensions/interop/finstack-ai-completion-ingress
git commit -m "feat(completion-ingress): scaffold crate with validated signing config"
```

---

### Task 2: Callback token mint/verify (`src/token.rs`)

**Files:**
- Create: `extensions/interop/finstack-ai-completion-ingress/src/token.rs`
- Modify: `extensions/interop/finstack-ai-completion-ingress/src/lib.rs` (add `mod token;` and `pub use token::CallbackToken;`)

**Interfaces:**
- Consumes: `ResolvedKeys` from Task 1.
- Produces:
  - `pub struct CallbackToken(String)` with `pub fn as_str(&self) -> &str` (Clone, PartialEq, Eq; Debug prints `CallbackToken([REDACTED])` — the token is a bearer capability).
  - `pub(crate) struct Claims { pub v: u32, pub kid: String, pub kind: String, pub locator: OperationLocator, pub principal: PrincipalRef, pub authorization: AuthorizationEvidence, pub effect_id: EffectId, pub expires_at: Timestamp }` (Serialize + Deserialize, `#[serde(deny_unknown_fields)]`).
  - `pub(crate) fn mint_token(keys: &ResolvedKeys, claims: &Claims) -> Result<CallbackToken, MintErrorKind>` where `pub(crate) enum MintErrorKind { Encoding }`.
  - `pub(crate) fn verify_token(keys: &ResolvedKeys, token: &str, submitted_at: Timestamp) -> Result<Claims, VerifyFailure>` where `pub(crate) enum VerifyFailure { Malformed, UnknownKey, BadSignature, Expired }` — this enum drives the audit reason code and is never shown to callers.
  - Consts `pub(crate) const MAX_TOKEN_BYTES: usize = 4_096;`, `const TOKEN_PREFIX: &str = "fcit1.";`, `pub(crate) const CLAIMS_VERSION: u32 = 1;`, `pub(crate) const KIND_EFFECT_COMPLETION: &str = "effect_completion";`.

- [ ] **Step 1: Write the failing token tests** (bottom of `src/token.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CompletionIngressConfig, validated_keys};
    use finstack_ai_kernel::{
        AuthorizationEvidence, EffectId, LaneId, OperationLocator, PrincipalRef, RunId, SessionId,
        Timestamp,
    };
    use finstack_ai_runtime::SecretString;

    fn keys() -> crate::config::ResolvedKeys {
        validated_keys(&CompletionIngressConfig {
            key_id: "k-active".to_owned(),
            key: SecretString::try_new("a".repeat(32)).expect("secret"),
            additional_verification_keys: vec![(
                "k-old".to_owned(),
                SecretString::try_new("b".repeat(32)).expect("secret"),
            )],
        })
        .expect("keys")
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    fn claims(kid: &str, expires_at: Timestamp) -> Claims {
        Claims {
            v: CLAIMS_VERSION,
            kid: kid.to_owned(),
            kind: KIND_EFFECT_COMPLETION.to_owned(),
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::parse("01234567-89ab-7cde-89ab-0123456789a1").expect("session"),
                LaneId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("lane"),
                RunId::parse("01234567-89ab-7cde-89ab-0123456789a3").expect("run"),
            )
            .expect("locator"),
            principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                .expect("principal"),
            authorization: AuthorizationEvidence::try_new("policy-v1", "decision-v1")
                .expect("auth"),
            effect_id: EffectId::parse("01234567-89ab-7cde-89ab-0123456789a4").expect("effect"),
            expires_at,
        }
    }

    #[test]
    fn mint_then_verify_round_trips() {
        let keys = keys();
        let minted = mint_token(&keys, &claims("k-active", ts(10_000))).expect("mint");
        let verified = verify_token(&keys, minted.as_str(), ts(9_999)).expect("verify");
        assert_eq!(verified.kid, "k-active");
        assert_eq!(verified.effect_id, claims("k-active", ts(10_000)).effect_id);
    }

    #[test]
    fn expiry_boundary_rejects_at_and_after_expiry() {
        let keys = keys();
        let minted = mint_token(&keys, &claims("k-active", ts(10_000))).expect("mint");
        assert_eq!(
            verify_token(&keys, minted.as_str(), ts(10_000)).expect_err("at expiry"),
            VerifyFailure::Expired
        );
        assert_eq!(
            verify_token(&keys, minted.as_str(), ts(10_001)).expect_err("after expiry"),
            VerifyFailure::Expired
        );
    }

    #[test]
    fn tampered_claims_and_tag_fail_signature() {
        let keys = keys();
        let minted = mint_token(&keys, &claims("k-active", ts(10_000))).expect("mint");
        let token = minted.as_str();
        // Flip one character inside the claims segment.
        let mut chars: Vec<char> = token.chars().collect();
        let dot = token.rfind('.').expect("dot");
        chars[dot - 1] = if chars[dot - 1] == 'A' { 'B' } else { 'A' };
        let tampered_claims: String = chars.iter().collect();
        assert!(matches!(
            verify_token(&keys, &tampered_claims, ts(0)).expect_err("claims tamper"),
            VerifyFailure::BadSignature | VerifyFailure::Malformed
        ));
        // Flip the final tag character.
        let mut tag_chars: Vec<char> = token.chars().collect();
        let last = tag_chars.len() - 1;
        tag_chars[last] = if tag_chars[last] == 'A' { 'B' } else { 'A' };
        let tampered_tag: String = tag_chars.iter().collect();
        assert_eq!(
            verify_token(&keys, &tampered_tag, ts(0)).expect_err("tag tamper"),
            VerifyFailure::BadSignature
        );
    }

    #[test]
    fn shape_failures_are_malformed() {
        let keys = keys();
        for bad in [
            "",
            "fcit1.",
            "fcit1.onlyone",
            "not-a-token",
            "fcit2.AA.BB",
            "fcit1.!!.!!",
        ] {
            assert_eq!(
                verify_token(&keys, bad, ts(0)).expect_err("malformed"),
                VerifyFailure::Malformed,
                "input: {bad}"
            );
        }
        let oversize = format!("fcit1.{}.AA", "A".repeat(MAX_TOKEN_BYTES));
        assert_eq!(
            verify_token(&keys, &oversize, ts(0)).expect_err("oversize"),
            VerifyFailure::Malformed
        );
    }

    #[test]
    fn unknown_kid_rejects_and_rotation_key_verifies() {
        let keys = keys();
        let old_signed = {
            // Sign with the rotation key by making it active in a second key set.
            let alt = validated_keys(&CompletionIngressConfig {
                key_id: "k-old".to_owned(),
                key: SecretString::try_new("b".repeat(32)).expect("secret"),
                additional_verification_keys: vec![],
            })
            .expect("alt");
            mint_token(&alt, &claims("k-old", ts(10_000))).expect("mint")
        };
        assert!(verify_token(&keys, old_signed.as_str(), ts(0)).is_ok());
        let stranger = {
            let alt = validated_keys(&CompletionIngressConfig {
                key_id: "k-stranger".to_owned(),
                key: SecretString::try_new("c".repeat(32)).expect("secret"),
                additional_verification_keys: vec![],
            })
            .expect("alt");
            mint_token(&alt, &claims("k-stranger", ts(10_000))).expect("mint")
        };
        assert_eq!(
            verify_token(&keys, stranger.as_str(), ts(0)).expect_err("unknown kid"),
            VerifyFailure::UnknownKey
        );
    }

    #[test]
    fn wrong_kind_or_version_is_malformed() {
        let keys = keys();
        let mut wrong_kind = claims("k-active", ts(10_000));
        wrong_kind.kind = "interaction_resolution".to_owned();
        let minted = mint_token(&keys, &wrong_kind).expect("mint");
        assert_eq!(
            verify_token(&keys, minted.as_str(), ts(0)).expect_err("kind"),
            VerifyFailure::Malformed
        );
        let mut wrong_version = claims("k-active", ts(10_000));
        wrong_version.v = 2;
        let minted = mint_token(&keys, &wrong_version).expect("mint");
        assert_eq!(
            verify_token(&keys, minted.as_str(), ts(0)).expect_err("version"),
            VerifyFailure::Malformed
        );
    }
}
```

(If `SessionId`/`LaneId`/`RunId`/`EffectId` do not expose `parse` at these exact paths, use the same constructors the crash-prefix helpers use — see `crates/finstack-ai-test/tests/crash_prefix/helpers/mod.rs` `id::<T>()` and `crates/finstack-ai-test/tests/deferred_bridge/helpers/mod.rs`; `RunId::parse("01234567-...")` is used verbatim at deferred_bridge/helpers/mod.rs:401.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-completion-ingress token` — expected: compile error (`mint_token` not defined).

- [ ] **Step 3: Implement `src/token.rs`**

```rust
//! Signed opaque callback tokens binding one deferred effect to one locator.

use core::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use finstack_ai_kernel::{
    AuthorizationEvidence, EffectId, OperationLocator, PrincipalRef, Timestamp,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::ResolvedKeys;

type HmacSha256 = Hmac<Sha256>;

/// Maximum accepted token length in bytes.
pub(crate) const MAX_TOKEN_BYTES: usize = 4_096;
const TOKEN_PREFIX: &str = "fcit1.";
pub(crate) const CLAIMS_VERSION: u32 = 1;
pub(crate) const KIND_EFFECT_COMPLETION: &str = "effect_completion";

/// Opaque signed callback token. Treat as a bearer capability.
#[derive(Clone, PartialEq, Eq)]
pub struct CallbackToken(String);

impl CallbackToken {
    /// Borrow the encoded token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CallbackToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CallbackToken([REDACTED])")
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Claims {
    pub(crate) v: u32,
    pub(crate) kid: String,
    pub(crate) kind: String,
    pub(crate) locator: OperationLocator,
    pub(crate) principal: PrincipalRef,
    pub(crate) authorization: AuthorizationEvidence,
    pub(crate) effect_id: EffectId,
    pub(crate) expires_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MintErrorKind {
    Encoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifyFailure {
    Malformed,
    UnknownKey,
    BadSignature,
    Expired,
}

fn mac_for(key: &str, message: &[u8]) -> Result<HmacSha256, ()> {
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|_| ())?;
    mac.update(message);
    Ok(mac)
}

pub(crate) fn mint_token(
    keys: &ResolvedKeys,
    claims: &Claims,
) -> Result<CallbackToken, MintErrorKind> {
    let claims_bytes =
        serde_json_canonicalizer::to_vec(claims).map_err(|_| MintErrorKind::Encoding)?;
    let claims_b64 = URL_SAFE_NO_PAD.encode(claims_bytes);
    let message = format!("{TOKEN_PREFIX}{claims_b64}");
    let (_, key) = keys
        .keys
        .first()
        .ok_or(MintErrorKind::Encoding)?;
    let mac = mac_for(key.expose(), message.as_bytes()).map_err(|_| MintErrorKind::Encoding)?;
    let tag = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(CallbackToken(format!("{message}.{tag}")))
}

pub(crate) fn verify_token(
    keys: &ResolvedKeys,
    token: &str,
    submitted_at: Timestamp,
) -> Result<Claims, VerifyFailure> {
    if token.len() > MAX_TOKEN_BYTES || !token.is_ascii() {
        return Err(VerifyFailure::Malformed);
    }
    let rest = token.strip_prefix(TOKEN_PREFIX).ok_or(VerifyFailure::Malformed)?;
    let (claims_b64, tag_b64) = rest.split_once('.').ok_or(VerifyFailure::Malformed)?;
    if claims_b64.is_empty() || tag_b64.is_empty() || claims_b64.contains('.') {
        return Err(VerifyFailure::Malformed);
    }
    let claims_bytes = URL_SAFE_NO_PAD
        .decode(claims_b64)
        .map_err(|_| VerifyFailure::Malformed)?;
    let tag = URL_SAFE_NO_PAD
        .decode(tag_b64)
        .map_err(|_| VerifyFailure::Malformed)?;
    // Read only the kid before authentication; full strict decode happens
    // after the signature is proven.
    #[derive(Deserialize)]
    struct KidOnly {
        kid: String,
        #[serde(flatten)]
        _rest: serde_json::Map<String, serde_json::Value>,
    }
    let kid_probe: KidOnly =
        serde_json::from_slice(&claims_bytes).map_err(|_| VerifyFailure::Malformed)?;
    let key = keys
        .keys
        .iter()
        .find(|(id, _)| id.as_ref() == kid_probe.kid)
        .map(|(_, key)| key)
        .ok_or(VerifyFailure::UnknownKey)?;
    let message = format!("{TOKEN_PREFIX}{claims_b64}");
    let mac = mac_for(key.expose(), message.as_bytes()).map_err(|_| VerifyFailure::BadSignature)?;
    mac.verify_slice(&tag).map_err(|_| VerifyFailure::BadSignature)?;
    let claims: Claims =
        serde_json::from_slice(&claims_bytes).map_err(|_| VerifyFailure::Malformed)?;
    if claims.v != CLAIMS_VERSION
        || claims.kind != KIND_EFFECT_COMPLETION
        || claims.kid != kid_probe.kid
    {
        return Err(VerifyFailure::Malformed);
    }
    if submitted_at >= claims.expires_at {
        return Err(VerifyFailure::Expired);
    }
    Ok(claims)
}
```

Notes for the implementer:
- `OperationLocator` is Serialize + Deserialize with strict bounded wire structs (crates/finstack-ai-kernel/src/records/run/external.rs:17); `PrincipalRef`/`AuthorizationEvidence` likewise (primitives/identity.rs:146, :299). If `OperationLocator` turns out not to implement `Deserialize`, replace the nested field with the four flat fields (`tenant_scope: String`, `session_id: SessionId`, `lane_id: LaneId`, `run_id: RunId`) and rebuild via `OperationLocator::try_new` in Task 4 — keep the wire strict either way.
- If `Timestamp` is not `Ord`, compare via `as_unix_ms()`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-completion-ingress token` — expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add extensions/interop/finstack-ai-completion-ingress/src
git commit -m "feat(completion-ingress): HMAC callback-token mint and strict verify"
```

---

### Task 3: Leaf audit events (`src/audit.rs`)

**Files:**
- Create: `extensions/interop/finstack-ai-completion-ingress/src/audit.rs`
- Modify: `extensions/interop/finstack-ai-completion-ingress/src/lib.rs` (add `mod audit;`)

**Interfaces:**
- Consumes: `SecurityAuditEvent::try_new` (crates/finstack-ai-runtime/src/services/audit.rs:47), `SecurityAuditGate::record` (:240), `Digest::domain_separated`/`to_hex` (kernel).
- Produces:
  - `pub(crate) const TOKEN_DIGEST_DOMAIN: &str = "completion-ingress-token";`
  - `pub(crate) const BODY_DIGEST_DOMAIN: &str = "completion-ingress-body";`
  - `pub(crate) fn ingress_audit_event(category: SecurityAuditCategory, reason_code: &'static str, principal: Option<PrincipalRef>, tenant_scope: Option<&str>, token: &[u8], body: &[u8], submitted_at: Timestamp) -> Option<SecurityAuditEvent>` — returns `None` on any internal failure (caller rejects).
  - `pub(crate) async fn audit_and_reject(gate: &SecurityAuditGate, event: Option<SecurityAuditEvent>) -> crate::ingress::IngressError` — records when `Some`; **always** returns `IngressError::Rejected` (audit failure and audit success both reject; the difference is only whether evidence landed).

- [ ] **Step 1: Write the failing audit tests** (bottom of `src/audit.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::Timestamp;
    use finstack_ai_runtime::SecurityAuditCategory;

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    #[test]
    fn identical_inputs_derive_identical_event_ids() {
        let first = ingress_audit_event(
            SecurityAuditCategory::MalformedToken,
            "malformed_token",
            None,
            None,
            b"garbage-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        let second = ingress_audit_event(
            SecurityAuditCategory::MalformedToken,
            "malformed_token",
            None,
            None,
            b"garbage-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        assert_eq!(first.event_id(), second.event_id());
    }

    #[test]
    fn different_reason_or_bytes_change_the_event_id() {
        let base = ingress_audit_event(
            SecurityAuditCategory::MalformedToken,
            "malformed_token",
            None,
            None,
            b"garbage-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        let other_reason = ingress_audit_event(
            SecurityAuditCategory::AuthenticationFailure,
            "bad_signature",
            None,
            None,
            b"garbage-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        let other_token = ingress_audit_event(
            SecurityAuditCategory::MalformedToken,
            "malformed_token",
            None,
            None,
            b"other-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        assert_ne!(base.event_id(), other_reason.event_id());
        assert_ne!(base.event_id(), other_token.event_id());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-completion-ingress audit` — expected: compile error.

- [ ] **Step 3: Implement `src/audit.rs`**

```rust
//! Redacted audit events for pre-router ingress failures.
//!
//! Mirrors the runtime's derivation (driver/ingress/shared.rs): the event id
//! is the hex of a domain-separated digest over (token digest, body digest,
//! submitted_at, reason_code), so identical replayed garbage produces one
//! idempotent audit event.

use finstack_ai_kernel::{Digest, PrincipalRef, Timestamp};
use finstack_ai_runtime::{SecurityAuditCategory, SecurityAuditEvent, SecurityAuditGate};

use crate::ingress::IngressError;

pub(crate) const TOKEN_DIGEST_DOMAIN: &str = "completion-ingress-token";
pub(crate) const BODY_DIGEST_DOMAIN: &str = "completion-ingress-body";
const SECURITY_AUDIT_EVENT_DOMAIN: &str = "security-audit-event";

fn raw_digest(domain: &'static str, bytes: &[u8]) -> Option<Digest> {
    Digest::domain_separated(domain, 1, bytes).ok()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ingress_audit_event(
    category: SecurityAuditCategory,
    reason_code: &'static str,
    principal: Option<PrincipalRef>,
    tenant_scope: Option<&str>,
    token: &[u8],
    body: &[u8],
    submitted_at: Timestamp,
) -> Option<SecurityAuditEvent> {
    let token_digest = raw_digest(TOKEN_DIGEST_DOMAIN, token)?;
    let body_digest = raw_digest(BODY_DIGEST_DOMAIN, body)?;
    let id_bytes = serde_json_canonicalizer::to_vec(&(
        token_digest,
        body_digest,
        submitted_at,
        reason_code,
    ))
    .ok()?;
    let id_digest = raw_digest(SECURITY_AUDIT_EVENT_DOMAIN, &id_bytes)?;
    SecurityAuditEvent::try_new(
        id_digest.to_hex(),
        submitted_at,
        principal,
        tenant_scope,
        category,
        reason_code,
        Some(token_digest),
        Some(body_digest),
    )
    .ok()
}

/// Record evidence when possible, then reject either way (fail closed).
pub(crate) async fn audit_and_reject(
    gate: &SecurityAuditGate,
    event: Option<SecurityAuditEvent>,
) -> IngressError {
    if let Some(event) = event {
        // A failed audit write must not produce a distinct caller-visible
        // signal; the response is the same opaque rejection.
        let _ = gate.record(event).await;
    }
    IngressError::Rejected
}
```

Note: `SECURITY_AUDIT_EVENT_DOMAIN` reuses the runtime's constant value so audit tooling sees one domain; the leaf's `raw_digest` differs from the runtime's `normalized_digest` in that token/body digests are over raw received bytes (the leaf often cannot parse them — that is why it is auditing). If `Digest::domain_separated` requires a different argument shape than `(domain, 1, &bytes)`, copy the exact call from driver/ingress/shared.rs:104.

This module references `crate::ingress::IngressError` before Task 4 exists; to keep Task 3 independently compilable, add a minimal `src/ingress.rs` now containing only the error enum (Task 4 fills in the rest):

```rust
//! Completion ingress service.

use thiserror::Error;

/// Caller-visible delivery failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IngressError {
    /// One non-existence-revealing response for every token/body/audit failure.
    #[error("external completion rejected")]
    Rejected,
    /// Store/commit/id-allocation failure on a known authorized target.
    #[error("external completion ingress unavailable: {reason_code}")]
    Unavailable {
        /// Stable retryable-failure reason.
        reason_code: &'static str,
    },
}
```

and add `mod ingress;` + `pub use ingress::IngressError;` to `lib.rs`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-completion-ingress audit` — expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add extensions/interop/finstack-ai-completion-ingress/src
git commit -m "feat(completion-ingress): idempotent redacted audit events for ingress failures"
```

---

### Task 4: `CompletionIngress` construction and `mint` (`src/ingress.rs`)

**Files:**
- Modify: `extensions/interop/finstack-ai-completion-ingress/src/ingress.rs`
- Modify: `extensions/interop/finstack-ai-completion-ingress/src/lib.rs` (exports become `pub use ingress::{CompletionGrant, CompletionIngress, IngressError, MintError};`)

**Interfaces:**
- Consumes: `validated_keys`/`ResolvedKeys` (Task 1), `Claims`/`mint_token`/`verify_token`/`CallbackToken` (Task 2).
- Produces:

```rust
pub struct CompletionGrant {
    pub locator: OperationLocator,
    pub principal: PrincipalRef,
    pub authorization: AuthorizationEvidence,
    pub effect_id: EffectId,
    pub expires_at: Timestamp,
}

pub struct CompletionIngress { /* store: Arc<dyn JournalStore>, audit: Arc<SecurityAuditGate>, keys: ResolvedKeys, horizon: Option<IdempotencyHorizon> */ }
impl CompletionIngress {
    pub fn try_new(store: Arc<dyn JournalStore>, audit: Arc<SecurityAuditGate>, config: CompletionIngressConfig) -> Result<Self, CompletionIngressConfigError>;
    #[must_use] pub fn with_horizon(self, horizon: IdempotencyHorizon) -> Self;
    pub fn mint(&self, grant: &CompletionGrant) -> Result<CallbackToken, MintError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MintError {
    #[error("token encoding failed")] Encoding,
}
```

- [ ] **Step 1: Write the failing mint tests** (bottom of `src/ingress.rs`; reuse the `keys()`/`claims()`-style helpers from Task 2's test module by duplicating the small constructors — test modules do not share code)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CompletionIngressConfig;
    use crate::token::{verify_token, KIND_EFFECT_COMPLETION};
    use finstack_ai_kernel::{
        AuthorizationEvidence, EffectId, LaneId, OperationLocator, PrincipalRef, RunId, SessionId,
        Timestamp,
    };
    use finstack_ai_runtime::{SecretString, SecurityAuditGate};
    use finstack_ai_test::{MemoryJournalStore, MemoryStoreLimits};
    use std::sync::Arc;

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    fn config() -> CompletionIngressConfig {
        CompletionIngressConfig {
            key_id: "k-active".to_owned(),
            key: SecretString::try_new("a".repeat(32)).expect("secret"),
            additional_verification_keys: vec![],
        }
    }

    fn grant() -> CompletionGrant {
        CompletionGrant {
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::parse("01234567-89ab-7cde-89ab-0123456789a1").expect("session"),
                LaneId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("lane"),
                RunId::parse("01234567-89ab-7cde-89ab-0123456789a3").expect("run"),
            )
            .expect("locator"),
            principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                .expect("principal"),
            authorization: AuthorizationEvidence::try_new("policy-v1", "decision-v1")
                .expect("auth"),
            effect_id: EffectId::parse("01234567-89ab-7cde-89ab-0123456789a4").expect("effect"),
            expires_at: ts(10_000),
        }
    }

    async fn ingress() -> CompletionIngress {
        let store = Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 4,
                batches_per_session: 64,
                records_per_session: 256,
                snapshot_bytes: 64 * 1024,
            })
            .expect("store"),
        );
        let gate = SecurityAuditGate::enable_noop();
        CompletionIngress::try_new(store, gate, config()).expect("ingress")
    }

    #[tokio::test]
    async fn mint_produces_a_verifiable_grant_bound_token() {
        let ingress = ingress().await;
        let token = ingress.mint(&grant()).expect("mint");
        let claims = verify_token(&ingress.keys, token.as_str(), ts(0)).expect("verify");
        assert_eq!(claims.kind, KIND_EFFECT_COMPLETION);
        assert_eq!(claims.effect_id, grant().effect_id);
        assert_eq!(claims.locator, grant().locator);
        assert_eq!(claims.kid, "k-active");
    }

    #[tokio::test]
    async fn try_new_rejects_invalid_config() {
        let store = Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 1,
                batches_per_session: 8,
                records_per_session: 16,
                snapshot_bytes: 1024,
            })
            .expect("store"),
        );
        let gate = SecurityAuditGate::enable_noop();
        let bad = CompletionIngressConfig {
            key_id: "k1".to_owned(),
            key: SecretString::try_new("short").expect("secret"),
            additional_verification_keys: vec![],
        };
        assert!(CompletionIngress::try_new(store, gate, bad).is_err());
    }
}
```

Implementer notes: `SecurityAuditGate::enable_noop()` exists (used by `ExternalCompletionRouter::trusted`, driver/ingress/completion.rs:53) — check its exact signature/return there; if it is async or returns `Result`, adapt the call. `MemoryJournalStore` construction is copied from deferred_bridge/helpers/mod.rs:157-166 — if the paths differ, import from `finstack_ai_store_memory` instead of `finstack_ai_test`. The test touches the private `ingress.keys` field, which is fine inside the crate's own test module.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-completion-ingress ingress` — expected: compile error (`CompletionIngress` not defined).

- [ ] **Step 3: Implement construction and mint** (extend `src/ingress.rs`)

```rust
use std::sync::Arc;

use finstack_ai_kernel::{
    AuthorizationEvidence, EffectId, OperationLocator, PrincipalRef, Timestamp,
};
use finstack_ai_runtime::{IdempotencyHorizon, JournalStore, SecurityAuditGate};
use thiserror::Error;

use crate::config::{CompletionIngressConfig, CompletionIngressConfigError, ResolvedKeys, validated_keys};
use crate::token::{CallbackToken, Claims, CLAIMS_VERSION, KIND_EFFECT_COMPLETION, mint_token};

/// What a minted token authorizes: one effect on one run, until expiry.
#[derive(Clone)]
pub struct CompletionGrant {
    /// Durable target locator of the accepted run.
    pub locator: OperationLocator,
    /// Principal frozen at deferral time.
    pub principal: PrincipalRef,
    /// Authorization evidence frozen at deferral time.
    pub authorization: AuthorizationEvidence,
    /// Original deferred effect identity.
    pub effect_id: EffectId,
    /// Token expiry; must not exceed the configured idempotency horizon.
    pub expires_at: Timestamp,
}

/// Token issuance failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MintError {
    /// Claims could not be canonically encoded or signed.
    #[error("token encoding failed")]
    Encoding,
}

/// Host-embedded ingress: mints callback tokens and delivers completions.
pub struct CompletionIngress {
    pub(crate) store: Arc<dyn JournalStore>,
    pub(crate) audit: Arc<SecurityAuditGate>,
    pub(crate) keys: ResolvedKeys,
    pub(crate) horizon: Option<IdempotencyHorizon>,
}

impl CompletionIngress {
    /// Construct with a validated signing config and a real audit gate.
    ///
    /// There is intentionally no no-op-audit constructor on this type.
    ///
    /// # Errors
    ///
    /// Returns [`CompletionIngressConfigError`] for invalid keys.
    pub fn try_new(
        store: Arc<dyn JournalStore>,
        audit: Arc<SecurityAuditGate>,
        config: CompletionIngressConfig,
    ) -> Result<Self, CompletionIngressConfigError> {
        Ok(Self {
            store,
            audit,
            keys: validated_keys(&config)?,
            horizon: None,
        })
    }

    /// Apply the application idempotency horizon to every delivery.
    #[must_use]
    pub fn with_horizon(mut self, horizon: IdempotencyHorizon) -> Self {
        self.horizon = Some(horizon);
        self
    }

    /// Mint one signed callback token for one deferred effect.
    ///
    /// # Errors
    ///
    /// Returns [`MintError::Encoding`] when signing fails.
    pub fn mint(&self, grant: &CompletionGrant) -> Result<CallbackToken, MintError> {
        let claims = Claims {
            v: CLAIMS_VERSION,
            kid: self.keys.active_id.as_ref().to_owned(),
            kind: KIND_EFFECT_COMPLETION.to_owned(),
            locator: grant.locator.clone(),
            principal: grant.principal.clone(),
            authorization: grant.authorization.clone(),
            effect_id: grant.effect_id,
            expires_at: grant.expires_at,
        };
        mint_token(&self.keys, &claims).map_err(|_| MintError::Encoding)
    }
}
```

(If `EffectId` is not `Copy`, clone it. If the `Arc<dyn JournalStore>` generic bound differs, mirror `ExternalCompletionRouter::new`'s parameter exactly.)

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-completion-ingress` — expected: all prior + 2 new pass.

- [ ] **Step 5: Commit**

```bash
git add extensions/interop/finstack-ai-completion-ingress/src
git commit -m "feat(completion-ingress): ingress construction and grant-bound token minting"
```

---

### Task 5: `deliver` — pre-router pipeline with audited opaque rejection

**Files:**
- Modify: `extensions/interop/finstack-ai-completion-ingress/src/ingress.rs`
- Test: same file, extend `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `verify_token`/`VerifyFailure` (Task 2), `ingress_audit_event`/`audit_and_reject` (Task 3), `ExternalCompletionRouter`/`ExternalRouteOutcome`/`ExternalRouteError` (finstack-ai-runtime), kernel `ExternalEffectCompletion`/`ExternalEffectCompletionCommand`/`ExternalEffectOutcome`.
- Produces: `pub async fn deliver(&self, token: &str, body: &[u8], submitted_at: Timestamp) -> Result<ExternalRouteOutcome, IngressError>` and consts `pub(crate) const MAX_BODY_BYTES: usize = 1_048_576;`.

- [ ] **Step 1: Write the failing pre-router tests.** These use a `RecordingSink` (copy verbatim from `crates/finstack-ai-test/tests/crash_prefix/helpers/mod.rs:507-530` — a `Mutex<Vec<SecurityAuditEvent>>` sink implementing `SecurityAuditSink` with always-ready health) and `SecurityAuditGate::enable(Some(sink), Duration::from_millis(100))`:

```rust
    #[tokio::test]
    async fn garbage_token_audits_malformed_and_rejects_opaquely() {
        let (ingress, sink) = recording_ingress().await; // helper: like ingress() but real gate + RecordingSink
        let error = ingress
            .deliver("not-a-token", b"{}", ts(50))
            .await
            .expect_err("garbage");
        assert_eq!(error, IngressError::Rejected);
        let events = sink.events.lock().expect("lock");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].reason_code(), "malformed_token");
    }

    #[tokio::test]
    async fn identical_garbage_replay_is_one_audit_event() {
        let (ingress, sink) = recording_ingress().await;
        for _ in 0..2 {
            let _ = ingress.deliver("not-a-token", b"{}", ts(50)).await;
        }
        let events = sink.events.lock().expect("lock");
        // Two records with the same event_id; the gate/sink contract is
        // idempotent by event_id, so assert both share one id.
        assert!(events.iter().all(|event| event.event_id() == events[0].event_id()));
    }

    #[tokio::test]
    async fn expired_token_audits_authentication_failure() {
        let (ingress, sink) = recording_ingress().await;
        let mut expiring = grant();
        expiring.expires_at = ts(100);
        let token = ingress.mint(&expiring).expect("mint");
        let error = ingress
            .deliver(token.as_str(), b"{}", ts(100))
            .await
            .expect_err("expired");
        assert_eq!(error, IngressError::Rejected);
        let events = sink.events.lock().expect("lock");
        assert_eq!(events[0].reason_code(), "expired_token");
    }

    #[tokio::test]
    async fn oversize_and_undecodable_bodies_reject_after_auth() {
        let (ingress, sink) = recording_ingress().await;
        let token = ingress.mint(&grant()).expect("mint");
        let oversize = vec![b'x'; MAX_BODY_BYTES + 1];
        assert_eq!(
            ingress.deliver(token.as_str(), &oversize, ts(50)).await.expect_err("oversize"),
            IngressError::Rejected
        );
        assert_eq!(
            ingress
                .deliver(token.as_str(), br#"{"unexpected":true}"#, ts(50))
                .await
                .expect_err("bad body"),
            IngressError::Rejected
        );
        let events = sink.events.lock().expect("lock");
        assert!(events.iter().any(|event| event.reason_code() == "oversize_body"));
        assert!(events.iter().any(|event| event.reason_code() == "invalid_body"));
        // Post-auth events carry the authenticated principal.
        assert!(events
            .iter()
            .filter(|event| event.reason_code() == "oversize_body")
            .all(|event| event.principal().is_some()));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-completion-ingress deliver` — expected: compile error (`deliver` not defined).

- [ ] **Step 3: Implement `deliver`** (extend `impl CompletionIngress`)

```rust
/// Maximum accepted delivery body in bytes.
pub(crate) const MAX_BODY_BYTES: usize = 1_048_576;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryBody {
    #[serde(default)]
    completion_id: Option<String>,
    outcome: finstack_ai_kernel::ExternalEffectOutcome,
}

impl CompletionIngress {
    /// Deliver one external completion: verify the token, decode the body,
    /// and route the authenticated command.
    ///
    /// # Errors
    ///
    /// Returns the opaque [`IngressError::Rejected`] for every token, body,
    /// or audit failure without revealing target existence, and
    /// [`IngressError::Unavailable`] for store/commit failures on a known
    /// authorized target.
    pub async fn deliver(
        &self,
        token: &str,
        body: &[u8],
        submitted_at: Timestamp,
    ) -> Result<finstack_ai_runtime::ExternalRouteOutcome, IngressError> {
        use finstack_ai_runtime::SecurityAuditCategory;

        let claims = match crate::token::verify_token(&self.keys, token, submitted_at) {
            Ok(claims) => claims,
            Err(failure) => {
                let (category, reason) = match failure {
                    crate::token::VerifyFailure::Malformed => {
                        (SecurityAuditCategory::MalformedToken, "malformed_token")
                    }
                    crate::token::VerifyFailure::UnknownKey => {
                        (SecurityAuditCategory::AuthenticationFailure, "unknown_key")
                    }
                    crate::token::VerifyFailure::BadSignature => {
                        (SecurityAuditCategory::AuthenticationFailure, "bad_signature")
                    }
                    crate::token::VerifyFailure::Expired => {
                        (SecurityAuditCategory::AuthenticationFailure, "expired_token")
                    }
                };
                let event = crate::audit::ingress_audit_event(
                    category,
                    reason,
                    None,
                    None,
                    token.as_bytes(),
                    body,
                    submitted_at,
                );
                return Err(crate::audit::audit_and_reject(&self.audit, event).await);
            }
        };

        let reject_body = |reason: &'static str, claims: &crate::token::Claims| {
            crate::audit::ingress_audit_event(
                SecurityAuditCategory::MalformedToken,
                reason,
                Some(claims.principal.clone()),
                Some(claims.locator.tenant_scope.as_ref()),
                token.as_bytes(),
                body,
                submitted_at,
            )
        };

        if body.len() > MAX_BODY_BYTES {
            let event = reject_body("oversize_body", &claims);
            return Err(crate::audit::audit_and_reject(&self.audit, event).await);
        }
        let decoded: DeliveryBody = match serde_json::from_slice(body) {
            Ok(decoded) => decoded,
            Err(_) => {
                let event = reject_body("invalid_body", &claims);
                return Err(crate::audit::audit_and_reject(&self.audit, event).await);
            }
        };
        let completion_id = decoded
            .completion_id
            .unwrap_or_else(|| claims.effect_id.to_canonical_string());
        let command = match finstack_ai_kernel::ExternalEffectCompletion::try_new(
            claims.effect_id,
            completion_id,
            decoded.outcome,
        )
        .and_then(|completion| {
            finstack_ai_kernel::ExternalEffectCompletionCommand::try_new(
                claims.locator.clone(),
                claims.principal.clone(),
                claims.authorization.clone(),
                completion,
            )
        }) {
            Ok(command) => command,
            Err(_) => {
                let event = reject_body("invalid_body", &claims);
                return Err(crate::audit::audit_and_reject(&self.audit, event).await);
            }
        };

        let mut router = finstack_ai_runtime::ExternalCompletionRouter::new(
            Arc::clone(&self.store),
            Arc::clone(&self.audit),
        );
        if let Some(horizon) = self.horizon {
            router = router.with_horizon(horizon);
        }
        router.route(command, submitted_at).await.map_err(|error| match error {
            finstack_ai_runtime::ExternalRouteError::IngressRejected
            | finstack_ai_runtime::ExternalRouteError::InvalidNormalizedCommand => {
                IngressError::Rejected
            }
            finstack_ai_runtime::ExternalRouteError::IdAllocation => IngressError::Unavailable {
                reason_code: "id_allocation",
            },
            finstack_ai_runtime::ExternalRouteError::Runtime(_) => IngressError::Unavailable {
                reason_code: "runtime",
            },
        })
    }
}
```

(If the two `try_new` error types differ so `and_then` will not chain, use two sequential `match` blocks with the same `"invalid_body"` rejection. If `IdempotencyHorizon` is not `Copy`, clone it.)

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-completion-ingress` — expected: all pass, including the four new delivery tests.

- [ ] **Step 5: Commit**

```bash
git add extensions/interop/finstack-ai-completion-ingress/src
git commit -m "feat(completion-ingress): audited fail-closed delivery pipeline"
```

---

### Task 6: End-to-end integration tests (`tests/deliver.rs`)

**Files:**
- Create: `extensions/interop/finstack-ai-completion-ingress/tests/deliver.rs`

**Interfaces:**
- Consumes: everything public from Tasks 1–5; `finstack_ai::Agent` scripted-deferral setup copied from `crates/finstack-ai-test/tests/deferred_bridge/helpers/mod.rs:286-361` (the `deferred_tool_parent` pattern: `ScriptedToolset` emitting `ToolStreamItem::Deferred`, `ScriptedModel` issuing the tool call, `MemoryJournalStore`, poll `CommitCoordinator::recover` until `RunPhase::AwaitingExternal` and read `deferred.effect_id` from the active tool call).

- [ ] **Step 1: Port the deferral helper.** Copy `deferred_tool_parent`, `echo_tool_spec`, `security`, `agent_request`, `memory_store`, and the `profile`/`echo_tool_call`/`completed` scripted-model helpers from `crates/finstack-ai-test/tests/deferred_bridge/helpers/mod.rs` into a `mod helpers` block in `tests/deliver.rs`, plus the `RecordingSink` from `crates/finstack-ai-test/tests/crash_prefix/helpers/mod.rs:507-530`. Add one helper of our own:

```rust
fn grant_for(parent: &finstack_ai::AgentRun, effect_id: finstack_ai_kernel::EffectId) -> CompletionGrant {
    let security = helpers::security();
    CompletionGrant {
        locator: parent.locator().clone(),
        principal: security.principal().clone(),
        authorization: finstack_ai_kernel::AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .expect("auth"),
        effect_id,
        expires_at: Timestamp::from_unix_ms(60_000).expect("timestamp"),
    }
}

const FAILED_BODY: &[u8] =
    br#"{"outcome":{"failed":{"error":{"code":"provider_failed","message":"provider failed","category":"model","retryable":true}}}}"#;
```

(The exact `ExternalEffectOutcome::Failed` wire encoding must match the kernel's strict serde — derive it by serializing `ExternalEffectOutcome::Failed { error: ErrorDescriptor::new("provider_failed", "provider failed", ErrorCategory::Model, true).expect("error") }` with `serde_json::to_vec` in the test itself rather than hard-coding, i.e. `let body = serde_json::to_vec(&serde_json::json!({"outcome": outcome_value}))` where `outcome_value = serde_json::to_value(&outcome).expect("outcome")`. Use that mechanism; the literal above is illustrative only and must not be committed if it disagrees with the serializer.)

- [ ] **Step 2: Write the failing end-to-end tests**

```rust
#[tokio::test]
async fn delivery_commits_then_replays_idempotently_then_conflicts() {
    let (_agent, parent, store, effect_id) = helpers::deferred_tool_parent().await;
    let sink = std::sync::Arc::new(helpers::RecordingSink::default());
    let gate = SecurityAuditGate::enable(
        Some(std::sync::Arc::clone(&sink) as _),
        std::time::Duration::from_millis(100),
    )
    .await
    .expect("gate");
    let ingress = CompletionIngress::try_new(store, gate, helpers::config()).expect("ingress");
    let token = ingress.mint(&grant_for(&parent, effect_id)).expect("mint");
    let body = helpers::failed_outcome_body(); // serialized as described in Step 1

    let first = ingress
        .deliver(token.as_str(), &body, ts(1_000))
        .await
        .expect("first");
    assert!(matches!(first, ExternalRouteOutcome::Committed(_)));

    let replay = ingress
        .deliver(token.as_str(), &body, ts(1_001))
        .await
        .expect("replay");
    assert!(matches!(replay, ExternalRouteOutcome::Idempotent { .. }));

    let conflicting = helpers::failed_outcome_body_with_message("different failure");
    let conflict = ingress
        .deliver(token.as_str(), &conflicting, ts(1_002))
        .await
        .expect("conflict routes");
    assert!(matches!(conflict, ExternalRouteOutcome::Rejected { .. }));
}

#[tokio::test]
async fn token_for_unknown_session_is_indistinguishable_from_garbage() {
    let (_agent, _parent, store, _effect_id) = helpers::deferred_tool_parent().await;
    let sink = std::sync::Arc::new(helpers::RecordingSink::default());
    let gate = SecurityAuditGate::enable(
        Some(std::sync::Arc::clone(&sink) as _),
        std::time::Duration::from_millis(100),
    )
    .await
    .expect("gate");
    let ingress = CompletionIngress::try_new(store, gate, helpers::config()).expect("ingress");
    // Valid signature, nonexistent session: the router audits unknown_locator
    // and the caller sees the same opaque error as a garbage token.
    let phantom = helpers::phantom_grant(); // fabricated ids, same tenant label
    let token = ingress.mint(&phantom).expect("mint");
    let body = helpers::failed_outcome_body();
    let unknown = ingress
        .deliver(token.as_str(), &body, ts(1_000))
        .await
        .expect_err("unknown session");
    let garbage = ingress
        .deliver("fcit1.AAAA.BBBB", &body, ts(1_000))
        .await
        .expect_err("garbage");
    assert_eq!(unknown, garbage);
}

#[tokio::test]
async fn horizon_expires_deliveries_regardless_of_token_expiry() {
    let (_agent, parent, store, effect_id) = helpers::deferred_tool_parent().await;
    let sink = std::sync::Arc::new(helpers::RecordingSink::default());
    let gate = SecurityAuditGate::enable(
        Some(std::sync::Arc::clone(&sink) as _),
        std::time::Duration::from_millis(100),
    )
    .await
    .expect("gate");
    let ingress = CompletionIngress::try_new(store, gate, helpers::config())
        .expect("ingress")
        .with_horizon(IdempotencyHorizon { expire_at: ts(500) });
    let token = ingress.mint(&grant_for(&parent, effect_id)).expect("mint"); // token expires at 60_000
    let body = helpers::failed_outcome_body();
    let error = ingress
        .deliver(token.as_str(), &body, ts(1_000))
        .await
        .expect_err("past horizon");
    assert_eq!(error, IngressError::Rejected);
    let events = sink.events.lock().expect("lock");
    assert!(events.iter().any(|event| event.reason_code() == "expired_locator"));
}
```

`helpers::phantom_grant()` builds a `CompletionGrant` whose `OperationLocator` uses fabricated (but syntactically valid) session/lane/run UUIDs under `"tenant-preview"` and a fabricated `EffectId`. `helpers::config()` is the Task 4 test config. `helpers::failed_outcome_body_with_message` serializes a `Failed` outcome whose `ErrorDescriptor` message differs.

- [ ] **Step 3: Run to verify failure, then make it pass**

Run: `cargo test -p finstack-ai-completion-ingress --test deliver` — expected first: compile errors in helpers wiring; fix imports/signatures against the real helper sources until the three tests pass. No production code changes are expected in this task; if one is needed, it indicates a Task 5 bug — fix it there and note it in the commit message.

- [ ] **Step 4: Commit**

```bash
git add extensions/interop/finstack-ai-completion-ingress/tests
git commit -m "test(completion-ingress): end-to-end commit/idempotent/conflict/horizon coverage"
```

---

### Task 7: Baselines, docs, and gates

**Files:**
- Create: `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-completion-ingress.txt` (generated)
- Modify: `extensions/interop/finstack-ai-completion-ingress/README.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Finish the README.** Append a construction example after the trust paragraph:

````markdown
```rust
use finstack_ai_completion_ingress::{CompletionIngress, CompletionIngressConfig};

let ingress = CompletionIngress::try_new(store, audit_gate, CompletionIngressConfig {
    key_id: "k-2026-08".to_owned(),
    key: signing_key,                       // SecretString, >= 32 bytes
    additional_verification_keys: vec![],   // retired keys during rotation
})?;
let token = ingress.mint(&grant)?;          // hand token.as_str() to the external job
// later, from the host's transport terminator or the workflow webhook:
let outcome = ingress.deliver(received_token, received_body, now).await?;
```
````

Also state: minted tokens are signed but not encrypted (locator ids are non-secret); duplicate deliveries with an equal body are idempotent; conflicting duplicates fail closed with durable rejection evidence; interaction resolutions are out of scope.

- [ ] **Step 2: Regenerate the public-api baseline**

Run: `mise run check-public-api` — expected: FAIL with `finstack-ai-completion-ingress: missing baseline`. Then run the write mode (`python3 scripts/compat/public_api.py --write`, or the mise task variant the script's `--help` documents), and re-run `mise run check-public-api` — expected: PASS. Inspect the generated baseline: it should be small (target ≲ 25 lines — config, grant, ingress, token, three error enums). If anything unintended is public (e.g. `Claims`, `validated_keys`), fix visibility and regenerate.

- [ ] **Step 3: CHANGELOG entry** under the unreleased/next heading, matching existing entry style:

```markdown
- `finstack-ai-completion-ingress`: new `extensions/interop` leaf that mints signed
  opaque callback tokens for deferred effects and delivers authenticated external
  completions through `ExternalCompletionRouter` (TM-10; the workflow webhook path).
```

- [ ] **Step 4: Full gates**

Run and check exit codes explicitly (per RTK guidance, do not gate on rtk-filtered output):

```bash
cargo fmt --all -- --check
cargo clippy -p finstack-ai-completion-ingress --all-targets -- -D warnings
cargo test -p finstack-ai-completion-ingress
python3 scripts/wasm_package/check.py
mise run check-public-api
```

Expected: all exit 0.

- [ ] **Step 5: Commit**

```bash
git add fixtures/compatibility/public-rust-api extensions/interop/finstack-ai-completion-ingress/README.md CHANGELOG.md
git commit -m "chore(completion-ingress): public-api baseline, README, changelog"
```

---

## Out of scope (tracked in the spec §9)

Interaction-resolution tokens; HTTP/TCP listener; polling/reconciliation loop and durable inbox (workflow-worker design); SDK facade constructor (ADR-045 wiring can follow additively); delivery-ledger/threat-model registry updates (governance transactions follow the repo's PR-envelope process, not this plan).
