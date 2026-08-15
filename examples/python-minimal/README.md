# Python starter projects

Three deliberately small starters against the typed public `finstack_ai`
package. Workspace pin is `finstack-ai==1.0.0` (unpublished; last public
tag `v0.1.0`; not on PyPI).

- [`rust-backed/`](rust-backed/) — curated Rust-backed OpenAI-compatible
  provider ([T1](../../docs/site/security-trust-levels.md)). Default path
  performs no network I/O.
- [`python-callback/`](python-callback/) — trusted Python callback
  ([T2](../../docs/site/security-trust-levels.md)). Not isolated.
- [`service/`](service/) — resolve once, health, one offline request (T2).

## Quick start

```bash
uv run --isolated --no-project --with-editable bindings/finstack-ai-python \
  python examples/python-minimal/python-callback/main.py
```

Build a staged wheel when you want the no-compiler consumer path:

```bash
uv build --project bindings/finstack-ai-python --out-dir target/python-dist
uv run --isolated --no-project --with target/python-dist/*.whl \
  python examples/python-minimal/python-callback/main.py
```
