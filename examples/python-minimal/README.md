# Python starter projects

Three deliberately small starters against the typed public `finstack_ai`
package. Workspace pin is `finstack-ai==1.0.0` (unpublished; last public
tag `v0.1.0`; not on PyPI).

- [`rust-backed/`](rust-backed/) — curated Rust-backed native Ollama provider
  (T1). Default path performs no
  network I/O.
- [`python-callback/`](python-callback/) — trusted Python callback
  (T2). Not isolated.
- [`service/`](service/) — resolve once, health, one offline request (T2).
- [`notebooks/`](notebooks/) — seven-notebook learning series. 01–04 stay
  offline. 05 runs local Ollama when `gemma4:26b` (or `OLLAMA_MODEL`) is
  installed. 06–07 run live providers only when the matching API key is
  set.

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
