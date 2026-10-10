# finstack-ai-observer-otel

OpenTelemetry observer leaf. Default build uses an in-process
`SdkTracerProvider` with no network exporter. Span names are
`finstack.run`, `finstack.effect`, and `finstack.tool`.
Captured spans are bounded to the configured in-memory capture capacity.
`OtelObserver::try_new_with_provider` accepts an operator-built
`SdkTracerProvider` so its processors and exporters (OTLP, stdout, a test
exporter) receive the same redacted spans. The observer never flushes the
provider on its own; call `force_flush` when a batching processor needs it.

This crate is a T1 native adapter. It is not isolated.

```rust
use finstack_ai_observer_otel::OtelObserver;

let observer = OtelObserver::try_redacted(64).expect("otel");
```
