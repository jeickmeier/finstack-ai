# HITL notifier observer design: `finstack-ai-observer-notify`

- **Date:** 2026-08-20
- **Status:** Approved for planning
- **Source of truth for the shape:** `docs/planning/05-finstack-ai-future-capabilities-design-validation.md` §9.3 (interaction lifecycle), §9.6 (inboxes/notifications live outside the kernel), §9.7 ("workflow graphs, inboxes, notifications, and business state remain outside the kernel"), §18.7 (observers are best-effort); observer contract in `crates/finstack-ai-runtime/src/ports/observer/mod.rs`; house template: `docs/superpowers/specs/2026-08-20-billing-observer-design.md`.

## 1. Overview

A read-only observer leaf crate that watches the four interaction lifecycle
events — `InteractionRequested`, `InteractionResolved`, `InteractionExpired`,
`InteractionCancelled` — and announces them to an outbound sink (Slack
incoming webhook or a generic JSON webhook). It **cannot resolve interactions
and cannot change the run**: it implements only the object-safe `Observer`
port, whose contract already guarantees "observers never change behavior or
terminal state; delivery is batched; failures are isolated from the run."

```text
finstack-ai-observer-notify (extensions/observers/finstack-ai-observer-notify)
  NotifyObserver           — Observer port adapter (component id finstack.observer.notify)
  InteractionNotification  — whitelisted, redacted-by-construction projection of one event
  NotificationSink         — object-safe outbound sink trait (public; apps may add email etc.)
  WebhookSink              — generic JSON POST sink (reqwest, no redirects, SecretString URL)
  SlackSink                — Slack incoming-webhook sink ({"text": ...} payload)
  DeliveryPolicy           — per-notification timeout + bounded retry policy
```

Distinct from and deliberately not merged with:

