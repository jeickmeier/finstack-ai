# finstack-ai-observer-log

Structured JSON observer leaf. Default payload mode is `Redacted`. Export
queues are finite; overflow emits `observer_queue_overflow` and never fails a
run. `write_support_bundle` writes redacted events, metadata-only journal
export, and versions — not raw journal CBOR.

This crate is a T1 native adapter. It is not isolated.

```rust
use std::io::sink;
use std::sync::{Arc, Mutex};

use finstack_ai_observer_log::LogObserver;
use finstack_ai_runtime::ObserverBackpressure;

let observer = LogObserver::try_redacted(
    Arc::new(Mutex::new(sink())),
    64,
    ObserverBackpressure::DropProgress,
)
.expect("log");
```
