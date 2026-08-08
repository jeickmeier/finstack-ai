# PR-004 validation artifacts

Logical PR: [PR-004](../../../planning/04-finstack-ai-implementation-plan.md#pr-004---record-foundational-adrs-and-schema-governance)

Branch: `pr-004-adrs-schema-governance`

Local and hosted validation for foundational ADR records and schema
governance. Logical PR-004 is `Done` at merge
`9b13fe02d4cf41305daa20195eb0a537f85f9712`
([#2](https://github.com/jeickmeier/finstack-ai/pull/2)).

A01–A05 are Passed against PR implementation tip
`75537daad821f33de4a1465fbf83aa784b710cc5`. Hosted CI was green on PR head
`7634342b93e57aae0f93e17fd685b2e954584c1a` (see `hosted-ci.txt`). Gate G0
stays `Not ready` until PR-005 completes.

## Commands

```bash
mise run test-schema-governance
mise run lint-schema-governance
mise run format-schema-governance
mise run schema-governance
SCHEMA_GOVERNANCE_BASE="$(git rev-parse main)" mise run schema-governance
mise run lint-workflows
mise run ci
```

## Acceptance mapping

| Criterion | Local proof |
| --- | --- |
| A01 | `schema-governance.txt` (GOV001/GOV002) + ADR files under `docs/implementation/adrs/` |
| A02 | `schema-governance.txt` (GOV004) + `schemas/schema-families.toml` (includes process family) |
| A03 | `schema-governance.txt` (GOV007) + `.github/PULL_REQUEST_TEMPLATE.md` |
| A04 | `test-schema-governance.txt` (version coupling, rename sources, WIT paths, registry) + `schema-governance-base-main.txt` |
| A05 | `security-review.txt` + GOV003 completeness |

Checksums: [`SHA256SUMS`](SHA256SUMS).
