# Native examples

Four offline, secret-free binaries over the public Rust surface.
Workspace manifests are staged at **2.0.0**; this crate remains
`publish = false`.

Trust class: T1. Native
in-process providers and tools are not isolated.

- `minimal` completes a model-only run through the native Ollama provider.
- `coding` composes calculator, filesystem, shell, repository/memory context,
  sliding-window compaction, and a before_finalize verifier over a keyless
  loopback model. Sliding-window `CompactContext` lands. Summarize compaction
  completes via the runtime-owned phase; see
  how summarize compaction completes.
- `service` resolves once, checks component health, and handles one request.
- `diagnostic` prints credential-free `AgentSpec` and lock fingerprints.

## Quick start

```bash
cargo run -p finstack-ai-native-examples --bin minimal --offline --locked
```
