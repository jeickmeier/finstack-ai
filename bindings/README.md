# Bindings

Language bindings over the one Rust-owned semantic engine. They convert
and host; they do not reimplement continuation, recovery, or event order.

| Package | Path | Guide |
| --- | --- | --- |
| `finstack-ai` (Python) | [`finstack-ai-python/`](finstack-ai-python/README.md) | [docs/site/python.md](../docs/site/python.md) |
| `@finstack/ai` (JS/WASM) | [`finstack-ai-wasm/`](finstack-ai-wasm/js/README.md) | [docs/site/wasm.md](../docs/site/wasm.md) |

Workspace version is **1.0.0**. PyPI and npm packages are not published.
Consume a staged wheel or tarball from this repository.

Python callbacks and JavaScript host adapters are
[T2](../docs/site/security-trust-levels.md). They inherit process or page
authority and are not a sandbox.
