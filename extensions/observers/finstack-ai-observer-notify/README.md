# finstack-ai-observer-notify

Announce-only HITL notifier. Watches the four interaction lifecycle events —
`InteractionRequested`, `InteractionResolved`, `InteractionExpired`,
`InteractionCancelled` — and delivers redacted notifications to an outbound
sink (Slack incoming webhook or generic JSON webhook). It implements only the
read-only `Observer` port: it **cannot resolve interactions and cannot change
the run**. It is distinct from inbound resolution delivery (the workflow-worker
inbox) and from any workflow HITL router; routing, assignment, reminders, and
SLOs belong to an application or work-item service.

## Payload mode and redaction

Interaction event bodies derive `Sensitivity::Internal`, so this observer
declares `ObserverPayloadMode::Full` — a deliberate break from the
metadata-only default of the other observer leaves. Redaction is structural: a
sink only ever receives an `InteractionNotification`, a closed whitelist of
identifiers and label-validated strings (interaction/session/run ids,
timestamps, interaction kind label, assignee hint labels, resolution id,
principal issuer/subject, comment/reason labels, expiry, delegatability).
The following are excluded forever: prompt content blocks, response schemas,
resolution response JSON, authorization evidence, metadata, digests, and
policy identity. Canary tests assert none of them can reach a sink payload.

Sink URLs (a Slack webhook URL is a bearer credential) are held as
`SecretString`; `Debug` renders `[REDACTED]` and errors carry stable
`&'static str` reasons only.

## Best-effort delivery

Notifications are a convenience projection, not a record: the queue is
bounded (`ObserverQueue`, overflow counted via `dropped()` with the
`observer_queue_overflow` diagnostic), delivery uses a bounded
timeout-and-retry `DeliveryPolicy` (default 5 s timeout, 3 attempts, 500 ms
backoff; the policy is the single owner of the request timeout, and a
timed-out attempt is never retried since the endpoint may already have
received it), and a notification that exhausts its attempts is dropped with
the `notify_delivery_failed` diagnostic. Delivery runs in a spawned,
FIFO-ordered background task so a slow sink never stalls the observer's
event subscription. `ObserverBackpressure::BlockBounded` is rejected at
construction — the queue's only consumer is this observer's own drain, so
blocking for capacity cannot succeed; use `DropProgress` or `Disconnect`.
Authoritative interaction state lives in journal records; a missed
notification never gates a run.

## Pairing with `finstack-ai-middleware-verify`

The verify middleware raises a `Review`-kind interaction at
`before_finalize`; this observer announces it like any other interaction
request. They are deliberately separate crates: middleware must stay pure
with respect to external state (recovery re-runs the chain), so outbound
network sends live here, never in verify.

## Usage

```rust
use std::sync::Arc;

use finstack_ai_observer_notify::{DeliveryPolicy, NotifyObserver, SlackSink};
use finstack_ai_runtime::{ObserverBackpressure, SecretString};

let sink = SlackSink::try_new(
    SecretString::try_new("https://hooks.slack.com/services/T0/B0/example").expect("url"),
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

Attach it with `AgentBuilder::observer(component_ref, Arc::new(observer))`.
The public `NotificationSink` trait is the extension point for other
transports (e.g. email); no SMTP sink ships in this crate.

This crate is a T1 native adapter. It is not isolated.
