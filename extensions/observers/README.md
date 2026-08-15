# Observer batteries

Trusted native observer leaves. They format `ObserverEventView` projections
and never own compaction or change run semantics.

## Packages

- `finstack-ai-observer-log` — one JSON object per event, stderr or a caller
  `Write`, default `ObserverPayloadMode::Redacted`
- `finstack-ai-observer-metrics` — Prometheus text exposition, default
  `MetadataOnly` labels
- `finstack-ai-observer-otel` — in-process OpenTelemetry spans; optional
  `otlp` feature, no default network exporter

`NoopObserver` and `ReferenceObserver` stay in `finstack-ai-runtime` for tests.

## Redaction

Default streams, diagnostic JSON/JSONL, support bundles, and journal *export*
projections hide `Secret` / `Credential` bodies and omit journal `body` unless
the caller opts into `Full`. Authoritative journal CBOR is unchanged.

## Trace example

Compose a scripted model with `ReferenceObserver` plus the log adapter:

```text
Agent::builder(...)
    .observer(log_ref, Arc::new(LogObserver::try_redacted(writer, 32, DropProgress)?))
    .build()
```

A completed calculator/coding loop emits `RunAccepted` … `RunCompleted`. The
log line for each event carries session/run/effect IDs, `class`, `kind`, and
`sensitivity`. Public bodies may appear; secret tool payloads do not. Observer
`Err` or a full export queue cannot change the journal prefix.

Do not add a Grafana or hosted collector example here.
