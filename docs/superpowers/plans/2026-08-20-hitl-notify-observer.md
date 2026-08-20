# HITL Notify Observer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A read-only observer leaf crate `finstack-ai-observer-notify` that announces `InteractionRequested` / `InteractionResolved` / `InteractionExpired` / `InteractionCancelled` to a Slack or generic webhook sink, with structural redaction and a bounded queue.

**Architecture:** Implements the existing runtime `Observer` port (zero kernel/runtime changes). `observe()` projects interaction events into a whitelisted `InteractionNotification`, pushes onto a bounded `ObserverQueue`, then drains and delivers via an object-safe `NotificationSink` with a bounded timeout+retry `DeliveryPolicy`. Failures never propagate to the run.

**Tech Stack:** Rust edition 2024, `finstack-ai-kernel`, `finstack-ai-runtime` (Observer port, `ObserverQueue`, `SecretString`), `reqwest` 0.13.2 (workspace pin, no redirects), `tokio` time, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-20-hitl-notify-observer-design.md` (read it first — the redaction whitelist and public API there are normative).

## Global Constraints

- Work on a feature branch (repo is on `main`): `git checkout -b feature/notify-observer` before Task 1.
- Crate name `finstack-ai-observer-notify`, lib `finstack_ai_observer_notify`, component id `finstack.observer.notify`, descriptor `Version { major: 0, minor: 0, patch: 1 }`.
- Every source file carries the exact lint header from `extensions/observers/finstack-ai-observer-metrics/src/lib.rs:3-22` (lib.rs only; it is crate-wide).
- Error codes are stable snake_case `&'static str` reasons; constructors are `try_new` returning `Result`; no panics, no `unwrap`/`expect` outside tests.
- Secrets: sink URLs are `SecretString`; hand-written `Debug` impls render `[REDACTED]`; no URL or response body ever appears in errors, diagnostics, or exports.
- Redaction whitelist is spec §2. Excluded forever: prompt content blocks, `response_schema`, resolution `response` JSON, `AuthorizationEvidence`, `Metadata`, digests, policy component/version, all non-interaction bodies.
- HTTP: `reqwest::Client` with `.redirect(Policy::none())` and a client-level timeout; http/https URLs only.
- Test loop: `cargo nextest run -p finstack-ai-observer-notify` and `cargo test -p finstack-ai-observer-notify --doc`. Final gates: `mise run check-rust`, `mise run check-public-api` (after baseline regen), `mise run test-rust`.
- Commit after every green task, message style `feat: ...` / `test: ...` / `chore: ...`, trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: Crate scaffold, workspace wiring, observer skeleton

**Files:**
- Modify: root `Cargo.toml` (`[workspace] members` list ~line 40 area, alphabetical among `extensions/observers/*`; `[workspace.dependencies]` ~line 148 area)
- Create: `extensions/observers/finstack-ai-observer-notify/Cargo.toml`
- Create: `extensions/observers/finstack-ai-observer-notify/src/lib.rs`
- Create: `extensions/observers/finstack-ai-observer-notify/src/tests.rs`
- Create: `extensions/observers/finstack-ai-observer-notify/README.md` (stub; finished in Task 6)

**Interfaces:**
- Produces: `NotifyObserverError::Configuration { reason: &'static str }`; `NotifyObserver` (fields added in later tasks); descriptor with `ObserverPayloadMode::Full`.

- [ ] **Step 1: Branch**

```bash
git checkout -b feature/notify-observer
```

- [ ] **Step 2: Wire the workspace**

Root `Cargo.toml`: add member `"extensions/observers/finstack-ai-observer-notify"` (sorted next to the other observer members) and under `[workspace.dependencies]`:

```toml
finstack-ai-observer-notify = { path = "extensions/observers/finstack-ai-observer-notify", version = "1.0.0" }
```

Crate `Cargo.toml` (mirror the metrics leaf exactly, plus the network deps):

```toml
[package]
name = "finstack-ai-observer-notify"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "HITL interaction lifecycle notifier observer for finstack-ai"
readme = "README.md"

[features]
vendored-tls = ["reqwest/native-tls-vendored"]

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["time"] }

[dev-dependencies]
finstack-ai-test = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros", "net", "io-util", "time"] }

[lints]
workspace = true
```

(If `serde`/`serde_json` workspace entries use different feature shapes, copy whatever `extensions/providers/finstack-ai-provider-openrouter/Cargo.toml` does.)

- [ ] **Step 3: Write the failing conformance test**

`src/tests.rs`:

```rust
use std::sync::Arc;

use finstack_ai_runtime::{Observer, ObserverBackpressure, ObserverPayloadMode};
use finstack_ai_test::check_observer_conformance;

use super::{NotifyObserver, tests_support};

#[tokio::test]
async fn conformance_accepts_an_empty_batch() {
    let observer = NotifyObserver::try_new(
        tests_support::capturing_sink().0,
        super::DeliveryPolicy::default(),
        8,
        ObserverBackpressure::DropProgress,
    )
    .expect("observer");
    assert_eq!(
        observer.descriptor().payload_mode,
        ObserverPayloadMode::Full
    );
    check_observer_conformance(&observer, Arc::from([]))
        .await
        .expect("conformance");
}
```

For Task 1 only, define a minimal capturing sink in a `#[cfg(test)] pub(crate) mod tests_support` inside `lib.rs` (moved/extended in Task 3):

