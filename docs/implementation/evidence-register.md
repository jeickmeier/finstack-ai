# Evidence register

This register records proof that implementation work satisfies the existing planning baseline. It does not restate acceptance criteria, test plans, security controls, or gate requirements.

No implementation evidence or gate decision is recorded at initialization.

## Identifier and coverage rules

Use stable references derived from the authoritative [Implementation Plan](../planning/04-finstack-ai-implementation-plan.md):

- logical-PR acceptance evidence: `PR-001-A01`, `PR-001-A02`, and so on, in source order;
- phase entrance criteria: `PH0-ENT-A01`, `PH0-ENT-A02`, and so on;
- phase exit criteria: `PH0-EXIT-A01`, `PH0-EXIT-A02`, and so on;
- evidence records: `<scope>-E-short-slug-xxxxxxxxxxxx`, such as `PR-018-E-linux-tests-a1b2c3d4e5f6`; and
- gate decisions: `<gate>-D-short-slug-xxxxxxxxxxxx`, such as `G4-D-binding-alpha-a1b2c3d4e5f6`.

The final 12 lowercase hexadecimal characters are generated randomly when the record is created. Scope plus a descriptive slug keeps references readable; the random suffix prevents allocation collisions across parallel branches. IDs are immutable and never reused.

An ordinal identifies the criterion at its linked version of the plan. Do not copy criterion prose into this register. If a plan amendment adds, removes, or reorders criteria, record the old-to-new ID mapping before updating coverage totals.

PLAN-0.6 began with 342 logical-PR acceptance-evidence bullets and 62 phase entrance/exit bullets. The current PLAN-0.10 inventory remains 345 logical-PR criteria and 62 phase criteria. Rows are added when their scope becomes active, keeping this register useful without maintaining a duplicate plan.

## Plan baseline and criterion migration

The current ordinal namespace is bound to this exact plan artifact:

| Baseline | Plan version | SHA-256 | Effective date | Logical PRs | PR criteria | Phase criteria | Amendment | State |
| --- | --- | --- | ---: | ---: | ---: | ---: | --- | --- |
| PLAN-0.6 | 0.6 | `237b624f53488cb625c598e2affaf0bae484e7ae33da26d06516135b14b93cff` | 2026-08-08 | 66 | 342 | 62 | Initial implementation baseline | Superseded |
| PLAN-0.8 | 0.8 | `0e3d7c824daf036b72981ebbf0c1ab46a0989f277ce2c3f5ae5be728d4b51fa1` | 2026-08-08 | 66 | 345 | 62 | Documentation packs v0.9-v0.10 reconciliation | Superseded |
| PLAN-0.9 | 0.9 | `6a0cc9a3887026ff902322f7051e9fb0a6050880bbc6ff91ebbeb9e5011e5262` | 2026-08-08 | 66 | 345 | 62 | Pack v0.11 `extensions/` workspace layout | Superseded |
| PLAN-0.10 | 0.10 | `fea2ea8b02a710a0edc655909dc9f6f1a2aae6e1996f657b26c9e3424bfd2916` | 2026-08-08 | 66 | 345 | 62 | Pack v0.12 centralized license layout | Current |

When a versioned amendment changes criterion order or inventory, append every affected mapping before updating delivery totals or acceptance rows. `Removed` and `Replaced` dispositions require the amendment that authorized the scope change.

