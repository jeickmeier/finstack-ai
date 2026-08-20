# Policy Instructions Middleware Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new leaf middleware crate that injects tenant/policy instructions (compliance footer, as-of date, locale, tenant rules) at the `prepare_context` stage via `StageOutcome::AddInstructions`.

**Architecture:** New workspace crate `extensions/middleware/finstack-ai-middleware-instructions` implementing the ordinary `Middleware` port with `MiddlewareRole::Standard` at `Stage::PrepareContext`. Instructions are frozen into a serializable config at construction (pure/deterministic, `RecomputeSafe`); each entry becomes a protected `ContextItem` that the runtime mints as a System message, which the unique `ContextCompactor` (a `BeforeModel` component) must preserve byte-identically. No runtime changes are required — the stage contract, fold, and apply paths already support this outcome.

**Tech Stack:** Rust, `finstack-ai-kernel`, `finstack-ai-runtime` (ports only), `serde`/`serde_json` (digest), `thiserror`; tests with `tokio` + `finstack-ai-test` conformance harness.

**Spec:** `docs/superpowers/specs/2026-08-20-policy-instructions-design.md`

## Global Constraints

- Crate name `finstack-ai-middleware-instructions`, component id `finstack.middleware.instructions`, version `1.0.0`, `publish = false`, `[lints] workspace = true`.
- Copy the lint header verbatim from `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs:1-22` (`#![forbid(unsafe_code)]`, deny `unwrap_used`/`expect_used`/`panic`/`unreachable`, test-only allows).
- Middleware must be pure w.r.t. external state (`crates/finstack-ai-runtime/src/exec/middleware_driver/mod.rs:33-46`): no clock, no I/O, no randomness in `invoke`.
- Error codes are stable snake_case; construction errors use a `Configuration { reason: &'static str }` variant (convention from the verify and compaction leaves).
- Config validation: 1..=16 entries, non-empty `label` and `text` (fold bound: `instructions + context ≤ SEMANTIC_ARRAY_MAX_ITEMS` per stage, `crates/finstack-ai-runtime/src/exec/middleware_driver/fold.rs:197-222`).
- `configuration_digest = Digest::raw_json(&serde_json::to_vec(&config))` so distinct tenant policies get distinct digests.
- Verification commands: `cargo test -p finstack-ai-middleware-instructions`, `cargo clippy -p finstack-ai-middleware-instructions --all-targets --all-features`, then `mise run check-rust` before the final commit. If `mise run check-public-api` reports a baseline change for the new crate, regenerate baselines per its output and include them in the commit.

---

### Task 1: Crate scaffold, config, and error types

**Files:**
- Create: `extensions/middleware/finstack-ai-middleware-instructions/Cargo.toml`
- Create: `extensions/middleware/finstack-ai-middleware-instructions/README.md`
- Create: `extensions/middleware/finstack-ai-middleware-instructions/src/lib.rs`
- Create: `extensions/middleware/finstack-ai-middleware-instructions/src/tests.rs`
- Modify: `Cargo.toml` (root) — `[workspace] members` at lines 35-36 (alphabetical: compaction, **instructions**, verify) and `[workspace.dependencies]` at lines 143-144

**Interfaces:**
- Produces: `PolicyEntry { pub label: String, pub text: String }`, `PolicyInstructionsConfig { pub entries: Vec<PolicyEntry> }` (both `Debug, Clone, PartialEq, Eq, Serialize`), `InstructionsError::Configuration { reason: &'static str }` (`thiserror`, message `"instructions_configuration_invalid: {reason}"`), and `PolicyInstructionsConfig::validate(&self) -> Result<(), InstructionsError>`.
- Consumes: nothing from other tasks.

- [ ] **Step 1: Add the crate to the workspace**

In root `Cargo.toml`, insert into `[workspace] members` between the compaction and verify lines:

```toml
    "extensions/middleware/finstack-ai-middleware-instructions",
```

and into `[workspace.dependencies]` between the compaction and verify entries:

```toml
finstack-ai-middleware-instructions = { path = "extensions/middleware/finstack-ai-middleware-instructions", version = "1.0.0" }
```

- [ ] **Step 2: Write the crate manifest and README**

`extensions/middleware/finstack-ai-middleware-instructions/Cargo.toml` (mirrors the verify leaf, plus serde for the digest):

```toml
[package]
name = "finstack-ai-middleware-instructions"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Tenant/policy instruction injection middleware for finstack-ai (prepare_context)"
readme = "README.md"
publish = false

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
finstack-ai-test = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros"] }

[lints]
workspace = true
```