```rust
#[cfg(test)]
pub(crate) mod tests_support {
    use std::sync::{Arc, Mutex};

    use finstack_ai_runtime::PortFuture;

    use super::{InteractionNotification, NotificationSink, SinkError};

    pub(crate) struct CapturingSink {
        pub(crate) seen: Arc<Mutex<Vec<InteractionNotification>>>,
    }

    impl NotificationSink for CapturingSink {
        fn name(&self) -> &'static str {
            "capturing"
        }

        fn deliver(
            &self,
            notification: InteractionNotification,
        ) -> PortFuture<Result<(), SinkError>> {
            let seen = Arc::clone(&self.seen);
            Box::pin(async move {
                if let Ok(mut guard) = seen.lock() {
                    guard.push(notification);
                }
                Ok(())
            })
        }
    }

    pub(crate) fn capturing_sink()
    -> (Arc<CapturingSink>, Arc<Mutex<Vec<InteractionNotification>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(CapturingSink {
            seen: Arc::clone(&seen),
        });
        (sink, seen)
    }
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo nextest run -p finstack-ai-observer-notify`
Expected: compile failure — `NotifyObserver`, `NotificationSink`, etc. not defined.

- [ ] **Step 5: Minimal implementation in `lib.rs`**

```rust
//! HITL interaction lifecycle notifier. Announce-only; never resolves.

// [exact lint header block copied from finstack-ai-observer-metrics/src/lib.rs:3-22]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use finstack_ai_kernel::{
    ComponentId, ComponentRef, InteractionId, Metadata, RunEvent, RunId, SessionId, Timestamp,
    Version,
};
use finstack_ai_runtime::{
    OBSERVER_QUEUE_OVERFLOW, Observer, ObserverBackpressure, ObserverDescriptor,
    ObserverDiagnostic, ObserverError, ObserverPayloadMode, ObserverQueue, ObserverQueuePush,
    PortFuture,
};
use serde::Serialize;
use thiserror::Error;

/// Notify-observer construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NotifyObserverError {
    /// Configuration is malformed.
    #[error("notify_observer_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Outbound sink delivery failure. Reasons are stable and never echo payloads.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SinkError {
    /// The sink endpoint could not be reached or rejected the notification.
    #[error("notify_sink_unavailable: {reason}")]
    Unavailable {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Which lifecycle event a notification announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionEventKind {
    /// A human interaction was requested.
    Requested,
    /// The interaction was resolved.
    Resolved,
    /// The interaction expired.
    Expired,
    /// The interaction was cancelled.
    Cancelled,
}

/// Whitelisted projection of one interaction event. Placeholder until Task 2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionNotification {
    /// Lifecycle event announced.
    pub event: InteractionEventKind,
    /// Interaction identity.
    pub interaction_id: InteractionId,
    /// Session the interaction belongs to.
    pub session_id: SessionId,
    /// Run the interaction belongs to.
    pub run_id: RunId,
    /// Event timestamp.
    pub timestamp: Timestamp,
}

/// Object-safe outbound notification sink.
pub trait NotificationSink: Send + Sync + 'static {
    /// Stable sink name for diagnostics.
    fn name(&self) -> &'static str;

    /// Deliver one notification. Must be side-effect-only; the observer
    /// ignores everything but the error for retry accounting.
    fn deliver(&self, notification: InteractionNotification) -> PortFuture<Result<(), SinkError>>;
}

/// Bounded per-notification delivery policy. Placeholder clamps land in Task 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryPolicy {
    request_timeout: Duration,
    max_attempts: u32,
    retry_backoff: Duration,
}

impl Default for DeliveryPolicy {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(5),
            max_attempts: 3,
            retry_backoff: Duration::from_millis(500),
        }
    }
}

/// Announce-only interaction lifecycle observer.
pub struct NotifyObserver {
    descriptor: ObserverDescriptor,
    sink: Arc<dyn NotificationSink>,
    policy: DeliveryPolicy,
    queue: ObserverQueue<InteractionNotification>,
    delivered: AtomicU64,
    failed: AtomicU64,
    dropped: AtomicU64,
    diagnostic: Mutex<Option<ObserverDiagnostic>>,
}

impl NotifyObserver {
    /// Construct a notify observer over one sink.
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity or queue bound.
    pub fn try_new(
        sink: Arc<dyn NotificationSink>,
        policy: DeliveryPolicy,
        queue_capacity: usize,
        backpressure: ObserverBackpressure,
    ) -> Result<Self, NotifyObserverError> {
        Ok(Self {
            descriptor: ObserverDescriptor {
                component: ComponentRef::new(
                    ComponentId::parse("finstack.observer.notify").map_err(|_| {
                        NotifyObserverError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    Some(Version {
                        major: 0,
                        minor: 0,
                        patch: 1,
                    }),
                ),
                payload_mode: ObserverPayloadMode::Full,
                metadata: Metadata::empty(),
            },
            sink,
            policy,
            queue: ObserverQueue::try_new(queue_capacity, backpressure).map_err(|_| {
                NotifyObserverError::Configuration {
                    reason: "invalid_queue_capacity",
                }
            })?,
            delivered: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            diagnostic: Mutex::new(None),
        })
    }

    /// Notifications delivered successfully.
    #[must_use]
    pub fn delivered(&self) -> u64 {
        self.delivered.load(Ordering::Relaxed)
    }

    /// Notifications dropped after exhausting delivery attempts.
    #[must_use]
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }

    /// Notifications dropped by queue backpressure.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed) + self.queue.dropped()
    }

    /// Last stored diagnostic.
    #[must_use]
    pub fn last_diagnostic(&self) -> Option<ObserverDiagnostic> {
        self.diagnostic.lock().ok().and_then(|slot| *slot)
    }
}

impl Observer for NotifyObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let _ = batch;
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests;
```

