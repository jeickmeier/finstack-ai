# finstack-ai-observer-otel

OpenTelemetry observer leaf. Default build uses an in-process
`SdkTracerProvider` with no network exporter. Enable the `otlp` feature only
when an application installs an explicit OTLP exporter. Span names are
`finstack.run`, `finstack.effect`, and `finstack.tool`.
