# Public GA roadmap

In-repo issue roadmap after local tag `v1.0.0`. Opening GitHub
issues, pushing the tag, publishing registries, and announcing remain
external actions and are not claimed here. G8 passed via
`G8-D-general-availability-a889a29a3f54`.

## In force at GA

- Lockstep crate, wheel, and npm version fields are `1.0.0`.
- WIT permanent worlds are `finstack:ai-*@1.0.0`.
- Support windows in [`support-windows.md`](support-windows.md) apply
  to `1.0.x`. This is not LTS.
- `0.1.x` preview becomes security-only for 90 days after tagged
  `1.0.0`.

## Post-1.0 in-repo themes

These stay in-repo work. They are not a later-gate claim.

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