`README.md`:

```markdown
# finstack-ai-middleware-instructions

Injects tenant/policy instructions (compliance footer, as-of date, locale,
tenant rules) at the `prepare_context` stage via `AddInstructions`. Pure and
deterministic: all text is frozen into the configuration at construction.
Injected items land as protected System messages that the context compactor
must preserve. Design: `docs/superpowers/specs/2026-08-20-policy-instructions-design.md`.
```

- [ ] **Step 3: Write failing tests for config validation and error display**

`src/tests.rs`:

```rust
use super::*;

fn entry(label: &str, text: &str) -> PolicyEntry {
    PolicyEntry {
        label: label.to_owned(),
        text: text.to_owned(),
    }
}

#[test]
fn valid_config_passes_validation() {
    let config = PolicyInstructionsConfig {
        entries: vec![
            entry("compliance-footer", "All outputs are for tenant-a internal use only."),
            entry("as-of", "Treat 2026-08-20 as the current date."),
        ],
    };
    config.validate().expect("valid");
}

#[test]
fn empty_entries_are_rejected() {
    let config = PolicyInstructionsConfig { entries: vec![] };
    let error = config.validate().expect_err("empty");
    assert_eq!(
        error.to_string(),
        "instructions_configuration_invalid: entries_empty"
    );
}

#[test]
fn more_than_sixteen_entries_are_rejected() {
    let config = PolicyInstructionsConfig {
        entries: (0..17).map(|i| entry(&format!("rule-{i}"), "text")).collect(),
    };
    let error = config.validate().expect_err("too many");
    assert_eq!(
        error.to_string(),
        "instructions_configuration_invalid: entries_exceed_maximum"
    );
}

#[test]
fn blank_label_or_text_is_rejected() {
    let blank_label = PolicyInstructionsConfig {
        entries: vec![entry("", "text")],
    };
    assert_eq!(
        blank_label.validate().expect_err("label").to_string(),
        "instructions_configuration_invalid: entry_label_empty"
    );
    let blank_text = PolicyInstructionsConfig {
        entries: vec![entry("locale", "   ")],
    };
    assert_eq!(
        blank_text.validate().expect_err("text").to_string(),
        "instructions_configuration_invalid: entry_text_empty"
    );
}
```

`src/lib.rs` — lint header (copied verbatim from `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs:1-22`, with the doc comment swapped), then only the module hook so the tests compile against missing types and fail:

```rust
//! Tenant/policy instruction injection middleware (`prepare_context`).

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
#![doc(test(attr(allow(clippy::expect_used))))]

#[cfg(test)]
mod tests;
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-middleware-instructions`
Expected: COMPILE ERROR — `PolicyEntry`, `PolicyInstructionsConfig`, `InstructionsError` not found.

- [ ] **Step 5: Implement the config and error types**

Append to `src/lib.rs` (above `mod tests`):

```rust
use serde::Serialize;
use thiserror::Error;

/// Maximum number of policy entries one middleware may inject.
pub const MAX_POLICY_ENTRIES: usize = 16;

/// Instructions-leaf construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InstructionsError {
    /// Configuration is malformed.
    #[error("instructions_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// One policy instruction entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyEntry {
    /// Stable label; becomes the item's provenance source id (`policy:{label}`).
    pub label: String,
    /// Instruction text injected as a System message.
    pub text: String,
}

/// Ordered policy instruction set frozen at construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyInstructionsConfig {
    /// Entries injected in order at `prepare_context`.
    pub entries: Vec<PolicyEntry>,
}

impl PolicyInstructionsConfig {
    /// Validate entry count and per-entry label/text.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration reason for empty, oversized, or blank input.
    pub fn validate(&self) -> Result<(), InstructionsError> {
        if self.entries.is_empty() {
            return Err(InstructionsError::Configuration {
                reason: "entries_empty",
            });
        }
        if self.entries.len() > MAX_POLICY_ENTRIES {
            return Err(InstructionsError::Configuration {
                reason: "entries_exceed_maximum",
            });
        }
        for entry in &self.entries {
            if entry.label.trim().is_empty() {
                return Err(InstructionsError::Configuration {
                    reason: "entry_label_empty",
                });
            }
            if entry.text.trim().is_empty() {
                return Err(InstructionsError::Configuration {
                    reason: "entry_text_empty",
                });
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-middleware-instructions`
Expected: PASS (4 tests).

- [ ] **Step 7: Lint and commit**

Run: `cargo clippy -p finstack-ai-middleware-instructions --all-targets --all-features`
Expected: clean.