| Date | Prior baseline | New baseline / digest | Old criterion | New criterion or disposition | Amendment | Reconciled by |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / `0e3d7c824daf036b72981ebbf0c1ab46a0989f277ce2c3f5ae5be728d4b51fa1` | PH0-ENT-A01 | PH0-ENT-A01 (clarified) | Pack v0.9 gate sequencing | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-001-A04 | PR-001-A04 (revised) | Pack v0.10 mise bootstrap | Pack v0.10 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | — | PR-005-A05 (added) | Pack v0.9 gate sign-off alignment | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-006-A02 | PR-006-A02 (revised) | Pack v0.9 metadata bounds | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-008-A08 | PR-008-A08 (revised) | Pack v0.9 record bounds | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-011-A02 | PR-011-A02 (revised) | Pack v0.9 cost encoding | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-016-A04 | PR-016-A04 (revised) | Pack v0.9 approval-policy floor | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-022-A12 | PR-022-A12 (revised) | Pack v0.9 artifact metadata contract | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | — | PR-026-A05 (added) | Pack v0.9 gate sign-off alignment | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | — | PR-038-A06 (added) | Pack v0.9 gate sign-off alignment | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-039-A02 | PR-039-A02 (revised) | Pack v0.9 cost projection | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-039-A03 | PR-039-A03 (revised) | Pack v0.9 CBOR boundaries | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-039-A04 | PR-039-A04 (revised) | Pack v0.9 decoder bounds | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-057-A02 | PR-057-A02 (revised) | Pack v0.9 redaction coverage | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.6 | PLAN-0.8 / same digest | PR-063-A02 | PR-063-A02 (revised) | Pack v0.9 performance ownership | Pack v0.9 amendment |
| 2026-08-08 | PLAN-0.8 | PLAN-0.9 / `6a0cc9a3887026ff902322f7051e9fb0a6050880bbc6ff91ebbeb9e5011e5262` | PR-001 principal changes | PR-001 principal changes (revised; acceptance IDs unchanged) | Pack v0.11 `extensions/` layout | Pack v0.11 amendment |
| 2026-08-08 | PLAN-0.9 | PLAN-0.10 / `fea2ea8b02a710a0edc655909dc9f6f1a2aae6e1996f657b26c9e3424bfd2916` | PR-001 principal changes | PR-001 principal changes (license paths revised; acceptance IDs unchanged) | Pack v0.12 centralized license layout | Pack v0.12 amendment |

## Acceptance dispositions

| Status | Required record |
| --- | --- |
| `Pending` | Owner and target evidence are identified. |
| `Passed` | One or more verified evidence IDs demonstrate the criterion at the reviewed commit. |
| `Failed` | Failure evidence and linked remediation work are recorded. |
| `Not applicable` | Written rationale and reviewer approval show why the criterion does not apply without changing scope. |
| `Waived` | An approved, unexpired exception and its compensating-control evidence are linked. |

`Not applicable` cannot be used to remove planned scope. `Waived` cannot be used for a prohibited exception or after its expiry.

## Acceptance coverage

| Criterion | Scope | Plan version / link | Status | Owner | Evidence | Exception | Reviewer | Reviewed date |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PR-001-A01 | PR-001 | PLAN-0.10 / [PR-001](../planning/04-finstack-ai-implementation-plan.md#pr-001---create-the-workspace-and-package-skeleton) | Passed | me@jeickmeier.com | PR-001-E-dep-direction-1fe94f769044 | — | me@jeickmeier.com | 2026-08-08 |
| PR-001-A02 | PR-001 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-001-E-cargo-check-3e6a85e8d27f | — | me@jeickmeier.com | 2026-08-08 |
| PR-001-A03 | PR-001 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-001-E-kernel-deps-1d8c0f41d158 | — | me@jeickmeier.com | 2026-08-08 |
| PR-001-A04 | PR-001 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-001-E-mise-doctor-cae2f2eb7918 | — | me@jeickmeier.com | 2026-08-08 |
| PR-001-A05 | PR-001 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-001-E-ownership-review-b14626f70259 | — | me@jeickmeier.com | 2026-08-08 |
| PR-001-A06 | PR-001 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-001-E-security-md-f70391db8ac2 | — | me@jeickmeier.com | 2026-08-08 |

PR-001 acceptance is closed against `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862`. Artifacts live under [`artifacts/pr-001/`](artifacts/pr-001/).

## Evidence records

| Evidence | Produced date | Scope | Type | Command, job, or review | Environment / target | Commit | Result | Artifact, log, or digest | Produced by | Verified by | Verified date | Supersedes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PR-001-E-dep-direction-1fe94f769044 | 2026-08-08 | PR-001 | Local command | `cargo metadata --format-version 1` + workspace edge assertion | Darwin arm64; rustc/cargo 1.97.1 (`environment.txt`) | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/`](artifacts/pr-001/) `dependency-direction.txt` sha256 `fe777c14e61c21f41535e465fa7eec585ec1b35facc4b63fe566e54ca607e026`; `cargo-metadata.json` sha256 `c64923139ab02812544da4e1bdf113bf68e5c647ab008da2a254fe505f1f682f` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-cargo-check-3e6a85e8d27f | 2026-08-08 | PR-001 | Local command | `cargo check --workspace --all-targets`; `cargo check -p finstack-ai-kernel` | Darwin arm64; rustc/cargo 1.97.1 | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/cargo-check.txt`](artifacts/pr-001/cargo-check.txt) sha256 `0d2dcea452af6e8fcab54f40cd8c92cdefff15c8b334ec2910e6412fa6211c26` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-kernel-deps-1d8c0f41d158 | 2026-08-08 | PR-001 | Local command | Kernel dependency inventory from `cargo metadata` (forbidden set empty) | Darwin arm64; rustc/cargo 1.97.1 | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/kernel-deps.txt`](artifacts/pr-001/kernel-deps.txt) sha256 `7dd89503a66614cafdd08c63050b00b28639e6b86f806c2d9471e77c533bc8ca` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-mise-doctor-cae2f2eb7918 | 2026-08-08 | PR-001 | Local command | `mise run install`; `mise run doctor` | Darwin arm64; mise tools rust 1.97.1, python 3.14.6, uv 0.10.11 | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/mise-install.txt`](artifacts/pr-001/mise-install.txt) sha256 `3d93bbf76a6716cd61c97a3f711a0a9bfcf1b2208ce0b0234eca8fc416ab4502`; [`mise-doctor.txt`](artifacts/pr-001/mise-doctor.txt) sha256 `b97d92e7d7636fd9e6a9ced0e9496c78cad2ab53fbaec1914a3fdaa84866fcae` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-ownership-review-b14626f70259 | 2026-08-08 | PR-001 | Manual review | Review `GOVERNANCE.md` + `CONTRIBUTING.md` ownership/DCO | Files at reviewed commit | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/ownership-security-review.txt`](artifacts/pr-001/ownership-security-review.txt) sha256 `6b41e0e37adb1d09c4a5c51613da432de89edcdcb120cd05980bbf013be0182e` (A05) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-security-md-f70391db8ac2 | 2026-08-08 | PR-001 | Manual review | Review `SECURITY.md` private path, supported-version placeholder, response owner vs Threat Model §14 | Files at reviewed commit | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/ownership-security-review.txt`](artifacts/pr-001/ownership-security-review.txt) sha256 `6b41e0e37adb1d09c4a5c51613da432de89edcdcb120cd05980bbf013be0182e` (A06) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |

