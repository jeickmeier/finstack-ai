# Python starter projects

Three deliberately small starters against the typed public `finstack_ai`
package. The workspace pins `finstack-ai==2.0.0`; build from the repository
until a 2.0 package is published.

- [`rust-backed/`](rust-backed/) — curated Rust-backed native Ollama provider
  (T1). Default path performs no
  network I/O.
- [`python-callback/`](python-callback/) — trusted Python callback
  (T2). Not isolated.
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
