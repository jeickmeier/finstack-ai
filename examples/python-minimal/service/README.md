# Python service starter

Resolve once, print `health()`, print the compact capability catalog, and
handle one offline request. Matches the Rust `service` binary shape.

Trust class: T2. This is
trusted in-process code. It is not isolated.

Workspace version is **1.0.0 unpublished**.

## Quick start

```bash
uv run --isolated --no-project --with-editable bindings/finstack-ai-python \
  python examples/python-minimal/service/main.py
```