Plus the `tests_support` module from Step 3. Field/method dead-code warnings under `-D warnings`: `sink`, `policy`, `delivered`, `failed` are unused until Task 3 — reference them from `dropped()`/a private `fn _hold(&self)` is ugly; instead have Task 1's `observe()` already do the queue push of nothing and add `#[allow(dead_code)]` on nothing — the clean fix is: Task 1 keeps `sink`/`policy` out of the struct and `try_new` takes-and-drops them (`let _ = (sink, policy);` documented as task-1 scaffolding), then Task 3 adds the fields. Choose that; the public signature is already final.

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo nextest run -p finstack-ai-observer-notify` then `cargo test -p finstack-ai-observer-notify --doc` and `cargo clippy -p finstack-ai-observer-notify --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 7: Stub README** (two lines: crate purpose + "This crate is a T1 native adapter. It is not isolated.")

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/observers/finstack-ai-observer-notify docs/superpowers/specs/2026-08-20-hitl-notify-observer-design.md docs/superpowers/plans/2026-08-20-hitl-notify-observer.md
git commit -m "feat: scaffold finstack-ai-observer-notify observer leaf"
```

---

### Task 2: Redacted projection of the four interaction events

**Files:**
- Create: `extensions/observers/finstack-ai-observer-notify/src/project.rs`
- Modify: `src/lib.rs` (replace placeholder `InteractionNotification`; `mod project;` + re-exports)
- Modify: `src/tests.rs`

**Interfaces:**
- Consumes: `RunEvent` accessors `session_id()`, `run_id()`, `timestamp()`, `body()`; kernel types `InteractionRequest`, `InteractionResolution`, `InteractionExpired`, `InteractionCancelled`, `InteractionKind`, `AssigneeHint`, `PrincipalRef`.
- Produces: `pub fn project(event: &RunEvent) -> Option<InteractionNotification>` plus final `InteractionNotification`, `NotificationDetail`, `AssigneeLabel`, `PrincipalLabel` (shapes in spec §4 — copy them verbatim, all `Serialize` with `#[serde(rename_all = "snake_case")]`; `NotificationDetail` uses `#[serde(tag = "detail_kind", rename_all = "snake_case")]`).

- [ ] **Step 1: Write the failing tests** (in `src/tests.rs`; the event fixture helpers `id::<T>(n)` and `event(RunEventBody)` are copied from `extensions/observers/finstack-ai-observer-metrics/src/tests.rs:19-46`, using `Sensitivity::Internal` for interaction bodies)

```rust
const CANARY: &str = "CANARY_SECRET_VALUE";

fn requested_body() -> RunEventBody {
    let prompt = vec![ContentBlock::Text(TextBlock {
        text: Arc::from(format!("please approve {CANARY}")),
        annotations: None,
    })];
    let request = InteractionRequest::try_new(
        1,
        id::<InteractionTag>(7),
        id::<EffectTag>(8),
        InteractionKind::Approval,
        prompt,
        RawJson::parse(&format!("{{\"marker\":\"{CANARY}\"}}")).expect("schema"),
        ComponentRef::new(ComponentId::parse("policy.approval").expect("component"), None),
        Version { major: 1, minor: 0, patch: 0 },
        Some(AssigneeHint::Role(Arc::from("risk-desk"))),
        Some(Timestamp::from_unix_ms(9_000).expect("ts")),
        true,
        Metadata::empty(),
    )
    .expect("request");
    RunEventBody::InteractionRequested(request)
}

#[test]
fn projection_whitelists_requested_fields_and_drops_prompt_and_schema() {
    let notification = super::project(&event(requested_body())).expect("projected");
    assert_eq!(notification.event, InteractionEventKind::Requested);
    match &notification.detail {
        NotificationDetail::Requested { kind, assignee, expires_at, delegatable } => {
            assert_eq!(kind.as_ref(), "approval");
            assert!(matches!(assignee, Some(AssigneeLabel::Role(role)) if role.as_ref() == "risk-desk"));
            assert!(expires_at.is_some());
            assert!(delegatable);
        }
        other => panic!("wrong detail: {other:?}"),
    }
    let serialized = serde_json::to_string(&notification).expect("json");
    assert!(!serialized.contains(CANARY));
}

#[test]
fn projection_redacts_resolution_response_and_keeps_labels() {
    let resolution = InteractionResolution::try_new(
        id::<InteractionTag>(7),
        "resolution-1",
        PrincipalRef::try_new("oidc", "reviewer-9", None::<&str>).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        RawJson::parse(&format!("{{\"secret\":\"{CANARY}\"}}")).expect("json"),
        Some("looks-fine"),
    )
    .expect("resolution");
    let notification =
        super::project(&event(RunEventBody::InteractionResolved(resolution))).expect("projected");
    let serialized = serde_json::to_string(&notification).expect("json");
    assert!(serialized.contains("reviewer-9"));
    assert!(serialized.contains("looks-fine"));
    assert!(!serialized.contains(CANARY));
    assert!(!serialized.contains("decision-v1")); // authorization evidence excluded
}

#[test]
fn projection_covers_expired_and_cancelled_and_ignores_other_events() {
    let expired = InteractionExpired {
        interaction_id: id::<InteractionTag>(7),
        expired_at: Timestamp::from_unix_ms(9_500).expect("ts"),
    };
    assert!(super::project(&event(RunEventBody::InteractionExpired(expired))).is_some());

    let cancelled = InteractionCancelled::try_new(
        id::<InteractionTag>(7), None, None, Some("timeout"),
    )
    .expect("cancelled");
    let notification =
        super::project(&event(RunEventBody::InteractionCancelled(cancelled))).expect("projected");
    assert!(matches!(
        notification.detail,
        NotificationDetail::Cancelled { principal: None, reason: Some(ref r) } if r.as_ref() == "timeout"
    ));

    assert!(super::project(&event(RunEventBody::QueueDepthWarning(
        QueueDepthWarning { depth: 3, limit: 8 }
    )))
    .is_none());
}

#[test]
fn custom_kind_label_and_principal_assignee_project_safely() {
    // Same requested fixture but InteractionKind::Custom { name: "escalation" } and
    // AssigneeHint::Principal(PrincipalRef::try_new("oidc", "user-1", Some("tenant")).unwrap()).
    // Assert kind == "escalation"; assignee is AssigneeLabel::Principal with issuer "oidc",
    // subject "user-1"; serialized output does NOT contain "tenant" (tenant_scope excluded).
}
```

