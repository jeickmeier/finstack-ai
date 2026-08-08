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
| PR-002-A01 | PR-002 | PLAN-0.10 / [PR-002](../planning/04-finstack-ai-implementation-plan.md#pr-002---add-architecture-and-dependency-enforcement) | Passed | me@jeickmeier.com | PR-002-E-unit-tests-fc419e9d2920; PR-002-E-architecture-6ed3268e6ff5 | — | me@jeickmeier.com | 2026-08-08 |
| PR-002-A02 | PR-002 | PLAN-0.10 / same | Not applicable | me@jeickmeier.com | PR-002-E-review-627382618b20 | — | me@jeickmeier.com | 2026-08-08 |
| PR-002-A03 | PR-002 | PLAN-0.10 / same | Not applicable | me@jeickmeier.com | PR-002-E-review-627382618b20; PR-002-E-dep-direction-321ce9b2b4b4 | — | me@jeickmeier.com | 2026-08-08 |
| PR-002-A04 | PR-002 | PLAN-0.10 / same | Not applicable | me@jeickmeier.com | PR-002-E-review-627382618b20; PR-002-E-wasm-binding-5253da7a99f9 | — | me@jeickmeier.com | 2026-08-08 |
| PR-002-A05 | PR-002 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-002-E-architecture-6ed3268e6ff5 | — | me@jeickmeier.com | 2026-08-08 |
| PR-002-A06 | PR-002 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-002-E-review-627382618b20 | — | me@jeickmeier.com | 2026-08-08 |
| PR-002-A07 | PR-002 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-002-E-waiver-fa6bff499990 | — | me@jeickmeier.com | 2026-08-08 |
| PR-003-A01 | PR-003 | PLAN-0.10 / [PR-003](../planning/04-finstack-ai-implementation-plan.md#pr-003---establish-the-cross-platform-ci-and-release-build-matrix) | Pending | me@jeickmeier.com | Hosted PR workflow runs for `ci.yml`, `security.yml`, and `nightly.yml` with no unsafe path skips | — | — | — |
| PR-003-A02 | PR-003 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-003-E-channel-ownership-7d912e4d6ce2 | — | me@jeickmeier.com | 2026-08-08 |
| PR-003-A03 | PR-003 | PLAN-0.10 / same | Pending | me@jeickmeier.com | Private `finstack-ai-ci-smoke` on Linux/macOS/Windows via hosted CI; local Darwin smoke recorded as PR-003-E-release-smoke-5ac88e7bdf8a | — | — | — |
| PR-003-A04 | PR-003 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-003-E-generated-docs-ba54420a0521 | — | me@jeickmeier.com | 2026-08-08 |
| PR-003-A05 | PR-003 | PLAN-0.10 / same | Passed | me@jeickmeier.com | PR-003-E-supply-chain-9c119be89fd4; PR-003-E-secret-scan-b09efb8d60cb; PR-003-E-security-review-fc22979d5bcb | — | me@jeickmeier.com | 2026-08-08 |

PR-001 acceptance is closed against `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862`. Artifacts live under [`artifacts/pr-001/`](artifacts/pr-001/).

PR-002 acceptance is closed against `ee9754fe2d0f015181dcefa97e715392aadd28ed`. Artifacts live under [`artifacts/pr-002/`](artifacts/pr-002/). A02–A04 are `Not applicable` because compile-fixture proofs were intentionally deferred to PR-014–PR-018; package-edge / wasm-host checks remain in scope for PR-002. Hosted CI wiring is PR-003.

PR-003 acceptance is partially closed on branch `pr-003-ci-release-matrix` (A02/A04/A05 Passed with local/review evidence). A01 and cross-platform A03 remain `Pending` until hosted CI evidence exists; blocked by PR-003-B-no-remote-ede93913b2ea. Artifacts live under [`artifacts/pr-003/`](artifacts/pr-003/).

## Evidence records

| Evidence | Produced date | Scope | Type | Command, job, or review | Environment / target | Commit | Result | Artifact, log, or digest | Produced by | Verified by | Verified date | Supersedes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PR-001-E-dep-direction-1fe94f769044 | 2026-08-08 | PR-001 | Local command | `cargo metadata --format-version 1` + workspace edge assertion | Darwin arm64; rustc/cargo 1.97.1 (`environment.txt`) | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/`](artifacts/pr-001/) `dependency-direction.txt` sha256 `fe777c14e61c21f41535e465fa7eec585ec1b35facc4b63fe566e54ca607e026`; `cargo-metadata.json` sha256 `c64923139ab02812544da4e1bdf113bf68e5c647ab008da2a254fe505f1f682f` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-cargo-check-3e6a85e8d27f | 2026-08-08 | PR-001 | Local command | `cargo check --workspace --all-targets`; `cargo check -p finstack-ai-kernel` | Darwin arm64; rustc/cargo 1.97.1 | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/cargo-check.txt`](artifacts/pr-001/cargo-check.txt) sha256 `0d2dcea452af6e8fcab54f40cd8c92cdefff15c8b334ec2910e6412fa6211c26` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-kernel-deps-1d8c0f41d158 | 2026-08-08 | PR-001 | Local command | Kernel dependency inventory from `cargo metadata` (forbidden set empty) | Darwin arm64; rustc/cargo 1.97.1 | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/kernel-deps.txt`](artifacts/pr-001/kernel-deps.txt) sha256 `7dd89503a66614cafdd08c63050b00b28639e6b86f806c2d9471e77c533bc8ca` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-mise-doctor-cae2f2eb7918 | 2026-08-08 | PR-001 | Local command | `mise run install`; `mise run doctor` | Darwin arm64; mise tools rust 1.97.1, python 3.14.6, uv 0.10.11 | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/mise-install.txt`](artifacts/pr-001/mise-install.txt) sha256 `3d93bbf76a6716cd61c97a3f711a0a9bfcf1b2208ce0b0234eca8fc416ab4502`; [`mise-doctor.txt`](artifacts/pr-001/mise-doctor.txt) sha256 `b97d92e7d7636fd9e6a9ced0e9496c78cad2ab53fbaec1914a3fdaa84866fcae` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-ownership-review-b14626f70259 | 2026-08-08 | PR-001 | Manual review | Review `GOVERNANCE.md` + `CONTRIBUTING.md` ownership/DCO | Files at reviewed commit | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/ownership-security-review.txt`](artifacts/pr-001/ownership-security-review.txt) sha256 `6b41e0e37adb1d09c4a5c51613da432de89edcdcb120cd05980bbf013be0182e` (A05) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-001-E-security-md-f70391db8ac2 | 2026-08-08 | PR-001 | Manual review | Review `SECURITY.md` private path, supported-version placeholder, response owner vs Threat Model §14 | Files at reviewed commit | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` | Pass | [`artifacts/pr-001/ownership-security-review.txt`](artifacts/pr-001/ownership-security-review.txt) sha256 `6b41e0e37adb1d09c4a5c51613da432de89edcdcb120cd05980bbf013be0182e` (A06) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-002-E-architecture-6ed3268e6ff5 | 2026-08-08 | PR-002 | Local command | `mise run architecture` (`/usr/bin/time -p`; real 0.32s) | Darwin arm64; rustc/cargo 1.97.1; python 3.14.6; uv 0.10.11 (`environment.txt`) | `ee9754fe2d0f015181dcefa97e715392aadd28ed` | Pass | [`artifacts/pr-002/`](artifacts/pr-002/) `architecture.txt` sha256 `d3df9d09b4218e46945ad5d776347b67a3972d36c72aa6721c87dcbfb8ca4689`; `architecture.time.txt` sha256 `6fd7f72f2ed0049e6386dc3f2ed5d96f083eafd7e4f0d48e27a2cb36ae8f1f4f` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-002-E-unit-tests-fc419e9d2920 | 2026-08-08 | PR-002 | Local command | `uv run --no-project python -m unittest discover -s tools/architecture/tests -v` (21 tests; forbidden-kernel cases) | Darwin arm64; python 3.14.6 | `ee9754fe2d0f015181dcefa97e715392aadd28ed` | Pass | [`artifacts/pr-002/unit-tests.txt`](artifacts/pr-002/unit-tests.txt) sha256 `38af2502594e9051258eed9711ef60f48534beed45fbd36d68057763cce65ba8` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-002-E-wasm-binding-5253da7a99f9 | 2026-08-08 | PR-002 | Local command | `cargo check -p finstack-ai-wasm --target wasm32-unknown-unknown` | Darwin arm64; rustc/cargo 1.97.1; wasm32-unknown-unknown | `ee9754fe2d0f015181dcefa97e715392aadd28ed` | Pass | [`artifacts/pr-002/wasm-binding.txt`](artifacts/pr-002/wasm-binding.txt) sha256 `23b9a21bbaf189889e08dc27ed8eba521021280c5852db22205f721db08b2c7c` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-002-E-dep-direction-321ce9b2b4b4 | 2026-08-08 | PR-002 | Local command | `cargo metadata --format-version 1 --locked` + workspace edge inventory / ARCH004 policy | Darwin arm64; rustc/cargo 1.97.1 | `ee9754fe2d0f015181dcefa97e715392aadd28ed` | Pass | [`artifacts/pr-002/dependency-direction.txt`](artifacts/pr-002/dependency-direction.txt) sha256 `507d4f5b1cf069d787898f1e039e9fa7f7b01935b424242ff690cab87b49d940`; `cargo-metadata.json` sha256 `26c2854866f59ac887883bc0e35e5c5643e1993f3c6d35230cacbacbcf3d2b58` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-002-E-kernel-deps-2745bac40199 | 2026-08-08 | PR-002 | Local command | Kernel dependency inventory from `cargo metadata` (forbidden set empty) | Darwin arm64; rustc/cargo 1.97.1 | `ee9754fe2d0f015181dcefa97e715392aadd28ed` | Pass | [`artifacts/pr-002/kernel-deps.txt`](artifacts/pr-002/kernel-deps.txt) sha256 `e9feac6445e4b25e2028f0b9243cbeb0e9c8d5a483320d2fc6df8c103db4514f` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-002-E-waiver-fa6bff499990 | 2026-08-08 | PR-002 | Local command | Waiver field unit tests (`fixtures/architecture/cases/valid-waiver.toml`) | Darwin arm64; python 3.14.6 | `ee9754fe2d0f015181dcefa97e715392aadd28ed` | Pass | [`artifacts/pr-002/unit-tests.txt`](artifacts/pr-002/unit-tests.txt) sha256 `38af2502594e9051258eed9711ef60f48534beed45fbd36d68057763cce65ba8` (waiver cases) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-002-E-review-627382618b20 | 2026-08-08 | PR-002 | Manual review | Review checklist/allowlist/exceptions linkage; approve A02–A04 N/A (compile fixtures deferred to PR-014–PR-018; edge + wasm-host checks remain) | Files at reviewed commit | `ee9754fe2d0f015181dcefa97e715392aadd28ed` | Pass | [`artifacts/pr-002/README.md`](artifacts/pr-002/README.md) sha256 `d8f4e1220ab459e90337b273b5a213300673ec0a2cd82073016aba1aee65645d`; checklist `.github/ARCHITECTURE_REVIEW_CHECKLIST.md` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-003-E-channel-ownership-3ada3f6cab05 | 2026-08-08 | PR-003 | Manual review | Review `.github/ci/README.md` + `ci.yml`/`nightly.yml` stable/MSRV/nightly/fuzz ownership and cadence | Files on branch `pr-003-ci-release-matrix` | `92217776d215c5658910b60397d0e22960466218` | Pass | [`artifacts/pr-003/README.md`](artifacts/pr-003/README.md); [`.github/ci/README.md`](../../.github/ci/README.md) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-003-E-release-smoke-86364cc1b512 | 2026-08-08 | PR-003 | Local command | `mise run release-smoke` | Darwin arm64; rustc 1.97.1 (`environment.txt`) | `92217776d215c5658910b60397d0e22960466218` | Pass | [`artifacts/pr-003/release-smoke.txt`](artifacts/pr-003/release-smoke.txt); `build-metadata.json`; `release-SHA256SUMS` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-003-E-generated-docs-ba54420a0521 | 2026-08-08 | PR-003 | Manual review | Review generated-binding/fixture verification and dirty-tree policy in `.github/ci/README.md` | Files on branch `pr-003-ci-release-matrix` | `92217776d215c5658910b60397d0e22960466218` | Pass | [`.github/ci/README.md`](../../.github/ci/README.md); [`artifacts/pr-003/README.md`](artifacts/pr-003/README.md) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-003-E-supply-chain-01e376e9f82a | 2026-08-08 | PR-003 | Local command | `mise run supply-chain` (Eng §9; TM-18) | Darwin arm64; cargo-deny 0.20.2 | `92217776d215c5658910b60397d0e22960466218` | Pass | [`artifacts/pr-003/supply-chain.txt`](artifacts/pr-003/supply-chain.txt) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-003-E-secret-scan-37002e564f23 | 2026-08-08 | PR-003 | Local command | `mise run secret-scan`; `mise run secret-scan-canary` (SEC-INV-005; TM-04) | Darwin arm64; gitleaks 8.30.1 | `92217776d215c5658910b60397d0e22960466218` | Pass | [`artifacts/pr-003/secret-scan.txt`](artifacts/pr-003/secret-scan.txt); [`secret-scan-canary.txt`](artifacts/pr-003/secret-scan-canary.txt) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-003-E-security-review-9d2dc7548b4f | 2026-08-08 | PR-003 | Manual review | Threat Model §18 build/release review for CI/release-smoke introduction | Files on branch `pr-003-ci-release-matrix` | `92217776d215c5658910b60397d0e22960466218` | Pass | [`artifacts/pr-003/security-review.txt`](artifacts/pr-003/security-review.txt) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | — |
| PR-003-E-channel-ownership-7d912e4d6ce2 | 2026-08-08 | PR-003 | Manual review | Re-review channel ownership after removing green reserved-fuzz job; fuzz remains docs-only with cadence | Files on branch `pr-003-ci-release-matrix` | `7c9d1a4b1d485f8bcd187e5a95301c6e1a840e22` | Pass | [`.github/ci/README.md`](../../.github/ci/README.md); [`.github/workflows/nightly.yml`](../../.github/workflows/nightly.yml) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | PR-003-E-channel-ownership-3ada3f6cab05 |
| PR-003-E-release-smoke-5ac88e7bdf8a | 2026-08-08 | PR-003 | Local command | `mise run release-smoke` (Cargo-derived metadata; native-tokio facade feature) | Darwin arm64; rustc 1.97.1 (`environment.txt`) | `7c9d1a4b1d485f8bcd187e5a95301c6e1a840e22` | Pass | [`artifacts/pr-003/release-smoke.txt`](artifacts/pr-003/release-smoke.txt); `build-metadata.json`; `release-SHA256SUMS` | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | PR-003-E-release-smoke-86364cc1b512 |
| PR-003-E-supply-chain-9c119be89fd4 | 2026-08-08 | PR-003 | Local command | `mise run supply-chain` with `deny.toml` `all-features = true` | Darwin arm64; cargo-deny 0.20.2 | `7c9d1a4b1d485f8bcd187e5a95301c6e1a840e22` | Pass | [`artifacts/pr-003/supply-chain.txt`](artifacts/pr-003/supply-chain.txt) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | PR-003-E-supply-chain-01e376e9f82a |
| PR-003-E-secret-scan-b09efb8d60cb | 2026-08-08 | PR-003 | Local command | `mise run secret-scan`; `mise run secret-scan-canary` including Cargo.lock/uv.lock paths | Darwin arm64; gitleaks 8.30.1 | `7c9d1a4b1d485f8bcd187e5a95301c6e1a840e22` | Pass | [`artifacts/pr-003/secret-scan.txt`](artifacts/pr-003/secret-scan.txt); [`secret-scan-canary.txt`](artifacts/pr-003/secret-scan-canary.txt) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | PR-003-E-secret-scan-37002e564f23 |
| PR-003-E-security-review-fc22979d5bcb | 2026-08-08 | PR-003 | Manual review | Threat Model §18 review updated for review-fix remediation | Files on branch `pr-003-ci-release-matrix` | `7c9d1a4b1d485f8bcd187e5a95301c6e1a840e22` | Pass | [`artifacts/pr-003/security-review.txt`](artifacts/pr-003/security-review.txt) | me@jeickmeier.com | me@jeickmeier.com | 2026-08-08 | PR-003-E-security-review-9d2dc7548b4f |

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
