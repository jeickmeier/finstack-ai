# finstack-ai native developer preview

The `0.0.1-dev` checkpoint is a Rust-first preview of the deterministic
microkernel, commit-before-effect runtime, public SDK facade, OpenAI-compatible
provider, calculator, and capability-scoped filesystem tools.

Start with the four offline examples in `examples/rust-minimal`. They require no
credential and send provider requests only to a process-local loopback server.
For a real endpoint, construct `OpenAiCompatibleConfig` with an HTTPS base URL
and pass credentials only through `Authentication`; never put secret material
in `AgentSpec`, bundle configuration, a resolution lock, logs, or source files.

This checkpoint does not promise Python/WASM parity, durable database storage,
or a stable plugin ABI. Those remain later gated work.

## Local verification

```text
mise run test-native-examples
mise run preview-performance
mise run preview-stage
```

The stage is written to `target/preview/0.0.1-dev` and is not published.