(Write the fourth test out fully — the comment describes the exact assertions.)

- [ ] **Step 2: Run to verify failure** — `cargo nextest run -p finstack-ai-observer-notify` fails: `project` undefined.

- [ ] **Step 3: Implement `src/project.rs`**

```rust
//! Structural redaction: RunEvent -> whitelisted InteractionNotification.

use std::sync::Arc;

use finstack_ai_kernel::{
    AssigneeHint, InteractionKind, PrincipalRef, RunEvent, RunEventBody,
};

use crate::{
    AssigneeLabel, InteractionEventKind, InteractionNotification, NotificationDetail,
    PrincipalLabel,
};

fn principal_label(principal: &PrincipalRef) -> PrincipalLabel {
    PrincipalLabel {
        issuer: Arc::from(principal.issuer()),
        subject: Arc::from(principal.subject()),
    }
}

fn kind_label(kind: &InteractionKind) -> Arc<str> {
    match kind {
        InteractionKind::Approval => Arc::from("approval"),
        InteractionKind::Choice => Arc::from("choice"),
        InteractionKind::Form => Arc::from("form"),
        InteractionKind::FreeText => Arc::from("free_text"),
        InteractionKind::Review => Arc::from("review"),
        InteractionKind::Correction => Arc::from("correction"),
        InteractionKind::Custom { name } => Arc::clone(name),
    }
}

fn assignee_label(hint: &AssigneeHint) -> AssigneeLabel {
    match hint {
        AssigneeHint::Principal(principal) => AssigneeLabel::Principal(principal_label(principal)),
        AssigneeHint::Role(role) => AssigneeLabel::Role(Arc::clone(role)),
        AssigneeHint::Queue(queue) => AssigneeLabel::Queue(Arc::clone(queue)),
    }
}

/// Project one run event into a notification. Non-interaction events map to `None`.
#[must_use]
pub fn project(event: &RunEvent) -> Option<InteractionNotification> {
    let (event_kind, interaction_id, detail) = match event.body() {
        RunEventBody::InteractionRequested(request) => (
            InteractionEventKind::Requested,
            request.interaction_id(),
            NotificationDetail::Requested {
                kind: kind_label(request.kind()),
                assignee: request.assignee_hint().map(assignee_label),
                expires_at: request.expires_at(),
                delegatable: request.delegatable(),
            },
        ),
        RunEventBody::InteractionResolved(resolution) => (
            InteractionEventKind::Resolved,
            resolution.interaction_id(),
            NotificationDetail::Resolved {
                resolution_id: Arc::from(resolution.resolution_id()),
                principal: principal_label(resolution.principal()),
                comment: resolution.comment().map(Arc::from),
            },
        ),
        RunEventBody::InteractionExpired(expired) => (
            InteractionEventKind::Expired,
            expired.interaction_id,
            NotificationDetail::Expired { expired_at: expired.expired_at },
        ),
        RunEventBody::InteractionCancelled(cancelled) => (
            InteractionEventKind::Cancelled,
            cancelled.interaction_id(),
            NotificationDetail::Cancelled {
                principal: cancelled.principal().map(principal_label),
                reason: cancelled.reason().map(Arc::from),
            },
        ),
        _ => return None,
    };
    Some(InteractionNotification {
        event: event_kind,
        interaction_id,
        session_id: event.session_id(),
        run_id: event.run_id(),
        timestamp: event.timestamp(),
        detail,
    })
}
```

(Verify the exact accessor names on `InteractionCancelled` — `principal()` / `reason()` per `crates/finstack-ai-kernel/src/effects/interaction.rs:559-581`; `expires_at()` returns `Option<Timestamp>` by value or reference — adjust `.copied()` as the compiler demands.) In `lib.rs`, replace the placeholder struct with the spec §4 shapes and re-export `project` as `pub use project::project;`.

- [ ] **Step 4: Run tests to verify pass** — `cargo nextest run -p finstack-ai-observer-notify`, clippy clean.

- [ ] **Step 5: Commit** — `git commit -m "feat: redacted interaction notification projection"`

---

### Task 3: Bounded queue + delivery loop with retry policy

**Files:**
- Modify: `src/lib.rs` (`DeliveryPolicy::try_new` with clamps; wire `sink`/`policy` fields into `NotifyObserver`; real `observe()`; `NOTIFY_DELIVERY_FAILED` const)
- Modify: `src/tests.rs` (move/extend `tests_support`: add `FailingSink { fail_first: AtomicU64 }` that errors N times then succeeds, recording attempt count)

