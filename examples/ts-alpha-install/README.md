# Clean TypeScript install

This example typechecks against a packed `@finstack/ai` tarball. It does not
map repository `src/` or `js/dist` paths. Do not embed provider credentials.

```bash
mise run stage-wasm
# stage.py also installs this example from the packed tarball and runs tsc
```

Persistence, live providers, and crash durability are out of scope.
