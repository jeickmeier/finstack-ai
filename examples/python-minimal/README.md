# Python starter projects

PR-032 provides two deliberately small starters against the typed public
`finstack_ai` package:

- [`rust-backed/`](rust-backed/) constructs the curated Rust-backed
  OpenAI-compatible provider. Its default smoke path performs no network I/O;
  pass `--run` only with an explicit trusted endpoint.
- [`python-callback/`](python-callback/) runs an entirely offline trusted Python
  callback through the Rust-owned kernel/runtime loop.

Build the local candidate and run either starter against it:

```bash
uv build --project bindings/finstack-ai-python --out-dir target/python-dist
uv run --isolated --no-project --with target/python-dist/*.whl \
  python examples/python-minimal/rust-backed/main.py
```

The callback starter is trusted in-process code. It is not an isolation
boundary. Neither starter claims durable restart support; PR-048 owns that
gate.
