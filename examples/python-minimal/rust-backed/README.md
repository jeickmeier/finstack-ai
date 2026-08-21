# Rust-backed Python starter

Constructs the curated Rust-backed native Ollama provider. The default path
performs no network I/O. Pass `--run` only with an explicit trusted endpoint.

Trust class: T1. Native
provider code is not isolated.

Workspace version is **2.0.0 unpublished**. Pin: `finstack-ai==2.0.0`.

## Quick start

```bash
uv run --isolated --no-project --with-editable bindings/finstack-ai-python \
  python examples/python-minimal/rust-backed/main.py
```
