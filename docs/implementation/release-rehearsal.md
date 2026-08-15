# Release rehearsal (unpublished 0.1.0)

This is a local preview rehearsal. It does **not** `git tag` or publish.
Named G7 and `v0.1.0` remain owner decisions.

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
across maturin/rustc embeddings; that residual is **blocking for G7**, not
this rehearsal's local pass.

Provenance statement names the commit SHA, `mise.toml` toolchain pins,
`version: 0.1.0`, and `staged_not_published: true`. Artifacts may say
"preview rehearsal" and must not claim a named G7 or public tag.

Hosted Sigstore/OIDC is not required (`external actions=none`).
