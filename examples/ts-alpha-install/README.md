# Clean TypeScript install

This example typechecks against a packed `@finstack/ai` tarball. It does not
map repository `src/` or `js/dist` paths. Do not embed provider credentials.

Trust class: T2 when host
adapters are registered. Not isolated.

Workspace version is **2.0.0** unpublished (last public tag `v0.1.0`; not on npm).

## Quick start

```bash
mise run build-wasm -- release
uv run --no-project python scripts/wasm_package/stage.py
# stage.py also installs this example from the packed tarball and runs tsc
```

Persistence, live providers, and crash durability are out of scope.