**Interfaces:**
- Consumes: `project()` (Task 2), `ObserverQueue` push/drain, `tokio::time::{sleep, timeout}`.
- Produces: final `observe()` behavior; `pub const NOTIFY_DELIVERY_FAILED: ObserverDiagnostic`.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn observe_delivers_interaction_events_and_ignores_the_rest() {
    let (sink, seen) = tests_support::capturing_sink();
    let observer = NotifyObserver::try_new(
        sink, DeliveryPolicy::default(), 8, ObserverBackpressure::DropProgress,
    ).expect("observer");
    observer.observe(Arc::from([
        event(requested_body()),
        event(RunEventBody::QueueDepthWarning(QueueDepthWarning { depth: 3, limit: 8 })),
    ])).await.expect("observe");
    assert_eq!(seen.lock().expect("lock").len(), 1);
    assert_eq!(observer.delivered(), 1);
    assert_eq!(observer.failed(), 0);
}

#[tokio::test]
async fn delivery_retries_then_records_failure_diagnostic() {
    // FailingSink that always fails; policy with max_attempts 2, retry_backoff 1ms, timeout 50ms.
    let policy = DeliveryPolicy::try_new(
        Duration::from_millis(50), 2, Duration::from_millis(1),
    ).expect("policy");
    let sink = Arc::new(tests_support::FailingSink::always());
    let observer = NotifyObserver::try_new(
        Arc::clone(&sink) as Arc<dyn NotificationSink>, policy, 8,
        ObserverBackpressure::DropProgress,
    ).expect("observer");
    observer.observe(Arc::from([event(requested_body())])).await.expect("observe");
    assert_eq!(sink.attempts(), 2);
    assert_eq!(observer.failed(), 1);
    assert_eq!(observer.delivered(), 0);
    assert_eq!(observer.last_diagnostic().expect("diag").code, "notify_delivery_failed");
}

#[tokio::test]
async fn queue_overflow_drops_and_stores_overflow_diagnostic() {
    // capacity 1, DropProgress; observe a batch of 3 requested events.
    // delivered + dropped() must equal 3; if dropped() > 0 the stored diagnostic
    // after overflow (and before any delivery failure) is OBSERVER_QUEUE_OVERFLOW.
}

#[test]
fn delivery_policy_clamps_are_enforced() {
    assert!(DeliveryPolicy::try_new(Duration::from_millis(50), 0, Duration::ZERO).is_err()); // attempts < 1
    assert!(DeliveryPolicy::try_new(Duration::from_millis(1), 3, Duration::ZERO).is_err());  // timeout < 100ms
    assert!(DeliveryPolicy::try_new(Duration::from_secs(61), 3, Duration::ZERO).is_err());   // timeout > 60s
    assert!(DeliveryPolicy::try_new(Duration::from_secs(5), 6, Duration::ZERO).is_err());    // attempts > 5
    assert!(DeliveryPolicy::try_new(Duration::from_secs(5), 3, Duration::from_secs(11)).is_err()); // backoff > 10s
    assert!(DeliveryPolicy::try_new(Duration::from_secs(5), 3, Duration::from_millis(500)).is_ok());
}
```

(Write the overflow test fully.) `FailingSink`:

```rust
pub(crate) struct FailingSink {
    attempts: std::sync::atomic::AtomicU64,
}

impl FailingSink {
    pub(crate) fn always() -> Self {
        Self { attempts: std::sync::atomic::AtomicU64::new(0) }
    }

