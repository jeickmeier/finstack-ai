# Preview compatibility policy

Adopter-facing promise for tagged lockstep **`0.1.0`** (`v0.1.0`). This is
not a 1.0 SemVer guarantee. G7 passed via
`G7-D-public-preview-f7c7e70b9e04`. Operational routing stays in
[`compatibility-governance.md`](compatibility-governance.md). Planning
docs under `docs/planning/` remain authoritative.

Framework `0.1.0` maps to experimental WIT package names
`finstack:ai-toolset@0.0.4` and `finstack:ai-context@0.0.4`. Crate, wheel,
and npm **version fields** are `0.1.0`. WIT **package names** stay `@0.0.4`
until the 1.0 freeze. PR-062 generates `@1.0.0` beside `@0.0.4`. The
adopter-facing 1.0 promise is
[`1.0-compatibility-policy.md`](1.0-compatibility-policy.md)
(approved by `COMP-1.0-D-contract-freeze-00b78667ecc4`). Workspace
version fields are now unpublished `1.0.0`. This file remains the
historical tagged `0.1.0` promise.

Tag `v0.1.0` is cut. crates.io / PyPI / npm publication remains blocked
on owner registry credentials. Until those exist, consume this tree from
the git tag, not from a registry.

## Supported families

| Family | Preview label | Notes |
| --- | --- | --- |
| Public Rust API | `candidate` | Native SDK, kernel/runtime types, and first-party leaf crates that share the lockstep version |
| Journal / snapshot | `candidate` (candidate-v1) | Durable meaning breaks need an ADR and a migration |
| Runtime events | `candidate` | Durable vs transient classification is part of the contract |
| AgentSpec / locks | `candidate` | Strict reject-unknown unless an extension-owned schema declares fields |
| Python wheel | `experimental` (alpha) | One curated wheel; `import finstack_ai` stays client-free |
| JS / WASM (`@finstack/ai`) | `experimental` (alpha) | Worker default; SharedArrayBuffer stays post-preview |
| Remote protocol | `candidate` | Frame, handshake, and remote session vocabulary |
| Process handshake | `candidate` | Family tag plus handshake only; session vocabulary is later |
| WIT packages | `experimental` (`@0.0.4`); `@1.0.0` frozen | Exact compiled worlds; both majors load through adapters |
| Plugin lockfile | `experimental` | Local lockfile-only discovery; no registry fetch |
| IndexedDB adapter | `experimental` / non-durable | Not a durable store; NFR-REL-001 is not advertised |

## Promise

Preview is supported for security fixes on the tagged `0.1.0` line
(`v0.1.0` and the default-branch tip while it carries that line).
Tagged registry support begins when crates.io / PyPI / npm publication
is completed with owner credentials.

Breaking changes before `1.0.0` still require classification, changelog,
and fixtures under [`compatibility-governance.md`](compatibility-governance.md).
Candidate or experimental labels do **not** permit silent breakage.

Journal meaning breaks still need an ADR and a migration. WIT `@1.0.0`
and the leaf-versioning *policy* are recorded by PR-062. Versions stay
lockstep until preview feedback names coupling harm.

## Deprecation process

1. Record the deprecation in the changelog `Deprecated` section.
2. Name the replacement in the same change.
3. Do not remove the deprecated row before `1.0.0` unless it was never in
   the published preview scope.
4. Classify the change and update fixtures when decoding independence
   requires it.

## How to report a defect

- Security and confidentiality defects: follow [`SECURITY.md`](../../SECURITY.md).
  Do not open a public issue for vulnerabilities that could harm users.
- Compatibility defects that are not security issues: record them on the
  in-repo [`public-preview-roadmap.md`](public-preview-roadmap.md). Mirroring
  those rows to GitHub is a separately named external action.
