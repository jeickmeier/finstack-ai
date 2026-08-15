# Release rehearsal (tagged 0.1.0)

This is a local preview rehearsal. It does **not** publish to crates.io,
PyPI, or npm. G7 passed via `G7-D-public-preview-f7c7e70b9e04`. Tag
`v0.1.0` is cut. Registry publish remains blocked on owner credentials.

```text
mise run release-rehearsal
```

The runner executes two clean staging passes from the same tree with
`SOURCE_DATE_EPOCH=0` and compares SHA-256 files.

## Existing staging paths

| Artifact | Command |
| --- | --- |
| Python sdist | `uv build --sdist --project bindings/finstack-ai-python` |
| Python wheel | in-tree `maturin build --offline --locked` (PEP 517 wheel-from-sdist fails with `locked = true` outside the workspace) |
| npm tarball + CycloneDX | `mise run stage-wasm` (requires generated WASM) or `npm pack` when `dist/` exists |
| Crate metadata | `cargo package --list --locked --offline` on public crates |
| WIT/plugin bits | already staged under `plugins/`; hashed when present |

Two-run identity is required for sdist, crate package lists, `plugin.lock.json`,
and npm pack when `dist/` exists. Wheel bytes are recorded but may differ
across maturin/rustc embeddings; that residual is an accepted G7 residual,
not a rehearsal failure.

Provenance statement names the commit SHA, `mise.toml` toolchain pins,
`version: 0.1.0`, and `staged_not_published: true` for registry
artifacts. The git tag `v0.1.0` is a separate identity from those
staged bytes.

Hosted Sigstore/OIDC is not required (`external actions=none`).
