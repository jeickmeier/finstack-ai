# PR-050 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-050-context-manifest-lifecycle`
Intended baseline: local `main` at `972591d77abecad635276cdc6d247aba489e91b5`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-050. The closed PR-049 envelope
is not reused. PR-050 is the only active logical PR.

## Execution envelope

Authorized 2026-08-14 by `implement the plan` using the same local-only
integrated envelope as PR-049.

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, tag, G5 inference,
G6 inference. Do not start PR-051+.

## Admission (when authorized)

- Dependency PR-049 is `Done` at local `main` merge
  `5a6eeeb783be8e243cdc72c387ac28f4b3a0e2cb`.
- Phase 7 entrance is already `Passed` (3/3):
  `PH7-E-entrance-ports-d16eb3f550bd`,
  `PH7-E-entrance-record-context-81f85ef9d2cf`, and
  `PH7-E-entrance-adr-035-3c4a20fedca4`. Do not re-record.
- G5 is `Not ready`. That does not block Phase 7. Do not infer G5 or G6.
- ADR-035 stays `In progress` / `Partial`. Never mark Implemented
  (G6 owns closeout).
- ADR-010 / ADR-011 stay Partial. ADR-014 stays Missing: WIT worlds
  remain neither remote nor process DTOs.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  copies TDD §27.5 verbatim, keeps lifecycle/manifest as host types,
  does not add a middleware world, and does not add native dylib
  loading.
- Threat Model section 18 is triggered (dynamic ABI / plugin surface).
  Primary **TM-08** (manifest identity/ABI); also TM-07 (context world
  surface), TM-06 labels, TM-01 (no self-elevation to trusted
  instructions), and TM-16 bounds. Complete the review before merge.
  Digest/signature verification, lockfile discovery, compiled-cache
  keys, and Wasmtime remain PR-051–PR-054.

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-050-A01: A context component contributes bounded context through
  the normal pipeline. In-process `GuestContextProvider` +
  `WitContextAdapter` implementing `ContextProvider`; the adapter is
  selected on a resolved `AgentSpec` and `collect` runs in
  `prepare_context`. No Wasmtime instantiate.
- PR-050-A02: Manifest validation rejects duplicate identities,
  incompatible interfaces (`@1.0.0`, unknown world, kind/world
  mismatch), and undeclared capability worlds (`middleware`, `agent`,
  or any world the package does not export).
- PR-050-A03: Lifecycle timeouts and failures have stable host
  diagnostics (`plugin_initialize_failed`, `plugin_warmup_failed`,
  `plugin_lifecycle_timeout`, `plugin_lifecycle_failed`). Initialize,
  health, shutdown, and optional warmup are **host-side** hooks, not
  extra WIT exports.
- PR-050-A04: Native agent resolution treats plugin adapters like
  ordinary port implementations. `WitPluginExtension` registers
  through `Registrar::context_provider` / `Registrar::toolset` using
  `ReadyComponent` + `LifecycleBinding`. `ResolvedAgent` holds the
  same `Arc<dyn ContextProvider>` / `Arc<dyn Toolset>` shape as a
  native battery.
- PR-050-A05: WIT lifecycle and context calls cannot invoke a full
  nested agent. `context-plugin` exports only `context-provider`
  (`collect`). Host-mediated child runs remain an SDK/runtime
  concern. `item-json` that encodes interaction, deferral, session
  mutation, or agent/run/lineage operations fails closed.

## Locked design

### WIT

Copy TDD §27.5 verbatim. Create only:

- `plugins/finstack-ai-wit/wit/v0.0.4/finstack-ai-context/context.wit`

Do not add initialize, health, shutdown, warmup, attribution, or
manifest records to WIT. Those are host types. No component resources.
No middleware world. Host imports stay `logging.log` and `blobs.read`.

`context-plugin` exports only `context-provider`. `collect` returns
one `list<context-item>` or `plugin-error`. Coarse completion: no
progress stream and no plugin-private suspension protocol.

### Host types (not WIT)

`PluginManifest` fields, fail-closed:

| Field | Rule |
| --- | --- |
| `identity` | Namespaced `ComponentId` (`finstack.plugin.*`). Empty/invalid fails. |
| `version` | Semver string. Must be `0.0.4` for this experimental cut. `@1.0.0` / `1.0.0` fails. |
| `worlds` | Non-empty subset of `{toolset-plugin, context-plugin}`. Unknown or undeclared world fails. |
| `permissions` | Subset of `{logging, blobs}`. `http`, `filesystem`, `network`, `secrets`, `clock`, `random` fail as undeclared until PR-052. |
| `configuration_schema` | JSON object bytes, 1 MiB `RawJson` ceiling, reject before allocation. Empty object allowed. |
| `digest` | Non-empty hex. Host computes the in-process digest over canonical `{identity, version, worlds}` JSON and verifies the guest-supplied value. Tamper/mismatch fails (TM-08 slice). Wasm component bytes and signatures stay PR-052. |
| `signature` | Optional `{algorithm, key_id, signature}`. Omitted means unsigned (allowed). Present but malformed/empty fields fail. Verification against trust roots is PR-052. |
| `resource_limits` | Optional host-side ceilings (`max_output_bytes`, `call_timeout_ms`). `0` is invalid. Fuel/memory/WASI preopens stay PR-052. |