```bash
git add Cargo.toml Cargo.lock extensions/middleware/finstack-ai-middleware-instructions
git commit -m "feat: scaffold policy-instructions middleware crate with config validation"
```

---

### Task 2: `InstructionsMiddleware` — descriptor and invoke

**Files:**
- Modify: `extensions/middleware/finstack-ai-middleware-instructions/src/lib.rs`
- Test: `extensions/middleware/finstack-ai-middleware-instructions/src/tests.rs`

**Interfaces:**
- Consumes: `PolicyInstructionsConfig`, `InstructionsError` from Task 1.
- Produces: `InstructionsMiddleware` with `pub fn try_new(config: PolicyInstructionsConfig) -> Result<Self, InstructionsError>` and `impl finstack_ai_runtime::Middleware` (descriptor: component `finstack.middleware.instructions`, stage `PrepareContext`, `OrderTier::Standard`, `MiddlewareRole::Standard`, `InvocationRecovery::RecomputeSafe`; invoke: `StageOutcome::AddInstructions`). This is the type applications register via `NativeAgentBuilder::middleware` (`crates/finstack-ai/src/agent/builder.rs:207-211`).

- [ ] **Step 1: Write failing behavior tests**

Append to `src/tests.rs`. The `ctx()` helper is copied from the verify leaf's test pattern (`extensions/middleware/finstack-ai-middleware-verify/src/tests.rs:14-50`):

```rust
use std::sync::Arc;

use finstack_ai_kernel::{
    Digest, EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RawJson, RunId, SessionId,
    Stage,
};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, ContextAuthority, ContextItemKind, Middleware,
    MiddlewareContext, MiddlewareRole, RunCallContext, StageInput, StageOutcome,
    validate_stage_outcome,
};
use finstack_ai_test::{MiddlewareConformanceCase, check_middleware_conformance};

fn id<T>(value: u64, parse: impl FnOnce(&str) -> T) -> T {
    parse(&format!("00000000-0000-7000-8000-{value:012x}"))
}

fn ctx() -> MiddlewareContext {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    MiddlewareContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                id(1, |value| SessionId::parse(value).expect("session")),
                id(2, |value| LaneId::parse(value).expect("lane")),
                id(3, |value| RunId::parse(value).expect("run")),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal,
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: id(4, |value| EffectId::parse(value).expect("effect")),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        },
        chain_digest: Digest::raw_json(b"chain"),
        chain_index: 0,
        compaction_resume: None,
    }
}

fn prepare_input() -> StageInput {
    StageInput::PrepareContext {
        value: RawJson::parse(b"[]").expect("messages"),
    }
}

fn sample_config() -> PolicyInstructionsConfig {
    PolicyInstructionsConfig {
        entries: vec![
            PolicyEntry {
                label: "compliance-footer".to_owned(),
                text: "All outputs are for tenant-a internal use only.".to_owned(),
            },
            PolicyEntry {
                label: "as-of".to_owned(),
                text: "Treat 2026-08-20 as the current date.".to_owned(),
            },
        ],
    }
}

#[tokio::test]
async fn invoke_adds_protected_instruction_items_in_entry_order() {
    let middleware = InstructionsMiddleware::try_new(sample_config()).expect("middleware");
    let outcome = middleware
        .invoke(ctx(), prepare_input())
        .await
        .expect("invoke");
    let StageOutcome::AddInstructions(items) = &outcome else {
        panic!("expected AddInstructions, got {outcome:?}");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(&*items[0].provenance.source_id, "policy:compliance-footer");
    assert_eq!(&*items[1].provenance.source_id, "policy:as-of");
    for item in items.iter() {
        assert_eq!(item.kind, ContextItemKind::Instruction);
        assert_eq!(item.authority, ContextAuthority::TrustedApplication);
        assert!(item.protected);
        assert!(!item.provenance.external);
    }
    validate_stage_outcome(&middleware.descriptor(), &prepare_input(), &outcome)
        .expect("allowed at prepare_context");
}

#[tokio::test]
async fn descriptor_declares_prepare_context_standard_role() {
    let middleware = InstructionsMiddleware::try_new(sample_config()).expect("middleware");
    let descriptor = middleware.descriptor();
    assert!(descriptor.stages.contains(Stage::PrepareContext));
    assert!(!descriptor.stages.contains(Stage::BeforeModel));
    assert_eq!(descriptor.role, MiddlewareRole::Standard);
    assert_eq!(
        descriptor.invocation.component,
        finstack_ai_kernel::ComponentId::parse("finstack.middleware.instructions")
            .expect("component id")
    );
}

#[tokio::test]
async fn distinct_configs_produce_distinct_digests() {
    let first = InstructionsMiddleware::try_new(sample_config()).expect("first");
    let mut other = sample_config();
    other.entries[1].text = "Treat 2026-08-21 as the current date.".to_owned();
    let second = InstructionsMiddleware::try_new(other).expect("second");
    assert_ne!(
        first.descriptor().invocation.configuration_digest,
        second.descriptor().invocation.configuration_digest
    );
    let same = InstructionsMiddleware::try_new(sample_config()).expect("same");
    assert_eq!(
        first.descriptor().invocation.configuration_digest,
        same.descriptor().invocation.configuration_digest
    );
}

#[tokio::test]
async fn wrong_stage_input_is_rejected() {
    let middleware = InstructionsMiddleware::try_new(sample_config()).expect("middleware");
    let error = middleware
        .invoke(
            ctx(),
            StageInput::BeforeFinalize {
                candidate: RawJson::parse(br#""candidate""#).expect("candidate"),
            },
        )
        .await
        .expect_err("wrong stage");
    assert_eq!(
        error.code(),
        finstack_ai_runtime::MIDDLEWARE_OUTCOME_NOT_ALLOWED
    );
}

#[tokio::test]
async fn invalid_config_is_rejected_at_construction() {
    let error = InstructionsMiddleware::try_new(PolicyInstructionsConfig { entries: vec![] })
        .expect_err("invalid");
    assert_eq!(
        error.to_string(),
        "instructions_configuration_invalid: entries_empty"
    );
}

#[tokio::test]
async fn middleware_satisfies_the_published_port_conformance_suite() {
    let middleware = InstructionsMiddleware::try_new(sample_config()).expect("middleware");
    let expected = match middleware
        .invoke(ctx(), prepare_input())
        .await
        .expect("expected outcome")
    {
        outcome @ StageOutcome::AddInstructions(_) => outcome,
        other => panic!("expected AddInstructions, got {other:?}"),
    };
    let outcome = check_middleware_conformance(
        &middleware,
        MiddlewareConformanceCase {
            context: ctx(),
            input: prepare_input(),
            expected: expected.clone(),
        },
    )
    .await
    .expect("published middleware conformance suite");
    assert_eq!(outcome, expected);
}
```

