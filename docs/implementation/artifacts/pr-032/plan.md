# PR-032 execution plan

Date: 2026-08-12
Owner: me@jeickmeier.com
Branch: `codex/pr-032-python-release-staging`
Baseline: local and remote `main` at `51c71d4abf7243ac10096308d7ac8e63bd732425`
Plan baseline: documentation pack v0.20 / PLAN-0.18

## Admission

- PR-004 records accepted ADR-017, ADR-018, and ADR-020.
- PR-027 through PR-031 are locally integrated, closed, and pushed to `main`.
- Both Phase 4 entrance criteria remain passed.
- PR-032 is the only active logical PR. PR-033 and later work is excluded.
- No Implementation Plan section 6.3 ADR trigger applies.
- Security and Threat Model section 18 is triggered because this PR changes
  release identities, signing, and provenance. A focused security review and
  hosted security evidence are required before merge.

## Acceptance mapping

| Criterion | Planned proof |
| --- | --- |
| PR-032-A01 | Run every golden scenario applicable to the shipped Python facade and compare its normalized Rust-owned records/events/results with the shared expected traces. |
| PR-032-A02 | Build the release-candidate sdist and complete wheel matrix, install in clean compiler-free environments, and run the supported Python conformance/examples. |
| PR-032-A03 | Complete runtime/stub typing, API reference, migration notes, pytest fixtures, and two clean starter projects: Rust-backed and trusted Python-callback. |
| PR-032-A04 | Add the compact shared capability catalog and deterministic activation policy, expose `Always`, `Application`, and `Model` idiomatically in Python, and prove activation appends after an unchanged stable prompt prefix. |
| PR-032-A05 | Stage the Python half of version `0.0.2` with deterministic checksums and CycloneDX SBOM references, then generate GitHub-signed build/SBOM attestations without publishing, tagging, or claiming the cross-binding checkpoint. |

## Implementation slices

1. Complete the shared SDK capability catalog, bounded rendering, deterministic
   selection policy, immutable plan rebuilds, and kernel activation source proof.
   The kernel remains the semantic owner; Python only converts configuration
   and returns Rust-owned records/events.
2. Add the Python capability surface and conformance fixtures, keeping callbacks
   coarse and trusted and preserving stub/runtime signature and hover parity.
3. Add API/migration/benchmark documentation and executable starter projects.
4. Advance lockstep workspace/Python staging metadata to `0.0.2`; extend the
   existing reproducible build with deterministic checksums and a CycloneDX
   SBOM generated from locked source metadata.
5. Add a manual/branch release-staging workflow that uses the current GitHub
   artifact-attestation contract with least privilege. Actions remain pinned by
   immutable digest. The workflow uploads staging artifacts only; it does not
   publish to PyPI or create a release/tag.
6. Run focused, package, type, docs, benchmark, supply-chain, aggregate, hosted
   wheel/CI/security/release-staging, and post-merge checks; bind immutable
   candidate, hosted, security-review, and integration evidence.

## Current documentation baseline

Context7 was queried for the current GitHub Actions artifact-attestation
contract. GitHub documents `actions/attest@v4`, `subject-path`, `sbom-path`, and
the required `contents: read`, `id-token: write`, and `attestations: write`
permissions. The implementation will pin the resolved action commit rather
than a mutable major tag.

The repository `finstack-production-release-prep` skill informed the release
hygiene, dependency/security, artifact, checksum, SBOM, reproducibility, and
final-verification slices. Its finstack-quant command names are not applicable;
this repository's checked-in `mise run` tasks remain authoritative.

## Security disposition

- Signing identity is the GitHub Actions OIDC workflow identity at an immutable
  source revision; no private signing key is stored in the repository.
- Attestation jobs receive no package-registry credential and cannot publish.
- Release inputs and third-party actions are pinned; artifacts are checksummed
  before attestation and retained as ordinary workflow artifacts.
- SBOM generation is deterministic, offline, bounded, and based on committed
  lock/source metadata. It must not embed local extraction paths or credentials.
- Model capability selection cannot grant ambient authority. Only
  application-declared, prevalidated capabilities are eligible, and existing
  tool policy/approval floors remain in force.
- No live provider credential, registry token, signing secret, browser/WASM
  support, durable restart claim, or independent review is introduced.

## Explicit exclusions and nonclaims

- No browser WASM or WIT/plugin implementation.
- No PyPI publication, Git tag, GitHub release, or hosted pull request/merge.
- No claim that the exact cross-binding `0.0.2` checkpoint exists before PR-038.
- No G4 decision or approval; PR-038 owns the complete Phase 5/G4 evidence set.
- No PR-048 durability/restart parity and no PR-063 final performance budget.
- Real-provider, prompt-cache, and restart validation required before the 0.1.0
  public preview remains later gated work unless directly exercised here.
