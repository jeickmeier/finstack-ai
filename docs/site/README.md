# finstack-ai 0.0.2 alpha candidate

The current staged candidate contains the deterministic microkernel,
commit-before-effect runtime, public SDK facade, OpenAI-compatible provider,
calculator, capability-scoped filesystem tools, and the Python binding half of
the planned `0.0.2` alpha. The exact checkpoint is not cut until the browser
WASM half and G4 evidence pass in PR-038.

Start with the four offline examples in `examples/rust-minimal`. They require no
credential and send provider requests only to a process-local loopback server.
For a real endpoint, construct `OpenAiCompatibleConfig` with an HTTPS base URL
and pass credentials only through `Authentication`; never put secret material
in `AgentSpec`, bundle configuration, a resolution lock, logs, or source files.

This candidate does not promise browser WASM parity, durable database storage,
or a stable plugin ABI. Those remain later gated work.

## Local verification

```text
mise run test-pr032
mise run preview-performance
mise run preview-stage
```

The stage is written to `target/preview/0.0.2-alpha-candidate` and is not
published, tagged, or represented as a passed gate.
