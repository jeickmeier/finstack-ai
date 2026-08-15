# Support windows

Adopter-facing support windows for finstack-ai. Operational detail
lives in
[support-windows.md](../implementation/support-windows.md).
This is not an LTS program and not `G8-D-*`.

Workspace crate, wheel, and npm version fields are unpublished
**1.0.0**. Tag `v0.1.0` exists. Tag `v1.0.0` and crates.io / PyPI /
npm wait on a later named sentence. Windows become in force for
`1.0.x` at GA. This is not LTS.

| Line | Window |
| --- | --- |
| Current minor (`1.0.x` at GA) | Patches until the next minor |
| Previous minor | Security-only until the next minor ships or 90 days, whichever is shorter |
| `0.1.x` preview | Supported until tagged `1.0.0` per the [preview policy](../implementation/preview-compatibility-policy.md); then security-only for 90 days |
| Historical snapshots (`0.0.4` and earlier) | Not supported |

Maintenance branches are cut at GA as `release/1.0` from the tagged
commit. This tree does not create or push that branch.
Hotfixes cherry-pick onto the maintenance line; they do not merge
unrelated Phase 9 work into it.

See [SECURITY.md](../../SECURITY.md) for the reporting path and
[release engineering](../implementation/release-engineering.md) for
rollback and hotfix procedure.
