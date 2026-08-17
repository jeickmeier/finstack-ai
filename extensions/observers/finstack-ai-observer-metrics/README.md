# finstack-ai-observer-metrics

Reference Prometheus text observer. Call `encode_prometheus()`; this crate
does not start an HTTP server. Labels are identifiers and strategy IDs only.
`record_compaction` / `record_compaction_result` ignore replacement-message
and summary text.

This crate is a T1 native adapter. It is not isolated.

```rust
use finstack_ai_observer_metrics::MetricsObserver;
use finstack_ai_runtime::ObserverBackpressure;

let observer = MetricsObserver::try_new(64, ObserverBackpressure::DropProgress)
    .expect("metrics");
let _ = observer.encode_prometheus();
```
