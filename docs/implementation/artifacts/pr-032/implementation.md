# PR-032 implementation review

Date: 2026-08-12
Branch: `codex/pr-032-python-release-staging`

## Delivered boundary

- The shared bundle resolver now supports `Model` activation by rebuilding a
  complete immutable resolved plan. `Always`, `Application`, and `Model`
  membership are represented in the exact lock and committed through the
  existing kernel `CapabilitiesActivated` transition with the matching source.
- The native `Agent` retains a pre-resolved variant for each model-selectable
  capability. A bounded 8 KiB compact catalog and deterministic ASCII token
  overlap policy select at most one variant per run; ties resolve by capability
  ID. Ordinary runs retain direct component handles and perform no registry
  lookup.
- The Python facade exposes declarative `Capability`, application activation,
  the compact model catalog, committed active-capability snapshots, and stable
  Rust record-kind traces. Runtime exports and the shipped stub remain aligned.
- Python conformance covers all three activation sources, unchanged stable
  prompt prefixes, inactive and selected model variants, shared structured
  output/tool-batch golden traces, and fail-closed invalid activation.
- The package includes API, migration, and benchmark notes plus two executable
  clean starter projects: a curated Rust-backed provider and an offline trusted
  Python callback.
- Workspace and Python package metadata advance in lockstep to `0.0.2`. The
  excluded fuzz workspace dependency and lock are aligned as part of the same
  metadata boundary.
- Release staging builds the sdist and per-interpreter wheel twice, requires
  byte-identical pairs, writes checksums, emits a deterministic CycloneDX 1.6
  SBOM from committed lock metadata, and binds all subjects to the source
  revision in a staged-only manifest.
- A least-privilege GitHub workflow stages the candidate, creates GitHub OIDC
  build and SBOM attestations with an immutable `actions/attest` revision, and
  uploads an ordinary retained artifact. It has no registry credential or
  publication step.

## Invariants retained

- The synchronous kernel remains deterministic and I/O-free; it owns sorted
  activation records, source classification, replay, and state transitions.
- Capability selection grants no authority. Only application-declared and
  fully resolved definitions are eligible, while existing tool policy,
  approval, registration, and component-version rules remain authoritative.
- Base instructions and `Always`/`Application` contributions are resolved
  before a selected `Model` contribution, preserving an unchanged prompt
  prefix across inactive and activated paths.
- Python remains a typed facade over the Rust semantic engine. Trusted callback
  boundaries, six ports, seven middleware stages, commit-before-effect
  ordering, and direct-handle execution are unchanged.
- Build metadata, checksums, SBOM content, and staged manifest contain no
  credentials or local extraction paths. Hosted signing uses workflow identity
  rather than a repository private key.

## Explicit limits and nonclaims

- The alpha Python `Capability` constructor contributes instructions only.
  Native bundle definitions may also contribute already registered toolsets,
  context providers, and middleware.
- Model selection is a small deterministic token-overlap activation heuristic,
  not free-form model-authored component loading or an authorization decision.
- The staged wheel is per-interpreter and platform-specific; hosted PR-032
  evidence must still prove the complete approved matrix and compiler-free
  installation.
- No PyPI publication, Git tag, GitHub release, hosted pull request/merge,
  independent review, browser/WASM binding, WIT/plugin host, or durable restart
  claim is introduced.
- `0.0.2-alpha-candidate` is not the exact cross-binding checkpoint. PR-038 and
  a separate passing G4 decision remain required before that checkpoint can be
  claimed.
