# Public GA roadmap

In-repo issue roadmap after the unpublished `1.0.0` candidate. Opening
GitHub issues, tagging `v1.0.0`, publishing registries, and recording
`G8-D-*` are external actions and are not claimed here.

## In force at GA (after a later named sentence)

- Lockstep crate, wheel, and npm version fields are `1.0.0`.
- WIT permanent worlds are `finstack:ai-*@1.0.0`.
- Support windows in [`support-windows.md`](support-windows.md) apply
  to `1.0.x`. This is not LTS.
- `0.1.x` preview becomes security-only for 90 days after tagged
  `1.0.0`.

## Post-1.0 in-repo themes

These stay in-repo work. They are not this PR and are not a G8 pass.

- Independent first-party leaf versions only after named coupling harm
  ([`1.0-leaf-versioning.md`](1.0-leaf-versioning.md)).
- WIT `@0.x` guest retarget of published experimental components.
- Process session vocabulary.
- IndexedDB durability (today inspect-not-continue).
- SharedArrayBuffer / cross-origin isolation (ADR-031).
- Maintenance branch `release/1.0` after a named tag.

## Explicit exclusions

The following stay **excluded**. This file does not promise them.

- Plugin marketplace
- Broad channel catalog
- Product-specific UIs
- Native dylib ABI
- PostgreSQL journal
- Exactly-once delivery

## Related

- [Public preview roadmap](public-preview-roadmap.md)
- [1.0 compatibility matrix](1.0-compatibility-matrix.md)
- [Support windows](support-windows.md)
