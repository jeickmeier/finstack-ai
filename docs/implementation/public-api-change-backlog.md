# Public API change backlog

Operational triage of open public-surface deltas before `0.1.0`.
This register does not redefine compatibility policy. Planning docs
under `docs/planning/` remain authoritative. Published preview
compatibility scope is later Phase 8 / PR-061 / G7 work.

Seeded 2026-08-15 from
[`compatibility-governance.md`](compatibility-governance.md),
[`native-preview-binding-surface.md`](native-preview-binding-surface.md),
named gate decisions G3–G6, and the Implementation Plan PR-055–PR-061
sequence. Rows are existing surfaces and already-mapped delivery; they
are not newly invented breaks.

Disposition values: `keep` (no preview-blocking change),
`change before preview` (mapped Phase 8 work),
`defer past preview` (explicit later-phase or excluded scope).

| Surface family | Current promise | Owner | Open deltas | 0.1.0 disposition | Evidence |
| --- | --- | --- | --- | --- | --- |
| Public Rust APIs / native preview | `candidate` | Core/runtime lead (`me@jeickmeier.com`) | Additive provider/tool/observer/server leaves in PR-055–PR-058. [`native-preview-binding-surface.md`](native-preview-binding-surface.md) is stale (`0.0.2-alpha-candidate`; still excludes binding/durability work that later landed). Refresh that note in PR-061. No seventh port. | `keep` candidate; refresh the stale surface note before preview | [compatibility-governance](compatibility-governance.md); G3-D-native-preview-14a386c7db24 |
| Journal / snapshot | `candidate` (candidate-v1) | Durability/ecosystem lead (`me@jeickmeier.com`) | No open journal-meaning break. Staged unpublished `0.0.3` checkpoint is not a schema change. | `keep` | `fixtures/compatibility/journal/v1/`; PH6-E-exit-sqlite-b0641b0be338; G5-D-durable-beta-a9568bd869b5 |
| Runtime events | `candidate` | Core/runtime lead (`me@jeickmeier.com`) | Schema dir remains reserved. Durable vs transient classification already recorded. No preview-blocking event-envelope rewrite. | `keep` | [compatibility-governance](compatibility-governance.md) runtime-events row |
| AgentSpec / locks | `candidate` | Ecosystem lead (`me@jeickmeier.com`) | Strict reject-unknown stays. PR-055 does not change lock or catalog digest math. | `keep` | [compatibility-governance](compatibility-governance.md) AgentSpec row; ADR-008 / ADR-020 Verified at G5 |
| Python wheel / lazy providers | `experimental` (alpha) | Bindings lead (`me@jeickmeier.com`) | PR-055 added lazy Anthropic and Ollama/local to the one curated wheel. `linked_providers()` is `("openai-compatible", "anthropic", "ollama")`. `import finstack_ai` stays client-free. | `keep` experimental after PR-055 additive | G4-D-binding-parity-101224c5eb60; PR-055-E-candidate-1000935012bf |
| JS/WASM host adapters | `experimental` (alpha) | Bindings lead (`me@jeickmeier.com`) | No JS/WASM Anthropic adapter in PR-055. SharedArrayBuffer stays post-preview reconsideration. | `keep` host adapters; Anthropic JS `defer past preview` | G4-D-binding-parity-101224c5eb60; ADR-031 |
| WIT packages | `experimental` (`@0.0.4`) | Runtime/security owner (`me@jeickmeier.com`) | Worlds stay exact `@0.0.4`. `@1.0.0` generation remains blocked until PR-062. | `keep` experimental `@0.0.4`; `@1.0.0` `defer past preview` | G6-D-plugin-alpha-018aaea9aa00; ADR-035 |
| Plugin lockfile | `experimental` | Runtime/security owner (`me@jeickmeier.com`) | Local lockfile-only discovery. No registry fetch. | `keep` experimental | [compatibility-governance](compatibility-governance.md) plugin-lock row; G6-D-plugin-alpha-018aaea9aa00 |
| Remote protocol | `not yet` | Runtime/security owner (`me@jeickmeier.com`) | Reserved family. PR-058 owns the first transport-neutral session protocol. | `change before preview` (PR-058) | [compatibility-governance](compatibility-governance.md) remote row; ADR-014 Missing |
| Process protocol | `not yet` | Runtime/security owner (`me@jeickmeier.com`) | Reserved family. Distinct vocabulary; shared framing allowed later (ADR-021). | `change before preview` (PR-058 reserve); framing policy `defer past preview` until ADR-021 | [compatibility-governance](compatibility-governance.md) process row |
| IndexedDB adapter | `experimental` / non-durable | Bindings lead (`me@jeickmeier.com`) | Provisional label stays. Not a durable store. NFR-REL-001 is not advertised. | `keep` experimental; durability `defer past preview` | G5-D-durable-beta-a9568bd869b5 residual; PR-048-A10 |
| Golden traces / scripted inputs | `candidate` | Core/runtime lead (`me@jeickmeier.com`) | PR-055 added Anthropic and Ollama fixture families under `fixtures/compatibility/providers/v1/`. Scripted model remains the semantic reference. | `keep`; additive fixtures landed in PR-055 | [compatibility-governance](compatibility-governance.md) golden-trace row; PR-055-E-candidate-1000935012bf |
| Benchmark report metadata | `candidate` | Core/runtime lead (`me@jeickmeier.com`) | Warning-only Anthropic benches in PR-055. Named budgets are PR-063. | `keep` | [compatibility-governance](compatibility-governance.md) benchmark-report row |
| First-party batteries / observers | `not yet` (examples exist; batteries incomplete) | Ecosystem lead (`me@jeickmeier.com`) | Filesystem/shell/compaction (PR-056) and observer adapters (PR-057) are mapped Phase 8 work. | `change before preview` (PR-056, PR-057) | Implementation Plan Phase 8 PR-056–PR-057 |
| Docs / starters / security artifacts | `not yet` for preview pack | Quality/release owner (`me@jeickmeier.com`) | PR-060 owns guides, starters, SBOM, and Threat Model refresh. | `change before preview` (PR-060) | Implementation Plan Phase 8 PR-060 |
| Preview compatibility policy | `not yet` | Quality/release owner (`me@jeickmeier.com`) | This backlog is triage, not the published `0.1.0` policy. PR-061 / G7 own that publication. | `change before preview` (PR-061) | Implementation Plan Phase 8 PR-061 |

Explicitly not preview-blocking and not opened as new deltas here:
marketplace, native dylib ABI, PostgreSQL, `@1.0.0` WIT worlds,
exactly-once delivery, IndexedDB durability, SharedArrayBuffer, and
npm/pypi/crates.io publication of the staged `0.0.3` / `0.0.4`
checkpoints.
