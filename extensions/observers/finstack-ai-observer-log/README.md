# finstack-ai-observer-log

Structured JSON observer leaf. Default payload mode is `Redacted`. Export
queues are finite; overflow emits `observer_queue_overflow` and never fails a
run. `write_support_bundle` writes redacted events, metadata-only journal
export, and versions — not raw journal CBOR.
