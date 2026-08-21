# finstack-ai-observer-log

Structured JSON observer leaf. Default payload mode is `Redacted`. EventHub
owns bounded delivery; the observer writes each delivered batch directly.
`write_support_bundle` writes redacted events, metadata-only journal
export, and versions — not raw journal CBOR.

This crate is a T1 native adapter. It is not isolated.

```rust
use std::io::sink;
use std::sync::{Arc, Mutex};

use finstack_ai_observer_log::LogObserver;
let observer = LogObserver::try_redacted(Arc::new(Mutex::new(sink()))).expect("log");
```
