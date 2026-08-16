# Support windows

Adopter-facing support windows for finstack-ai. This is not an LTS
program. Operational detail lives in
[support-windows.md](../implementation/support-windows.md).

Workspace crate, wheel, and npm version fields are **1.0.0**.
Local tag `v1.0.0` exists. The last pushed GitHub tag is `v0.1.0`.
crates.io / PyPI / npm stay unpublished. Windows are in force for
`1.0.x`. This is not LTS.

| Line | Window |
| --- | --- |
| Current minor (`1.0.x` at GA) | Patches until the next minor |
| Previous minor | Security-only until the next minor ships or 90 days, whichever is shorter |
| `0.1.x` preview | Security-only for 90 days after local tag `v1.0.0` (2026-08-15) per the [preview policy](../implementation/preview-compatibility-policy.md) |
| Historical snapshots (`0.0.4` and earlier) | Not supported |

Maintenance branches are cut at GA as `release/1.0` from the tagged
commit. This tree does not create or push that branch.
Hotfixes cherry-pick onto the maintenance line; they do not merge
unrelated Phase 9 work into it.

See [SECURITY.md](../../SECURITY.md) for the reporting path and
[release engineering](../implementation/release-engineering.md) for
rollback and hotfix procedure.
