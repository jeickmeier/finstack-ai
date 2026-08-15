# Native developer-preview examples

Four offline, secret-free binaries over the public Rust surface.
Workspace version is **0.1.0** (tag `v0.1.0`). `publish = false`.

Trust class: [T1](../../docs/site/security-trust-levels.md). Native
in-process providers and tools are not isolated.

- `minimal` completes a model-only run through the OpenAI-compatible provider.
- `coding` composes calculator, filesystem, shell, repository/memory context,
  sliding-window compaction, and a before_finalize verifier over a keyless
  loopback model.
- `service` resolves once, checks component health, and handles one request.
- `diagnostic` prints credential-free `AgentSpec` and lock fingerprints.

## Quick start

```bash
cargo run -p finstack-ai-native-examples --bin minimal --offline --locked
```

See [docs/site/rust.md](../../docs/site/rust.md).
