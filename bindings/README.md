# Bindings

Language bindings over the one Rust-owned semantic engine. They convert
and host; they do not reimplement continuation, recovery, or event order.

| Package | Guide |
| --- | --- |
| `finstack-ai` (Python) | [`finstack-ai-python/README.md`](finstack-ai-python/README.md) |
| `@finstack/ai` (JS/WASM) | [`finstack-ai-wasm/js/README.md`](finstack-ai-wasm/js/README.md) |

Workspace version is **2.0.0**. PyPI and npm packages are not published.
Consume a staged wheel or tarball from this repository.

Python callbacks and JavaScript host adapters are
T2. They inherit process or page
authority and are not a sandbox.
