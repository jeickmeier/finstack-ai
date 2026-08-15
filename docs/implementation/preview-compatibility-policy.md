# Preview compatibility policy

Adopter-facing promise for unpublished lockstep **`0.1.0`**. This is not a
1.0 SemVer guarantee and not a named G7 decision. Operational routing stays
in [`compatibility-governance.md`](compatibility-governance.md). Planning
docs under `docs/planning/` remain authoritative.

Framework `0.1.0` maps to experimental WIT package names
`finstack:ai-toolset@0.0.4` and `finstack:ai-context@0.0.4`. Crate, wheel,
and npm **version fields** are `0.1.0`. WIT **package names** stay `@0.0.4`
until PR-062 / framework `1.0.0`.

Tag `v0.1.0`, registry publish, and `G7-D-*` are separately named owner
actions. Until those exist, treat this file as the in-tree preview scope
for the unpublished candidate.

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
| WIT packages | `experimental` (`@0.0.4`) | Exact compiled worlds; `@1.0.0` waits for PR-062 |
| Plugin lockfile | `experimental` | Local lockfile-only discovery; no registry fetch |
| IndexedDB adapter | `experimental` / non-durable | Not a durable store; NFR-REL-001 is not advertised |

## Promise

Preview is supported for security fixes on the `0.1.0` line once a named
tag exists. Until then, security contact is the default-branch tip of this
unpublished candidate.

Breaking changes before `1.0.0` still require classification, changelog,
and fixtures under [`compatibility-governance.md`](compatibility-governance.md).
Candidate or experimental labels do **not** permit silent breakage.

Journal meaning breaks still need an ADR and a migration. WIT `@1.0.0` and
independent leaf versioning wait for PR-062.

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