    pub(crate) fn attempts(&self) -> u64 {
        self.attempts.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl NotificationSink for FailingSink {
    fn name(&self) -> &'static str { "failing" }

    fn deliver(&self, _n: InteractionNotification) -> PortFuture<Result<(), SinkError>> {
        self.attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Box::pin(async { Err(SinkError::Unavailable { reason: "scripted_failure" }) })
    }
}
```

- [ ] **Step 2: Run to verify failure** (delivery not implemented; `try_new` on policy missing).

- [ ] **Step 3: Implement**

`DeliveryPolicy::try_new` — reject (stable reasons `invalid_request_timeout`, `invalid_max_attempts`, `invalid_retry_backoff`): timeout outside `100ms..=60s`, attempts outside `1..=5`, backoff `> 10s`.

`NOTIFY_DELIVERY_FAILED`:

```rust
/// Diagnostic stored when a notification exhausts its delivery attempts.
pub const NOTIFY_DELIVERY_FAILED: ObserverDiagnostic = ObserverDiagnostic {
    code: "notify_delivery_failed",
    detail: "notification dropped after exhausting sink delivery attempts",
};
```

`observe()` — push then drain then deliver inside the returned future:

```rust
fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
    for event in batch.iter() {
        let Some(notification) = crate::project(event) else { continue };
        match self.queue.push(notification) {
            Ok(ObserverQueuePush::Accepted) => {}
            Ok(ObserverQueuePush::Dropped) => self.record(OBSERVER_QUEUE_OVERFLOW, &self.dropped),
            Err(error) => {
                self.record(OBSERVER_QUEUE_OVERFLOW, &self.dropped);
                return Box::pin(async move { Err(error) });
            }
        }
    }
    let pending = match self.queue.drain() {
        Ok(pending) => pending,
        Err(error) => return Box::pin(async move { Err(error) }),
    };
    let sink = Arc::clone(&self.sink);
    let policy = self.policy.clone();
    let delivered = /* Arc-ify counters: change delivered/failed to Arc<AtomicU64>,
                       diagnostic to Arc<Mutex<Option<ObserverDiagnostic>>>, so the
                       'static future can update them */
    Box::pin(async move {
        for notification in pending {
            let mut attempt = 0_u32;
            loop {
                attempt += 1;
                let outcome = tokio::time::timeout(
                    policy.request_timeout,
                    sink.deliver(notification.clone()),
                )
                .await;
                match outcome {
                    Ok(Ok(())) => {
                        delivered.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                    Ok(Err(_)) | Err(_) if attempt < policy.max_attempts => {
                        tokio::time::sleep(policy.retry_backoff).await;
                    }
                    Ok(Err(_)) | Err(_) => {
                        failed.fetch_add(1, Ordering::Relaxed);
                        if let Ok(mut slot) = diagnostic.lock() {
                            *slot = Some(NOTIFY_DELIVERY_FAILED);
                        }
                        break;
                    }
                }
            }
        }
        Ok(())
    })
}
```

Concretely: change struct fields to `delivered: Arc<AtomicU64>`, `failed: Arc<AtomicU64>`, `diagnostic: Arc<Mutex<Option<ObserverDiagnostic>>>` (accessors unchanged), add private helper `fn record(&self, diag: ObserverDiagnostic, counter: &AtomicU64)` that bumps the counter and stores the diagnostic. Also wire the real `sink`/`policy` fields now (removing the Task 1 `let _ = (sink, policy);` scaffolding).

- [ ] **Step 4: Run tests to verify pass**; clippy clean; `check_observer_conformance` still green.

- [ ] **Step 5: Commit** — `git commit -m "feat: bounded delivery queue with retry policy for notify observer"`

---

### Task 4: Generic webhook sink

**Files:**
- Create: `src/webhook.rs`
- Modify: `src/lib.rs` (`mod webhook; pub use webhook::WebhookSink;`)
- Modify: `src/tests.rs`

**Interfaces:**
- Consumes: `SecretString` (`finstack_ai_runtime::SecretString` — confirm the re-export path; it lives at `ports/model/provider_util/secret.rs`; if not re-exported at crate root, use the same import path the openrouter provider uses), `NotificationSink`, `SinkError`.
- Produces: `WebhookSink::try_new(url: SecretString, request_timeout: Duration) -> Result<Self, NotifyObserverError>`; POSTs `serde_json::to_vec(&notification)` with `content-type: application/json`; treats any non-2xx status, connect error, or serialization failure as `SinkError::Unavailable` with reasons `http_status_error` / `http_request_failed` / `serialize_failed`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn webhook_sink_debug_never_leaks_the_url() {
    let sink = WebhookSink::try_new(
        SecretString::try_new("https://hooks.example.com/T000/SECRETPART").expect("url"),
        Duration::from_secs(5),
    )
    .expect("sink");
    let rendered = format!("{sink:?}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("SECRETPART"));
    assert!(!rendered.contains("hooks.example.com"));
}

#[test]
fn webhook_sink_rejects_non_http_urls() {
    for bad in ["ftp://x.example/hook", "not a url", "file:///etc/passwd"] {
        assert!(WebhookSink::try_new(
            SecretString::try_new(bad).expect("secret"),
            Duration::from_secs(5),
        )
        .is_err());
    }
}

#[tokio::test]
async fn webhook_sink_posts_notification_json_to_loopback() {
    let (address, received) = tests_support::spawn_loopback_http(200).await;
    let sink = WebhookSink::try_new(
        SecretString::try_new(&format!("http://{address}/hook")).expect("url"),
        Duration::from_secs(5),
    )
    .expect("sink");
    let notification = super::project(&event(requested_body())).expect("projected");
    sink.deliver(notification).await.expect("delivered");
    let body = received.lock().expect("lock").clone().expect("request captured");
    assert!(body.contains("\"event\":\"requested\""));
    assert!(body.contains("\"kind\":\"approval\""));
    assert!(!body.contains(CANARY));
}

#[tokio::test]
async fn webhook_sink_maps_server_errors_to_unavailable() {
    let (address, _received) = tests_support::spawn_loopback_http(500).await;
    let sink = WebhookSink::try_new(
        SecretString::try_new(&format!("http://{address}/hook")).expect("url"),
        Duration::from_secs(5),
    )
    .expect("sink");
    let notification = super::project(&event(requested_body())).expect("projected");
    let error = sink.deliver(notification).await.expect_err("must fail");
    assert_eq!(error, SinkError::Unavailable { reason: "http_status_error" });
}
```

`spawn_loopback_http` in `tests_support` (tokio only, no new deps): bind `tokio::net::TcpListener` on `127.0.0.1:0`, spawn a task that accepts one connection, reads until it has `content-length` bytes past the blank line, stores the body in `Arc<Mutex<Option<String>>>`, writes `HTTP/1.1 {status} OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n`, returns `(local_addr, body_slot)`.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement `src/webhook.rs`**

```rust
//! Generic JSON webhook sink. The URL is a bearer credential.

use core::fmt;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::{PortFuture, SecretString};
use reqwest::redirect::Policy;

use crate::{InteractionNotification, NotificationSink, NotifyObserverError, SinkError};

/// POSTs each notification as canonical JSON to a fixed URL.
pub struct WebhookSink {
    client: reqwest::Client,
    url: SecretString,
}

impl WebhookSink {
    /// Construct a webhook sink.
    ///
    /// # Errors
    ///
    /// Rejects a non-http(s) or unparseable URL and HTTP-client build failures.
    pub fn try_new(
        url: SecretString,
        request_timeout: Duration,
    ) -> Result<Self, NotifyObserverError> {
        let parsed = reqwest::Url::parse(url.expose())
            .map_err(|_| NotifyObserverError::Configuration { reason: "invalid_sink_url" })?;
        if parsed.scheme() != "https" && parsed.scheme() != "http" {
            return Err(NotifyObserverError::Configuration { reason: "invalid_sink_url_scheme" });
        }
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(request_timeout)
            .build()
            .map_err(|_| NotifyObserverError::Configuration { reason: "http_client_build_failed" })?;
        Ok(Self { client, url })
    }

    fn post_json(&self, body: Vec<u8>) -> PortFuture<Result<(), SinkError>> {
        let client = self.client.clone();
        let url = Arc::from(self.url.expose());
        Box::pin(async move {
            let response = client
                .post(&*url as &str)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await
                .map_err(|_| SinkError::Unavailable { reason: "http_request_failed" })?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(SinkError::Unavailable { reason: "http_status_error" })
            }
        })
    }
}

impl fmt::Debug for WebhookSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebhookSink")
            .field("url", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl NotificationSink for WebhookSink {
    fn name(&self) -> &'static str {
        "webhook"
    }

    fn deliver(&self, notification: InteractionNotification) -> PortFuture<Result<(), SinkError>> {
        match serde_json::to_vec(&notification) {
            Ok(body) => self.post_json(body),
            Err(_) => Box::pin(async { Err(SinkError::Unavailable { reason: "serialize_failed" }) }),
        }
    }
}
```

(If `SecretString` is not re-exported from `finstack_ai_runtime`'s root, import it the way `extensions/providers/finstack-ai-provider-openrouter` does and note the path in the commit message.)

- [ ] **Step 4: Run tests to verify pass**; clippy clean.

- [ ] **Step 5: Commit** — `git commit -m "feat: generic webhook sink with redacted URL handling"`

---

### Task 5: Slack sink and verify-middleware pairing

**Files:**
- Create: `src/slack.rs`
- Modify: `src/lib.rs` (`mod slack; pub use slack::{SlackSink, slack_text};`)
- Modify: `src/tests.rs`

**Interfaces:**
- Consumes: `WebhookSink`-style HTTP internals (duplicate the small client setup rather than exposing shared private helpers across sink modules — or move `post_json` into a private `mod http` shared by both; choose the shared private module, `src/http.rs`, and refactor `WebhookSink` onto it in this task).
- Produces: `SlackSink::try_new(webhook_url: SecretString, request_timeout: Duration)`; `pub fn slack_text(notification: &InteractionNotification) -> String`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn slack_text_renders_each_lifecycle_event() {
    let requested = super::project(&event(requested_body())).expect("projected");
    let text = slack_text(&requested);
    assert!(text.contains("Interaction requested"));
    assert!(text.contains("approval"));
    assert!(text.contains("risk-desk"));
    assert!(!text.contains(CANARY));

    // Build resolved/expired/cancelled notifications from the Task 2 fixtures and assert
    // "resolved by oidc/reviewer-9", "expired", "cancelled (timeout)" appear respectively.
}

#[test]
fn verify_review_interaction_produces_a_review_notification() {
    // Pairing test with finstack-ai-middleware-verify's contract: verify raises a
    // Review-kind interaction at before_finalize. Build requested_body() with
    // InteractionKind::Review and assert slack_text() contains "review" and the
    // notification detail kind label is "review".
}

#[tokio::test]
async fn slack_sink_posts_text_payload() {
    let (address, received) = tests_support::spawn_loopback_http(200).await;
    let sink = SlackSink::try_new(
        SecretString::try_new(&format!("http://{address}/services/T0/B0/x")).expect("url"),
        Duration::from_secs(5),
    )
    .expect("sink");
    sink.deliver(super::project(&event(requested_body())).expect("projected"))
        .await
        .expect("delivered");
    let body = received.lock().expect("lock").clone().expect("request captured");
    assert!(body.starts_with("{\"text\":"));
    assert!(!body.contains(CANARY));
}

#[test]
fn slack_sink_debug_never_leaks_the_url() {
    // Same shape as the webhook Debug test.
}
```

(Write the elided tests out fully.)

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement**

`src/http.rs` (private): `pub(crate) struct JsonPoster { client: reqwest::Client, url: SecretString }` with `try_new(url, request_timeout)` (URL/scheme validation + client build — moved verbatim from Task 4) and `post(&self, body: Vec<u8>) -> PortFuture<Result<(), SinkError>>`. Refactor `WebhookSink` to wrap `JsonPoster`.

`src/slack.rs`:

```rust
//! Slack incoming-webhook sink.

use core::fmt;
use std::fmt::Write as _;
use std::time::Duration;

use finstack_ai_runtime::{PortFuture, SecretString};

use crate::http::JsonPoster;
use crate::{
    AssigneeLabel, InteractionEventKind, InteractionNotification, NotificationDetail,
    NotificationSink, NotifyObserverError, SinkError,
};

/// Render one notification as a single-line Slack message.
#[must_use]
pub fn slack_text(notification: &InteractionNotification) -> String {
    let mut text = String::new();
    let _ = match notification.event {
        InteractionEventKind::Requested => write!(text, "Interaction requested"),
        InteractionEventKind::Resolved => write!(text, "Interaction resolved"),
        InteractionEventKind::Expired => write!(text, "Interaction expired"),
        InteractionEventKind::Cancelled => write!(text, "Interaction cancelled"),
    };
    let _ = write!(
        text,
        " — interaction {} (session {}, run {})",
        notification.interaction_id, notification.session_id, notification.run_id
    );
    match &notification.detail {
        NotificationDetail::Requested { kind, assignee, expires_at, delegatable } => {
            let _ = write!(text, " | kind: {kind}");
            match assignee {
                Some(AssigneeLabel::Principal(principal)) => {
                    let _ = write!(text, " | assignee: {}/{}", principal.issuer, principal.subject);
                }
                Some(AssigneeLabel::Role(role)) => { let _ = write!(text, " | role: {role}"); }
                Some(AssigneeLabel::Queue(queue)) => { let _ = write!(text, " | queue: {queue}"); }
                None => {}
            }
            if expires_at.is_some() { let _ = write!(text, " | expires"); }
            if *delegatable { let _ = write!(text, " | delegatable"); }
        }
        NotificationDetail::Resolved { resolution_id, principal, comment } => {
            let _ = write!(
                text, " | resolved by {}/{} ({resolution_id})", principal.issuer, principal.subject
            );
            if let Some(comment) = comment { let _ = write!(text, " | comment: {comment}"); }
        }
        NotificationDetail::Expired { .. } => {}
        NotificationDetail::Cancelled { principal, reason } => {
            if let Some(principal) = principal {
                let _ = write!(text, " | by {}/{}", principal.issuer, principal.subject);
            }
            if let Some(reason) = reason { let _ = write!(text, " ({reason})"); }
        }
    }
    text
}

/// Slack incoming-webhook sink; the webhook URL is a bearer credential.
pub struct SlackSink {
    poster: JsonPoster,
}

impl SlackSink {
    /// Construct a Slack sink.
    ///
    /// # Errors
    ///
    /// Rejects invalid URLs and HTTP-client build failures.
    pub fn try_new(
        webhook_url: SecretString,
        request_timeout: Duration,
    ) -> Result<Self, NotifyObserverError> {
        Ok(Self { poster: JsonPoster::try_new(webhook_url, request_timeout)? })
    }
}

impl fmt::Debug for SlackSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SlackSink")
            .field("webhook_url", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl NotificationSink for SlackSink {
    fn name(&self) -> &'static str {
        "slack"
    }

    fn deliver(&self, notification: InteractionNotification) -> PortFuture<Result<(), SinkError>> {
        let payload = serde_json::json!({ "text": slack_text(&notification) });
        match serde_json::to_vec(&payload) {
            Ok(body) => self.poster.post(body),
            Err(_) => Box::pin(async { Err(SinkError::Unavailable { reason: "serialize_failed" }) }),
        }
    }
}
```

(`interaction_id`/`session_id`/`run_id` are `Id<T>`; if they don't implement `Display`, format via their `Debug`/`to_string` equivalent used in the metrics leaf — `effect_id.to_string()` exists there, so `Id` implements `Display` or `ToString`; match whatever compiles without `unwrap`.)

- [ ] **Step 4: Run tests to verify pass**; clippy clean.

- [ ] **Step 5: Commit** — `git commit -m "feat: slack sink and verify-review pairing coverage"`

---

### Task 6: README, baselines, full gates

**Files:**
- Modify: `extensions/observers/finstack-ai-observer-notify/README.md`
- Create (generated): `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-observer-notify.txt`

- [ ] **Step 1: Write the README** — must cover, in this order:
  1. One-paragraph purpose: announce-only HITL notifier; cannot resolve interactions or change the run; distinct from inbound completion (workflow-worker inbox) and any HITL router.
  2. **Payload-mode justification:** declares `ObserverPayloadMode::Full` because interaction bodies derive `Sensitivity::Internal`; redaction is structural via the `InteractionNotification` whitelist; list the excluded fields verbatim from spec §2; note the canary tests.
  3. **Best-effort doctrine** (billing README wording): bounded queue, bounded retries, notifications may be dropped under backpressure or sink failure; authoritative interaction state lives in journal records.
  4. Pairing with `finstack-ai-middleware-verify` (verify raises the `Review` interaction; this crate announces it; they are deliberately separate crates).
  5. Usage snippet (doc-tested style):

```rust
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_observer_notify::{DeliveryPolicy, NotifyObserver, SlackSink};
use finstack_ai_runtime::{ObserverBackpressure, SecretString};

let sink = SlackSink::try_new(
    SecretString::try_new("https://hooks.slack.com/services/T0/B0/example").expect("url"),
    Duration::from_secs(5),
)
.expect("sink");
let observer = NotifyObserver::try_new(
    Arc::new(sink),
    DeliveryPolicy::default(),
    256,
    ObserverBackpressure::DropProgress,
)
.expect("observer");
let _ = observer.delivered();
```

  6. Trust tier line: "This crate is a T1 native adapter. It is not isolated."

- [ ] **Step 2: Regenerate public-api baselines**

```bash
uv run --no-project python scripts/compat/public_api.py --write
mise run check-public-api
```

Expected: a new baseline file for `finstack-ai-observer-notify` only; `git diff --stat fixtures/` must show no other baseline changed.

- [ ] **Step 3: Full gates**

```bash
mise run check-rust
mise run test-rust
```

Expected: all green. Fix anything that isn't before proceeding (workspace clippy is `-D warnings`).

- [ ] **Step 4: Commit**

```bash
git add extensions/observers/finstack-ai-observer-notify/README.md fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-observer-notify.txt
git commit -m "chore: notify observer README and public-api baseline"
```

- [ ] **Step 5: Finish** — use superpowers:finishing-a-development-branch (branch `feature/notify-observer`, base `main`).

---

## Self-review notes (already applied)

- Spec §2 whitelist ↔ Task 2 `project()` fields match one-to-one; canary tests cover prompt, schema, response, authorization, tenant_scope.
- `NotificationSink::deliver` takes an **owned** `InteractionNotification` everywhere (trait, capturing sink, failing sink, both HTTP sinks) because `PortFuture` is `'static`.
- Counter types change in Task 3 (plain `AtomicU64` → `Arc<AtomicU64>`) — accessor signatures (`delivered()`, `failed()`, `dropped()`, `last_diagnostic()`) never change.
- Email sink is a spec §5 non-goal; the public `NotificationSink` trait is the extension point.
- Zero kernel/runtime edits anywhere in the plan; root `Cargo.toml` and the new crate are the only non-doc modifications.
