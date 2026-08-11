# PR-026 implementation plan

Date: 2026-08-11  
Owner: `me@jeickmeier.com`  
Branch: `codex/pr-026-native-preview`  
Baseline: local `main` closeout `f38a0763420224b845d1c5da2512adbbed8bc6f8`

## Frozen scope

1. Finish the native SDK facade needed to execute a bounded model-only or
   tool-loop run through the commit-before-effect runtime.
2. Replace the Rust placeholder with offline-testable minimal, coding,
   service, and diagnostic CLI examples using canonical package names.
3. Add reproducible `0.0.1-dev` staging, Cargo publication dry runs,
   changelog generation/checking, and documentation-site scaffolding.
4. Publish repository-owned native benchmark thresholds, binary-size and
   memory reports, and architecture diagrams as generated release artifacts.
5. Declare the Phase 3 candidate binding surface and require an ADR plus the
   existing compatibility workflow for incompatible changes.
6. Validate the clean-user fixture, native examples, supported-host workflow,
   performance warnings, release staging, Phase 3 exits, and the delegated G3
   decision against immutable local integration evidence.

## Acceptance map

| Acceptance | Planned proof |
| --- | --- |
| PR-026-A01 | Isolated clean-user fixture adds `finstack-ai`, `finstack-ai-provider-openai-compatible`, and a canonical toolset package, then completes an offline loopback run |
| PR-026-A02 | Model-only and tool-loop example tests run locally and the required Linux/macOS/Windows workflow invokes the same `mise run test-pr026` task |
| PR-026-A03 | Versioned warning-only thresholds are evaluated against native throughput, memory, and binary-size artifacts |
| PR-026-A04 | `0.0.1-dev` manifest, package archives, docs, changelog, diagrams, checksums, and two-build reproducibility proof are staged without publication |
| PR-026-A05 | Phase 3 exit evidence is complete before a separate named G3 decision is recorded |

## Constraints and exclusions

The kernel remains synchronous and I/O-free; the runtime retains the six
ports and commit-before-effect ordering; SDK composition retains direct
resolved handles. Examples are offline and secret-free by default. No Python
or WASM binding implementation, durability database, plugin ABI promise,
push, hosted pull request/merge, registry publication, or other external
action is authorized. A checked-in cross-platform workflow is not itself a
claim that an unrun hosted matrix passed; any gate decision will state the
exact evidence and residual limits.
