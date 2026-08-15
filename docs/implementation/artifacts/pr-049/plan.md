# PR-049 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch: `codex/pr-049-wit-toolset-world`
Baseline: local `main` at `4e07db21b0e84e9cdb4be14a4b7d4b073904185e`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-049. The closed PR-043–PR-048
envelope is not reused. PR-049 is the only active logical PR.

## Admission

- Dependencies PR-021, PR-023, and PR-039 are `Done`.
- Phase 7 entrance is recorded as `Passed` (3/3) in this PR:
  `PH7-E-entrance-ports-d16eb3f550bd`,
  `PH7-E-entrance-record-context-81f85ef9d2cf`, and
  `PH7-E-entrance-adr-035-3c4a20fedca4`. Do not re-record later.
- G5 is `Not ready`. That does not block Phase 7. Do not infer G5 or G6.
- ADR-035 stays Accepted / Not started / Missing until candidate
  evidence; then **Partial**, never Implemented (G6 owns closeout).
- ADR-010 / ADR-011 stay Partial. ADR-014 stays Missing unless the
  security review records that WIT worlds are not remote/process DTOs.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  copies TDD §27.2–27.4 and does not change WIT compatibility policy
  or add native dylib loading.
- Threat Model section 18 is triggered (dynamic ABI / plugin surface).
  Primary **TM-07**; also TM-06 labels and ADR-011. Complete the review
  before merge. Full deny-by-default WASI is PR-052.

## Acceptance mapping

Seven Implementation Plan bullets map 1:1 to A01–A07.

- PR-049-A01: Rust guest and host bindings compile from checked-in WIT.
  Regenerated bindgen is dirty-tree-checked (`mise run gen-wit` /
  `check-wit`).
- PR-049-A02: A reference toolset lists and executes two or more tools
  through the generated `toolset` export. In-process Rust guest impl;
  no Wasmtime instantiate.
- PR-049-A03: Catalog digest, `execution-mode`, approval policy,
  `max-result-bytes`, side-effect/retry strings, and sanitized
  `call-context` map to native `ToolSpec` / `RunCallContext`. Omitted
  or invalid security metadata fails registration; no permissive
  defaults.
- PR-049-A04: Oversized `args-json`, catalog JSON, blob reads, and
  results reject before allocation (TDD §6.5: 4 MiB string, 1 MiB
  `RawJson`, 64 KiB metadata; `max-result-bytes == 0` invalid).
- PR-049-A05: WIT package documents exact-world / experimental-0.x /
  deprecation rules (family `exact-world` in
  `schemas/schema-families.toml`).
- PR-049-A06: Only `@0.0.4` packages. A check fails if `@1.0.0` worlds
  appear. Workspace crate version stays `0.0.3`; do not cut `0.0.4`.
- PR-049-A07: World exports only `toolset` (`list-tools`, `call`). No
  nested-agent, session, run, or lineage operations. Surface inventory
  test.

## Execution envelope

Authorized 2026-08-14 by `implement the plan`.

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, tag, G5 inference,
G6 inference. Do not start PR-050+.

## Locked design

Copy TDD §27.2–27.4 verbatim. Create only:

- `plugins/finstack-ai-wit/wit/v0.0.4/finstack-ai-types/types.wit`
- `plugins/finstack-ai-wit/wit/v0.0.4/finstack-ai-host/host.wit`
- `plugins/finstack-ai-wit/wit/v0.0.4/finstack-ai-toolset/toolset.wit`

Do not create `context.wit`. No component resources. Host imports grant
only `logging.log` and `blobs.read`.

`call-context` is a sanitized projection only. Catalog digest is
computed by the host mapper; the guest-supplied digest is verified.

Crate graph: leaf `plugins/finstack-ai-wit` may depend on
`finstack-ai-runtime` with `default-features = false`. Kernel, runtime,
and SDK must not depend on it. No `wasmtime` in any `Cargo.toml`.

Bindgen is a project-owned generator
(`tools/wit_bindgen/generate.py`) that emits checked-in
`plugins/finstack-ai-wit/src/generated.rs`. Never hand-edit generated
files.

`finstack-ai-plugin-host` stays README-only.

## Explicit exclusions

No context-provider world, manifest, lifecycle, or Registrar adapters
(PR-050). No Wasmtime engine/cache/adapters (PR-051). No WASI grants,
fuel/epoch, signatures (PR-052). No guest SDK or authoring templates
(PR-053). No lockfile discovery, G6, or `0.0.4` cut (PR-054). No model
provider world. No arbitrary host filesystem. No native dylib ABI. No
seventh port. No kernel/runtime dependency on plugins. No G5/G6
decision. Do not restore `tools/architecture/`.

## Validation

- `mise run gen-wit` then `mise run check-wit`
- `cargo test -p finstack-ai-wit --offline --locked`
- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai`
  — no `wasmtime`, no `finstack-ai-wit`
- `mise run check-wasm`
- `uv run --no-project python tools/wasm_package/check.py graph`

Do not require `mise run ci` or Playwright to close the candidate.