Duplicate `identity` across two manifests, or against an already
registered `ComponentId`, is a registration error.

### Fail-closed context maps

WIT `context-budget` → native `ContextBudget`:

- `max-items` / `max-tokens` / `max-bytes` take the **minimum** of
  provider, run, and plugin ceilings.
- `max-items == 0` or any field exceeding `SEMANTIC_ARRAY_MAX_ITEMS`
  / payload ceilings fails.
- Overflow policy is host-owned (absent from WIT). Plugins use
  `ContextOverflowPolicy::Reject`.

WIT `context-query` → native `ContextRequest` + sanitized
`call-context`:

- Reuse `sanitize_call_context`. No credentials, claims, cancellation,
  or attempt.
- `request-json` is 1 MiB `RawJson`. It must decode to the committed
  `ContextRequest` fields the host already owns; the guest cannot
  replace session/lane/run identity.

WIT `context-item.item-json` → native `ContextItem` (attribution lives
here, not in extra WIT fields):

Required JSON object keys, unknown keys rejected:

```json
{
  "kind": "quoted_source",
  "content": [{"type": "text", "text": "..."}],
  "provenance": {
    "source_id": "finstack.plugin.reference.context",
    "source_ref": null,
    "external": true
  },
  "authority": "untrusted",
  "priority": 0,
  "sensitivity": "internal",
  "protected": false
}
```

Locked maps:

- `kind` → `ContextItemKind` (`instruction` | `quoted_source` |
  `reference` | `metadata` | `hidden_application_context` |
  `derived_summary`). Empty/unknown fails.
- `authority` → `ContextAuthority` (`untrusted` |
  `trusted_application`). `trusted_application` is accepted only when
  `ContextProviderDescriptor.trusted_application_instructions` is
  true. Guests cannot self-elevate.
- `derived_summary` must stay `untrusted`.
- `estimated-tokens` and `bytes` on the WIT record must match the
  native item after `ContextItem::try_new`. Mismatch fails.
- `blobs` are authorized through the existing host blob ceiling.
- Keys `interaction`, `deferral`, `agent`, `session`, `run`,
  `lineage`, `suspend`, or `approval` in `item-json` fail as a private
  suspension / nested-agent protocol.

### Lifecycle (host-side)

```text
load → validate manifest → initialize → optional warmup
     → catalog/collect (ordinary port)
     → health
     → shutdown (at most once)
```

Map onto existing SDK types. Do not invent a seventh port.

- `initialize`: once, before the first `collect` / `list-tools`.
  Receives `ComponentConstructionContext` only. No fabricated run
  identity or principal (TDD §31.3).
- `warmup`: optional. If the guest implements it, run once at
  construction; if not, skip. Same construction context as initialize.
- `health` / `shutdown`: implement `ComponentLifecycle` and attach
  via `ReadyComponent::with_lifecycle(LifecycleBinding::resolved_agent(...))`.
- Deadline: honor `AgentConstructionContext.deadline` /
  `cancellation`. On fire, return `plugin_lifecycle_timeout`.
- Shutdown is idempotent. Host-owned shutdown stays available but the
  reference uses resolved-agent ownership.

### Adapters and crate graph

Keep work in the existing leaf `plugins/finstack-ai-wit`.

- `GuestContextProvider` generated from `context-provider`.
- `ReferenceContextProvider` in-process guest (`collect` returns two
  bounded attributed items; ids under `finstack.plugin.reference.context`).
- `WitContextAdapter` implements `ContextProvider`.
- `WitToolsetAdapter` implements `Toolset` around the existing
  `ReferenceToolset` / `GuestToolset` (PR-049 left mapping unregistered).
- `WitPluginExtension` implements `Extension` and registers the
  adapter(s) named by the manifest.

Crate graph:

- `finstack-ai-wit` may depend on `finstack-ai` with
  `default-features = false` so `Registrar` / `Extension` /
  `LifecycleBinding` are in scope. Dev-dependency `native-tokio` is
  allowed for the resolve/run proof.
- Kernel, runtime, and SDK must **not** depend on `finstack-ai-wit`.
- No `wasmtime` in any `Cargo.toml`.
- `finstack-ai-plugin-host` stays README-only.
- Workspace crate version stays `0.0.3`. Do not cut `0.0.4`.

### Bindgen

Extend `tools/wit_bindgen/generate.py`:

- Add `finstack:ai-context@0.0.4` to `ALLOWED_PACKAGES`.
- Delete the `CONTEXT_WIT.exists()` hard fail.
- Load `finstack-ai-context/context.wit` after host, before or after
  toolset (order does not change types).
- Per-world forbidden tokens: `toolset-plugin` still forbids
  `context-provider` / `agent` / `session` / `run` / `lineage` /
  `nested`. `context-plugin` forbids `toolset` / `agent` / `session` /
  `run` / `lineage` / `nested`. Remove the global ban on the token
  `context-provider`.
