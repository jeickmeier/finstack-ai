# Python callback starter

Trusted Python `PythonModel` callback through the Rust-owned kernel/runtime
loop. No network.

Trust class: T2. This is
trusted in-process code. It is not a sandbox and is not isolated.

Workspace version is **2.0.0 unpublished**. Pin: `finstack-ai==2.0.0`.

## Quick start

```bash
uv run --isolated --no-project --with-editable bindings/finstack-ai-python \
  python examples/python-minimal/python-callback/main.py
```