Note for the implementer: if `MiddlewareConformanceCase` in `crates/finstack-ai-test/src/conformance/ports.rs:285-331` has drifted from this field shape, adapt the conformance test to the harness as it exists — the harness is the source of truth.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p finstack-ai-middleware-instructions`
Expected: COMPILE ERROR — `InstructionsMiddleware` not found.

- [ ] **Step 3: Implement the middleware**

Append to `src/lib.rs` (imports merge into the existing `use` block region at the top):

```rust
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, ErrorCategory, InvocationRecovery,
    Metadata, Sensitivity, Stage, TextBlock, Version,
};
use finstack_ai_runtime::{
    ContextAuthority, ContextItem, ContextItemKind, ContextProvenance, Middleware,
    MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder, MiddlewareRole,
    OrderTier, PortFuture, StageInput, StageMask, StageOutcome,
};

const INSTRUCTIONS_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Pure `prepare_context` middleware that injects frozen policy instructions.
#[derive(Debug, Clone)]
pub struct InstructionsMiddleware {
    descriptor: MiddlewareDescriptor,
    items: Arc<[ContextItem]>,
}

impl InstructionsMiddleware {
    /// Construct the middleware, freezing one protected item per entry.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration reason for invalid entries or
    /// non-encodable content.
    pub fn try_new(config: PolicyInstructionsConfig) -> Result<Self, InstructionsError> {
        config.validate()?;
        let mut items = Vec::with_capacity(config.entries.len());
        for entry in &config.entries {
            let block = ContentBlock::Text(TextBlock::try_new(&entry.text).map_err(|_| {
                InstructionsError::Configuration {
                    reason: "entry_text_invalid",
                }
            })?);
            let estimated_tokens = u64::try_from(entry.text.len() / 4).unwrap_or(u64::MAX);
            let item = ContextItem::try_new(
                ContextItemKind::Instruction,
                vec![block],
                ContextProvenance {
                    source_id: Arc::from(format!("policy:{}", entry.label)),
                    source_ref: None,
                    external: false,
                },
                ContextAuthority::TrustedApplication,
                0,
                estimated_tokens,
                Sensitivity::Internal,
                true,
            )
            .map_err(|_| InstructionsError::Configuration {
                reason: "entry_item_invalid",
            })?;
            items.push(item);
        }
        let digest_bytes =
            serde_json::to_vec(&config).map_err(|_| InstructionsError::Configuration {
                reason: "configuration_not_serializable",
            })?;
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse("finstack.middleware.instructions").map_err(
                        |_| InstructionsError::Configuration {
                            reason: "invalid_component_id",
                        },
                    )?,
                    version: INSTRUCTIONS_VERSION,
                    configuration_digest: Digest::raw_json(&digest_bytes),
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages: StageMask::from_stages([Stage::PrepareContext]),
                order: MiddlewareOrder {
                    tier: OrderTier::Standard,
                    priority: 0,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            items: items.into(),
        })
    }
}

