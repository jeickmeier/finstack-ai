# Native developer-preview examples

This package contains four offline, secret-free binaries over the public Rust
surface:

- `minimal` completes a model-only run through the OpenAI-compatible provider.
- `coding` composes calculator, filesystem, shell, repository/memory context,
  sliding-window compaction, and a before_finalize verifier over a keyless
  loopback model.
- `service` resolves once, checks component health, and handles one request.
- `diagnostic` prints credential-free `AgentSpec` and lock fingerprints.

Run one with `cargo run -p finstack-ai-native-examples --bin minimal`.