An evidence record is valid only when another contributor can identify what ran or was reviewed, against which immutable revision, in which relevant environment, with what result, and where the durable output is stored. A bare statement such as “tests pass,” an unlinked local result, or evidence from a superseded commit cannot close acceptance.

Additional requirements apply by evidence type:

- CI evidence links the workflow, job, run, matrix target, and retained artifact or log.
- Local evidence records the exact command and material environment details; promote it to durable CI evidence when the plan requires CI.
- Benchmark evidence records the workload, baseline, threshold, hardware, samples, and report artifact.
- Security evidence names the applicable SEC-INV and TM controls and links the review or test output.
- Compatibility, schema, journal, protocol, binding, and conformance evidence identifies the fixture or version set exercised.
- Manual review evidence identifies reviewer, scope, decision, and any follow-up issue.

Large outputs belong in durable artifacts. This register stores their identity, link, and digest, not a pasted copy.

## Gate decision log

| Decision | Gate | Date | Result | Approver | Reviewed commit(s) | Evidence set | Open exceptions | Residual risk / conditions | Remediation |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

<!-- Example shape only; remove this comment when adding the first real row.
| GN-D-short-slug-xxxxxxxxxxxx | GN | YYYY-MM-DD | Passed/Failed | named approver | immutable commits | scoped evidence IDs | scoped exception ID or None | accepted conditions or None | issue/task or — |
-->

A passing decision requires the approver to verify the gate's plan requirements, phase exit evidence, applicable Security and Threat Model controls, compatibility obligations, and exception status. The decision row itself satisfies a final logical PR's gate-sign-off criterion; that criterion is not a prerequisite to recording the decision. The [delivery ledger](delivery-ledger.md) is updated to `Passed` only after this row exists.

## Corrections and retention

- Evidence rows are append-only. Correct an error with a new row that names `Supersedes`; retain the prior row.
- A failure remains part of the record after remediation. Link the later passing evidence rather than deleting the failure.
- Artifact retention must meet the planning baseline's audit, release, and security requirements.
- Moving a logical PR, phase, ADR, or gate to a complete state requires its evidence links to remain resolvable.