impl Middleware for InstructionsMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let items = Arc::clone(&self.items);
        Box::pin(async move {
            if !matches!(input, StageInput::PrepareContext { .. }) {
                return Err(MiddlewareError::try_new(
                    finstack_ai_runtime::MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                    ErrorCategory::Middleware,
                    "instructions only run at prepare_context",
                    Metadata::empty(),
                )
                .unwrap_or_else(Into::into));
            }
            Ok(StageOutcome::AddInstructions(items))
        })
    }
}
```

Implementer notes:
- `TextBlock::try_new` and `Digest::raw_json` argument types: match the call shapes used in `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs` and `extensions/middleware/finstack-ai-middleware-compaction/src/lib.rs:105-177` if a borrow/owned mismatch surfaces.
- `ContextItem::try_new` signature is at `crates/finstack-ai-runtime/src/ports/context/types.rs:131-140` — `(kind, content, provenance, authority, priority, estimated_tokens, sensitivity, protected)`; `bytes` is computed internally.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-middleware-instructions`
Expected: PASS (all 10 tests).

- [ ] **Step 5: Lint and commit**

Run: `cargo clippy -p finstack-ai-middleware-instructions --all-targets --all-features`
Expected: clean.

```bash
git add extensions/middleware/finstack-ai-middleware-instructions
git commit -m "feat: add InstructionsMiddleware injecting policy items at prepare_context"
```

---

### Task 3: Docs, public-API baseline, and workspace verification

**Files:**
- Modify: `docs/site/middleware.md` (the shipped-middleware / stage-outcome documentation)
- Modify: public-API baselines only if `mise run check-public-api` flags the new crate (it regenerates its own files; see `crates/finstack-ai-test/tests/public_rust_api.rs` and `scripts/compat/public_items.py`)

**Interfaces:**
- Consumes: the finished crate from Tasks 1-2 (`InstructionsMiddleware::try_new`, `PolicyInstructionsConfig`).
- Produces: nothing new in code — documentation and green workspace checks.

- [ ] **Step 1: Document the middleware**

In `docs/site/middleware.md`, alongside the existing shipped-leaf descriptions (verify, compaction, document-ingest), add a section. Adapt heading level and placement to the file's existing structure:

```markdown
## Policy instructions (`finstack.middleware.instructions`)

`finstack-ai-middleware-instructions` injects tenant/policy instructions —
a compliance footer, an as-of date, a locale tag, tenant rules — at
`prepare_context` via `AddInstructions`. All text is frozen into
`PolicyInstructionsConfig` at construction, so the leaf is pure and
`RecomputeSafe`; the configuration digest changes whenever the policy text
changes. Injected items land as protected System messages appended after the
current user message, which the context compactor must preserve
byte-identically. Register per tenant at agent build time:

```rust
let policy = InstructionsMiddleware::try_new(PolicyInstructionsConfig {
    entries: vec![
        PolicyEntry {
            label: "compliance-footer".to_owned(),
            text: "All outputs are for tenant-a internal use only.".to_owned(),
        },
        PolicyEntry {
            label: "as-of".to_owned(),
            text: "Treat 2026-08-20 as the current date.".to_owned(),
        },
    ],
})?;
builder.middleware(component_ref, Arc::new(policy));
```

Stable error code: `instructions_configuration_invalid`.
```

- [ ] **Step 2: Run the public-API check and update baselines if flagged**

Run: `mise run check-public-api`
Expected: either clean, or instructions to regenerate baselines for the new crate. If flagged, regenerate exactly as the task output says and stage the changed baseline files.

- [ ] **Step 3: Run the full workspace check**

Run: `mise run check-rust`
Expected: PASS (fmt, clippy, tests across the workspace).

- [ ] **Step 4: Commit**

```bash
git add docs/site/middleware.md
git commit -m "docs: document policy-instructions middleware and refresh baselines"
```

(Include regenerated baseline files in the same commit if Step 2 produced any.)
