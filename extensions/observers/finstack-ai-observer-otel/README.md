# finstack-ai-observer-otel

OpenTelemetry observer leaf. Default build uses an in-process
`SdkTracerProvider` with no network exporter. Enable the `otlp` feature only
when an application installs an explicit OTLP exporter. Span names are
`finstack.run`, `finstack.effect`, and `finstack.tool`.

This crate is a T1 native adapter. It is not isolated.

```rust
use finstack_ai_observer_otel::OtelObserver;

let observer = OtelObserver::try_redacted(64, finstack_ai_runtime::ObserverBackpressure::DropProgress)
    .expect("otel");
```