- **Inbound completion** (backlog #3, `docs/superpowers/specs/2026-08-19-workflow-worker-design.md` §7 inbox): that component *accepts* resolutions. This crate only announces outbound.
- **Workflow HITL router** (backlog #20): routing/assignment is an application concern (§9.6).
- **`finstack-ai-middleware-verify`**: verify *raises* a `Review` interaction at `before_finalize`; this crate *announces* it. They pair (a notification test covers the `Review` kind) but stay separate crates — middleware must stay pure w.r.t. external state (recovery re-runs the chain), so network sends can never live in verify.

## 2. Redaction rules (non-negotiable)

Interaction event bodies derive `Sensitivity::Internal`
(`crates/finstack-ai-kernel/src/events/derive.rs:350`), so this observer must
declare `ObserverPayloadMode::Full` to read them — a deliberate break from the
`MetadataOnly`/`Redacted` default of the other observer leaves. That is only
acceptable because redaction is **structural**: the sink never sees a
`RunEvent`, only an `InteractionNotification`, which is a closed whitelist.

**Included** (identifiers and label-validated strings only):

| Event | Fields |
|---|---|
| all | event name, `interaction_id`, `session_id`, `run_id`, event timestamp |
| requested | interaction kind wire label (incl. validated custom label), assignee hint (role/queue label, or principal issuer+subject labels), `expires_at`, `delegatable` |
| resolved | `resolution_id` (label), principal issuer+subject (labels), `comment` (label-validated by `InteractionResolution::try_new`) |
| expired | `expired_at` |
| cancelled | optional principal issuer+subject, `reason` (label-validated) |

**Excluded, forever:** prompt content blocks, `response_schema`, the
resolution `response` JSON, `AuthorizationEvidence`, `Metadata`, digests,
policy component/version, every non-interaction event body. Canary tests
plant a marker string in each excluded field and assert the serialized sink
payload never contains it.

**Secrets:** a Slack/webhook URL is a bearer credential. It is held as
`SecretString` (runtime provider-util type), hand-written `Debug` renders
`[REDACTED]`, and sink errors carry only `&'static str` reasons — never the
URL, never response bodies.

## 3. Delivery semantics

- **Best-effort and lossy by construction** (§18.7). Ingest pushes
  notifications onto a bounded `ObserverQueue<InteractionNotification>`
  (capacity clamped `1..=1_000_000`); overflow increments `dropped()` and
  stores `OBSERVER_QUEUE_OVERFLOW`. Correctness-critical facts live in
  journal records; a missed Slack ping must never gate a run.
- **Bounded retry, in-crate.** There is no reusable retry utility in the
  workspace (proposal FR-07 is unimplemented), so `DeliveryPolicy` owns it:
  `request_timeout` (default 5 s, clamped 100 ms..=60 s), `max_attempts`
  (default 3, clamped 1..=5), fixed `retry_backoff` (default 500 ms, clamped
  0..=10 s). After the final failed attempt the notification is dropped,
  `failed()` increments, and `NOTIFY_DELIVERY_FAILED` is stored. No
  unbounded queues, no infinite retries, no re-enqueue loops.
- Delivery happens inside the future returned by `observe()`; each observer
  already runs in its own spawned subscription task, so slow sinks delay only
  this observer, never the run.

## 4. Public API (complete)

```rust
pub enum NotifyObserverError { Configuration { reason: &'static str } } // code notify_observer_configuration_invalid

pub enum InteractionEventKind { Requested, Resolved, Expired, Cancelled }

pub struct PrincipalLabel { pub issuer: Arc<str>, pub subject: Arc<str> }

pub enum AssigneeLabel { Principal(PrincipalLabel), Role(Arc<str>), Queue(Arc<str>) }

pub enum NotificationDetail {
    Requested { kind: Arc<str>, assignee: Option<AssigneeLabel>,
                expires_at: Option<Timestamp>, delegatable: bool },
    Resolved  { resolution_id: Arc<str>, principal: PrincipalLabel, comment: Option<Arc<str>> },
    Expired   { expired_at: Timestamp },
    Cancelled { principal: Option<PrincipalLabel>, reason: Option<Arc<str>> },
}

pub struct InteractionNotification {   // Serialize, snake_case, deny nothing extra in
    pub event: InteractionEventKind,
    pub interaction_id: InteractionId,
    pub session_id: SessionId,
    pub run_id: RunId,
    pub timestamp: Timestamp,
    pub detail: NotificationDetail,
}

pub enum SinkError { Unavailable { reason: &'static str } }  // code notify_sink_unavailable

pub trait NotificationSink: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn deliver(&self, notification: InteractionNotification) -> PortFuture<Result<(), SinkError>>;
}

pub struct DeliveryPolicy { /* private */ }
impl DeliveryPolicy {
    pub fn try_new(request_timeout: Duration, max_attempts: u32, retry_backoff: Duration)
        -> Result<Self, NotifyObserverError>;
}
impl Default for DeliveryPolicy { /* 5s / 3 / 500ms */ }

pub struct NotifyObserver { /* descriptor, Arc<dyn NotificationSink>, DeliveryPolicy,
                               ObserverQueue<InteractionNotification>, AtomicU64 delivered/failed/dropped,
                               Mutex<Option<ObserverDiagnostic>> */ }
impl NotifyObserver {
    pub fn try_new(sink: Arc<dyn NotificationSink>, policy: DeliveryPolicy,
                   queue_capacity: usize, backpressure: ObserverBackpressure)
        -> Result<Self, NotifyObserverError>;
    pub fn delivered(&self) -> u64;
    pub fn failed(&self) -> u64;
    pub fn dropped(&self) -> u64;
    pub fn last_diagnostic(&self) -> Option<ObserverDiagnostic>;
}
impl Observer for NotifyObserver { /* descriptor(), observe() */ }

pub const NOTIFY_DELIVERY_FAILED: ObserverDiagnostic; // code "notify_delivery_failed"

pub struct WebhookSink { /* reqwest client + SecretString url */ } // Debug redacts url
impl WebhookSink { pub fn try_new(url: SecretString, request_timeout: Duration) -> Result<Self, NotifyObserverError>; }
impl NotificationSink for WebhookSink { /* POST canonical JSON of the notification */ }

pub struct SlackSink { /* reqwest client + SecretString webhook url */ } // Debug redacts url
impl SlackSink { pub fn try_new(webhook_url: SecretString, request_timeout: Duration) -> Result<Self, NotifyObserverError>; }
impl NotificationSink for SlackSink { /* POST {"text": slack_text(&n)} */ }

pub fn slack_text(notification: &InteractionNotification) -> String; // pure, unit-tested
```

HTTP conventions (workspace-mandatory): `reqwest` 0.13.2 workspace pin,
`redirect(Policy::none())`, client-level timeout, http(s) URLs only. A
`vendored-tls` feature forwards to `reqwest/native-tls-vendored`.

## 5. Non-goals

- **Resolving interactions or any inbound path.** No server, no callback
  endpoint, no `InteractionResolutionCommand`. That is workflow-worker
  territory (#3).
- **Routing, assignment, reminders, SLOs, dedup across restarts** — §9.6
  application/work-item-service territory (#20).
- **Email sink in v1.** No SMTP client exists in the workspace and adding
  `lettre` is a dependency decision to make separately; `NotificationSink`
  is public so an application can supply one today.
- **Retry durability.** In-memory bounded retry only; guaranteed delivery
  would re-derive from journal records externally.
- **Kernel/runtime changes.** Zero. Existing `Observer` port only; no
  seventh port.

## 6. Crate placement and dependencies

`extensions/observers/finstack-ai-observer-notify`, workspace member +
`[workspace.dependencies]` entry (not re-exported by the SDK facade — no
observer leaf is; apps attach via `AgentBuilder::observer(...)`). Deps:
`finstack-ai-kernel`, `finstack-ai-runtime` (default-features off,
`native-tokio`), `reqwest`, `serde`, `serde_json`, `thiserror`,
`tokio` (`time`). Dev-deps: `finstack-ai-test`, `tokio` (`rt`, `macros`,
`net`, `io-util`, `time`). Same lint header as the metrics leaf; README
states the trust tier, the `Full` payload-mode justification, and the
best-effort doctrine. Public-api baseline regenerated via
`uv run --no-project python scripts/compat/public_api.py --write`
(`scripts/compat/public_api.py` auto-globs `extensions/*/**/Cargo.toml`).
No Python/JS binding exposure, so `public_items.py` is untouched.