- Emit `CONTEXT_WORLD_EXPORTS` / `CONTEXT_FUNCS` /
  `AI_CONTEXT_PACKAGE`. Dedupe `HOST_IMPORTS`.
- Never hand-edit `generated.rs`.

### Compatibility fixtures

Keep family `wit` / profile `exact-world` in
`schemas/schema-families.toml` (`active`). Add under
`fixtures/compatibility/wit/v0.0.4/`:

- `packages/valid--published-packages.json` includes
  `finstack:ai-context@0.0.4`
- `world/valid--context-exports.json` (`context-provider`, `collect`)
- `world/invalid--undeclared-middleware-world.wit`
- `manifest/valid--context-reference.json`
- `manifest/invalid--duplicate-identity.json`
- `manifest/invalid--incompatible-interface.json`
- `item-json/invalid--trusted-self-elevation.json`
- `item-json/invalid--private-suspension.json`

Do not add WIT fixtures to the public-rust-api corpus (stays 131).

## File map

Create:

- `plugins/finstack-ai-wit/wit/v0.0.4/finstack-ai-context/context.wit`
- `plugins/finstack-ai-wit/src/manifest.rs`
- `plugins/finstack-ai-wit/src/lifecycle.rs`
- `plugins/finstack-ai-wit/src/context_mapping.rs`
- `plugins/finstack-ai-wit/src/adapters.rs`
- fixtures listed above
- `docs/implementation/artifacts/pr-050/README.md` (at admit)

Modify:

- `tools/wit_bindgen/generate.py`
- `plugins/finstack-ai-wit/src/generated.rs` (via `mise run gen-wit`)
- `plugins/finstack-ai-wit/src/inventory.rs`
- `plugins/finstack-ai-wit/src/lib.rs`
- `plugins/finstack-ai-wit/src/reference.rs` (add context guest)
- `plugins/finstack-ai-wit/Cargo.toml` (SDK dep, `default-features = false`)
- `plugins/finstack-ai-wit/COMPATIBILITY.md`
- `plugins/finstack-ai-wit/README.md`
- `plugins/README.md`
- `plugins/finstack-ai-plugin-host/README.md` (pointer only)
- `fixtures/compatibility/wit/README.md`
- `docs/implementation/*` registers (only after admit / evidence)

Do not modify `docs/planning/`. Do not restore `tools/architecture/`.

## Tasks (when admitted)

Task IDs are minted at admit, not now.

1. Tracking: confirm PR-049 Done, record exclusions, open
   `codex/pr-050-context-manifest-lifecycle` from the named baseline.
   Do not re-record Phase 7 entrance.
2. WIT: check in TDD §27.5 `context.wit`; extend bindgen; regenerate
   `generated.rs`; update inventory for four packages and both worlds.
3. Manifest: `PluginManifest` parse/validate + digest verify +
   duplicate / incompatible / undeclared-world fixtures (A02).
4. Context mapping: budget/query/item-json/attribution fail-closed
   maps and payload ceilings (A01 mapping slice, A05 item-json).
5. Lifecycle: host initialize/health/shutdown/optional warmup +
   timeout diagnostics (A03).
6. Adapters: `WitContextAdapter`, `WitToolsetAdapter`,
   `WitPluginExtension` → `Registrar` (A04).
7. Reference + pipeline: two-item reference context guest; resolve
   `AgentSpec` and collect through `prepare_context` (A01, A05).
8. Evidence: focused validation, TM-08/TM-07 review, candidate
   evidence. Stop before G6.

## Explicit exclusions

No Wasmtime engine/cache/adapters (PR-051). No WASI grants, fuel/epoch,
signatures, or trust-root verification (PR-052). No guest SDK or
authoring templates (PR-053). No lockfile discovery, G6, or `0.0.4`
cut (PR-054). No middleware world. No model provider world. No
arbitrary host filesystem. No native dylib ABI. No seventh port. No
kernel/runtime/SDK dependency on `finstack-ai-wit`. No G5/G6 decision.
Do not restore `tools/architecture/`.

## Validation

- `mise run gen-wit` then `mise run check-wit`
- `cargo test -p finstack-ai-wit --offline --locked`
- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai`
  — no `wasmtime`, no `finstack-ai-wit`
- `cargo tree -p finstack-ai-wit` — no `wasmtime`
- `mise run check-wasm`
- `uv run --no-project python tools/wasm_package/check.py graph`

Do not require `mise run ci` or Playwright to close the candidate.
Isolated `cargo clippy -p finstack-ai-wit` can hit the pre-existing
runtime `EventBatch::new` dead_code warning when `native-tokio` is
off; workspace clippy remains the gate.

## Closeout

PR-050 is `Done` at local `main` merge `5c987e379d0cd5a8e63f734eda26b69b07c4b337`.
The envelope is closed and is not reused. Phase 7 remains `In progress`.

## Suggested authorization sentence

When ready to admit and implement, authorize an envelope explicitly.
Example:

```
Run PR-050; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names the same local-only
integrated envelope used for PR-049. Do not infer authorization from
this planning file.
