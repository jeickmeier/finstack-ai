# Python starter projects

PR-032 provides two deliberately small starters against the typed public
`finstack_ai` package:

- [`rust-backed/`](rust-backed/) constructs the curated Rust-backed
  OpenAI-compatible provider. Its default smoke path performs no network I/O;
  pass `--run` only with an explicit trusted endpoint.
- [`python-callback/`](python-callback/) runs an entirely offline trusted Python
  callback through the Rust-owned kernel/runtime loop.

Build the local candidate and validate both projects without a compiler:

```bash
mise run build-python
mise run test-python-starters
```

The callback starter is trusted in-process code. It is not an isolation
boundary. Neither starter claims durable restart support; PR-048 owns that
gate.
