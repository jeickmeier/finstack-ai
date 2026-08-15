# Support windows

Adopter-facing support windows for finstack-ai. Operational detail
lives in
[support-windows.md](../implementation/support-windows.md).
This is not an LTS program and not `G8-D-*`.

Workspace crate, wheel, and npm version fields stay **0.1.0** until
PR-066. Tag `v0.1.0` exists. crates.io / PyPI / npm are unpublished.

| Line | Window |
| --- | --- |
| Current minor (`1.0.x` after PR-066) | Patches until the next minor |
| Previous minor | Security-only until the next minor ships or 90 days, whichever is shorter |
| `0.1.x` preview | Supported until `1.0.0` per the [preview policy](../implementation/preview-compatibility-policy.md); then security-only for 90 days |
| Historical snapshots (`0.0.4` and earlier) | Not supported |

Maintenance branches are cut at GA (PR-066) as `release/1.0` from
the tagged commit. This tree does not create or push that branch.
Hotfixes cherry-pick onto the maintenance line; they do not merge
unrelated Phase 9 work into it.

See [SECURITY.md](../../SECURITY.md) for the reporting path and
[release engineering](../implementation/release-engineering.md) for
rollback and hotfix procedure.
