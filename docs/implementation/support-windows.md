# Support windows (operational)

Owner: `me@jeickmeier.com`. Adopter page:
[`docs/site/support.md`](../site/support.md). Extends, and does not
replace, [`SECURITY.md`](../../SECURITY.md) and
[`preview-compatibility-policy.md`](preview-compatibility-policy.md).

Status: written by PR-065. Not `G8-D-*`. Not an LTS promise.

## Windows

| Line | Window |
| --- | --- |
| Current minor (`1.0.x` at GA) | Patches until the next minor |
| Previous minor | Security-only until the next minor ships or 90 days, whichever is shorter |
| `0.1.x` preview | Supported until `1.0.0`; then security-only for 90 days |
| Historical snapshots | Not supported |

## Maintenance-branch procedure

Do **not** create or push `release/1.0` in this repository until a
later sentence names that external action (PR-066).

1. At GA, cut `release/1.0` from the tagged `1.0.0` commit.
2. Hotfix: cherry-pick onto that branch, bump the patch, restage,
   recreate checksums (`mise run recreate-release` and
   `mise run hotfix-rehearsal`).
3. Do not merge unrelated Phase 9 work into a maintenance branch.
4. Consumers who cannot take the patch stay pinned to the previous
   checksum (rollback). Maintainers publish the hotfix as the next
   patch. Do not rewrite published bytes.

## Related

- [release-engineering.md](release-engineering.md)
- [1.0-compatibility-policy.md](1.0-compatibility-policy.md)
