# Native developer-preview examples

This package contains four offline, secret-free binaries over the public Rust
surface:

- `minimal` completes a model-only run through the OpenAI-compatible provider.
- `coding` completes a calculator tool loop and shows the filesystem toolset's
  explicit-root, fail-closed platform policy.
- `service` resolves once, checks component health, and handles one request.
- `diagnostic` prints credential-free `AgentSpec` and lock fingerprints.

Run one with `cargo run -p finstack-ai-native-examples --bin minimal`.
