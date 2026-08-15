# finstack-ai-observer-metrics

Reference Prometheus text observer. Call `encode_prometheus()`; this crate
does not start an HTTP server. Labels are identifiers and strategy IDs only.
`record_compaction` / `record_compaction_result` ignore replacement-message
and summary text.
