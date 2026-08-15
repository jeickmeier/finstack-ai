# PR-051 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-051-wasmtime-host-cache`
Intended baseline: local `main` at `8539c6bafcb2c99f09ace07d2a18d98ba0390f1d`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-051. The closed PR-050 envelope
is not reused. PR-051 is the only active logical PR.

## Execution envelope

Authorized 2026-08-14 by `implement the plan` using the same local-only
integrated envelope as PR-049/PR-050. Suggested text that was accepted:

```
Run PR-051; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names the same local-only
integrated envelope used for PR-049/PR-050. Do not infer authorization
from this planning file, from `continue`, or from Phase 7 being in
progress.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, tag, G5 inference,
G6 inference. Do not start PR-052+.

## Admission (when authorized)

- Dependency PR-049 is `Done` at local `main` merge
  `5a6eeeb783be8e243cdc72c387ac28f4b3a0e2cb`.
- Dependency PR-050 is `Done` at local `main` merge
  `5c987e379d0cd5a8e63f734eda26b69b07c4b337`. Closeout commit
  `8539c6bafcb2c99f09ace07d2a18d98ba0390f1d` is the intended baseline.
- Phase 7 entrance is already `Passed` (3/3):
  `PH7-E-entrance-ports-d16eb3f550bd`,
  `PH7-E-entrance-record-context-81f85ef9d2cf`, and
  `PH7-E-entrance-adr-035-3c4a20fedca4`. Do not re-record.
- G5 is `Not ready`. That does not block Phase 7. Do not infer G5 or G6.
- ADR-035 stays `In progress` / `Partial`. Never mark Implemented
  (G6 owns closeout).
- ADR-010 / ADR-011 stay Partial. This PR is the first Wasmtime leaf
  for ADR-010; it does not finish G6. ADR-014 stays Missing: WIT
  worlds remain neither remote nor process DTOs.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  keeps Wasmtime in the `plugins/` leaf, does not add a native dylib
  loader, does not change WIT compatibility policy, and does not add
  a seventh port.
- Threat Model section 18 is triggered (dynamic code-loading /
  plugin host). Primary **TM-07** (trap containment, no ambient WASI
  in this PR) and **TM-08** (compiled-cache key includes
  digest/engine/target/ABI). Also TM-06: label the Wasmtime path as
  isolated (T3) and keep in-process `finstack-ai-wit` adapters labeled
  as host-authority. Complete the review before merge.
  WASI grants, fuel/memory/table quotas, signatures/trust roots, and
  lockfile discovery remain PR-052–PR-054.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-051-A01: Minimal and standard native bundles remain Wasmtime-free
  unless plugin support is enabled. Default `finstack-ai` (with and
  without `native-tokio`), `finstack-ai-kernel`, `finstack-ai-runtime`,
  `finstack-ai-wit`, and `finstack-ai-native-examples` trees contain
  no `wasmtime`. Only `finstack-ai-plugin-host` may depend on it.
  Do not add a default SDK feature that pulls the host.
- PR-051-A02: Compiled component cache invalidates on digest, engine,
  target, or ABI change. Cache key is host-owned and must include
  component-bytes SHA-256, `wasmtime` crate/version plus a config
  fingerprint, host target triple, and ABI identity
  (`world` + `finstack:ai-*@0.0.4` package set). A hit deserializes
  the precompiled artifact; a miss compiles and stores. No lockfile
  scan and no wasmtime implicit global cache.
- PR-051-A03: A plugin trap is contained and converted to a stable
  plugin error (`plugin_trap`). The host process continues. Guest
  `unreachable` / Wasmtime trap must not panic the host or poison
  unrelated stores.
- PR-051-A04: Concurrent plugin calls respect configured
  instance/store policies. Default `Exclusive`: each call gets its
  own `Store` + `Instance` from the cached `Component`. Optional
  `Serialized`: one store/instance, mutex, no concurrent guest entry.
  `max_concurrent_instances` overflow fails closed
  (`plugin_instance_limit`). Worlds do not declare safe reuse; do
  not pool live instances.

Principal changes that are not extra acceptance IDs, but are required
to prove the four bullets:

- Leaf crate with direct Wasmtime Component Model integration.
- Engine configuration, async guest calls, and cancellation.
- `WasmToolsetAdapter` / `WasmContextAdapter` implementing the same
  native port traits as PR-050's in-process adapters (Architecture
  §15.2).
- Lazy startup: no process-global engine; `PluginHost::try_new` is
  the opt-in.

## Locked design

### Crate and graph

Create a real crate at the existing placeholder
`plugins/finstack-ai-plugin-host/` (today: README + empty `src/`).
Add it to the root workspace `members` list. Workspace version stays
`0.0.3`. `publish = false`. Native-only; do not add this crate to
`mise run check-wasm`.

```text
finstack-ai-plugin-host
  -> wasmtime (Component Model)
  -> finstack-ai-wit          # manifest, maps, lifecycle, host ceilings
  -> finstack-ai              # default-features = false (Registrar/Extension)
  -> finstack-ai-runtime      # default-features = false (ports)
  -> finstack-ai-kernel
```

Forbidden edges:

- kernel / runtime / SDK / protocol / bindings / rust-minimal →
  `wasmtime` or `finstack-ai-plugin-host`
- `finstack-ai-wit` → `wasmtime` or `finstack-ai-plugin-host`
- `finstack-ai-plugin-host` → `wasmtime-wasi` / WASI preview linker
- any `libloading` / native dylib loader

Do **not** add `plugin-host` to `finstack-ai` default features. Opt-in
is depending on the leaf crate and constructing `PluginHost`. That is
the "explicit plugin-host feature/bundle selection." A non-default
SDK convenience feature is unnecessary and would enlarge the public
feature matrix; skip it.

Pin `wasmtime` to the newest crates.io release whose MSRV is ≤ 1.97.1
(current newest at plan time: `47.0.3`; re-check at implement). Use
`default-features = false` and enable only what instantiate/async
needs, typically `runtime`, `component-model`, `async`, and
`cranelift`. Do not enable wasmtime's implicit filesystem `cache`
feature.

### Engine, lazy startup, async, cancellation

```text
PluginHost::try_new(config)
  -> Engine (once per host)
  -> ComponentCache
load(bytes, manifest)
  -> validate manifest (reuse PR-050)
  -> cache lookup / compile
  -> retain Component (not a live Instance)
first collect / list-tools / call
  -> instantiate per InstancePolicy
  -> async typed call
  -> drop Exclusive store after the call
```

- No `lazy_static` / `OnceLock` engine at crate load.
- `Config::async_support(true)`.
- `Config::epoch_interruption(true)` is allowed as the **cancellation
  channel** so a cancelled call can leave guest code. Fuel amounts,
  memory/table/instance quotas, and WASI deny-by-default policy stay
  PR-052. Do not treat epoch-on as "resource limits are enforced."
- Honor `CancellationSignal` and construction/run deadlines. Fired
  deadline or cancel maps to `plugin_lifecycle_timeout` (already
  stable). Do not invent a public SDK constructor.

### Host imports

Link only `finstack:ai-host/logging@0.0.4` and
`finstack:ai-host/blobs@0.0.4`. Reuse wit ceilings
(`reject_before_allocation`, `CeilingBlobStore` policy). Host import
state must be `Send + Sync` (PR-050 `RecordingLogger` is `RefCell`
and is not the concurrent host).

A component that requires `wasi:*`, `wasi:cli`, sockets, filesystem,
or any import other than the two host interfaces fails instantiate
with `plugin_instantiate_failed`. That is fail-closed, not a WASI
grant. PR-052 owns explicit capability linking.

### Cache (TDD §27.7 / TM-08 slice)

Host-owned directory or in-memory map. Key material, canonicalized
then SHA-256:

| Field | Source |
| --- | --- |
| `digest` | SHA-256 of the component bytes |
| `engine` | `wasmtime` crate version + config fingerprint (`async`, epoch-interrupt, compiler) |
| `target` | host `os` + `arch` (and cranelift ISA if exposed) |
| `abi` | world name + `finstack:ai-types@0.0.4` + toolset or context package |

Store `Engine::precompile_component` bytes. Load with
`Component::deserialize` (or the 47.x equivalent) only when the key
matches. Tests must show a miss after independently changing each of
the four fields.

Do not read a plugin lockfile. Do not search a registry. Do not
verify publisher signatures (PR-052). Manifest `digest` remains the
PR-050 identity/version/worlds digest; component-bytes digest is the
cache identity.

### Instance / store policy

```rust
pub enum InstancePolicy {
    Exclusive,
    Serialized,
}
```

TDD §27.7: pool live instances only if the world declares safe reuse.
`toolset-plugin` and `context-plugin` do not. Default `Exclusive`.
`Serialized` is the configured single-instance policy for A04, not a
reuse pool.

`max_concurrent_instances == 0` is invalid (same spirit as PR-050
resource-limit `0`).

### Adapters

Architecture §15.2 names `WasmToolsetAdapter` / `WasmContextAdapter`.
Put them in the plugin-host crate. They implement
`finstack_ai_runtime::{Toolset, ContextProvider}` and return
`PortFuture`. Reuse `finstack_ai_wit` manifest/world checks,
`sanitize_call_context`, `register_catalog` / `map_tool_spec`,
`map_query` / `map_context_item`, and `PluginLifecycle`.

Do **not** make PR-050 `GuestToolset` / `GuestContextProvider` async.
Those traits stay in-process and synchronous. Wasmtime calls are
async at the port boundary.

`WasmPluginExtension` registers through `Registrar::toolset` /
`Registrar::context_provider` with `ReadyComponent` +
`LifecycleBinding::resolved_agent`, same as `WitPluginExtension`.
Resolution must see ordinary `Arc<dyn Toolset>` /
`Arc<dyn ContextProvider>`.

Native `ContentBlock` serde still uses `kind` not `type`. Guests
still cannot self-elevate. `trusted_application_instructions: false`
for fixtures.

### Trap and stable codes

New host-side codes (not WIT exports):

| Code | When |
| --- | --- |
| `plugin_trap` | Wasmtime trap / `unreachable` / guest panic |
| `plugin_instantiate_failed` | link/instantiate/unresolved import |
| `plugin_compile_failed` | compile or deserialize mismatch |
| `plugin_instance_limit` | concurrent instance ceiling |

Reuse `plugin_lifecycle_timeout`, `plugin_payload_too_large`,
`plugin_registration_invalid`, `plugin_manifest_digest_mismatch`.
Wrap into `ToolError` / `ContextError` / `LifecycleError::failed`
the same way PR-050 wraps lifecycle codes (stable code in the
message; do not add public SDK constructors that change the
public-rust-api corpus).

### Test guests (not the PR-053 SDK)

`finstack-ai-wit` `generated.rs` is an in-process trait mapping, not
Canonical ABI. A real component requires official guest bindgen plus
Wasmtime host bindgen (or equivalent typed funcs) against the
checked-in WIT.

Lock:

- Test-only guests under
  `plugins/finstack-ai-plugin-host/fixtures/guests/`.
  Not workspace members. `publish = false`. Not a template. Not a
  guest SDK. Examples must not depend on them.
- Host uses `wasmtime::component::bindgen!` (or typed funcs) on
  `plugins/finstack-ai-wit/wit/v0.0.4/**`. Never hand-edit
  `finstack-ai-wit/src/generated.rs`.
- Guests may use the official `wit-bindgen` crate as a fixture
  dependency. Pin a version whose component encoding Wasmtime 47
  accepts.
- Compile `wasm32-unknown-unknown` `cdylib` (target already in mise)
  and encode with the `wit-component` crate. Prefer generating bytes
  in the test harness or a `mise run gen-plugin-wasm` /
  `check-plugin-wasm` pair. Checked-in `.wasm` is allowed only with
  that dirty-tree check.
- Three guests are enough:
  1. `echo-toolset` — `list-tools` + `call` echo/add, same spirit as
     `ReferenceToolset`.
  2. `reference-context` — two bounded attributed items, same spirit
     as `ReferenceContextProvider`.
  3. `trap-toolset` — `call` executes `unreachable`.
- If official guest bindgen injects `wasi:*` imports, do not "fix"
  that by linking WASI. Change the fixture toolchain until the
  component imports only `finstack:ai-host`. A WASI-importing
  fixture is a useful **negative** instantiate test, not the happy
  path.

Do not add WIT fixtures to the public-rust-api corpus (stays 131).
Do not cut `0.0.4`. Do not publish crates.

### Labels (TM-06)

Docs on `PluginHost` and adapters must say this path is isolated
Wasmtime (T3). `finstack-ai-wit` in-process adapters continue to say
they inherit host authority. Do not call in-process guests "sandboxed."

## File map

Create:

- `plugins/finstack-ai-plugin-host/Cargo.toml`
- `plugins/finstack-ai-plugin-host/src/lib.rs`
- `plugins/finstack-ai-plugin-host/src/error.rs`
- `plugins/finstack-ai-plugin-host/src/host.rs` — `PluginHost`, config, lazy engine
- `plugins/finstack-ai-plugin-host/src/cache.rs`
- `plugins/finstack-ai-plugin-host/src/instantiate.rs` — linker, trap map, instance policy
- `plugins/finstack-ai-plugin-host/src/adapters.rs` — Wasm toolset/context + extension
- `plugins/finstack-ai-plugin-host/fixtures/guests/{echo-toolset,reference-context,trap-toolset}/`
- `docs/implementation/artifacts/pr-051/README.md` (this folder; already started)

Modify:

- `Cargo.toml` workspace `members` + `[workspace.dependencies]` for
  `finstack-ai-plugin-host` / `wasmtime` as needed
- `Cargo.lock`
- `plugins/README.md`
- `plugins/finstack-ai-plugin-host/README.md`
- `plugins/finstack-ai-wit/README.md` / `COMPATIBILITY.md` (pointer:
  in-process vs Wasmtime leaf)
- `docs/implementation/*` registers only after admit / evidence
- `mise.toml` only if a `gen-plugin-wasm` / `check-plugin-wasm` task
  is required to keep generated component bytes honest

Do not modify `docs/planning/`. Do not restore `tools/architecture/`.
Do not add `wasmtime` to kernel forbidden-fixture cases beyond what
already exists. Do not edit `examples/rust-minimal`.

## Tasks (when admitted)

Task IDs are minted at admit, not now.

1. Tracking: confirm PR-049/PR-050 Done, record exclusions, open
   `codex/pr-051-wasmtime-host-cache` from the named baseline.
   Do not re-record Phase 7 entrance.
2. Crate: workspace member, Wasmtime pin, README, graph tests that
   default bundles stay Wasmtime-free (A01).
3. Cache: compile/deserialize + four invalidation cases (A02).
4. Instantiate: host imports only, unresolved-WASI fail-closed,
   trap → `plugin_trap` without host abort (A03).
5. Adapters: Wasm toolset/context ports, extension registration,
   Exclusive/Serialized + concurrent ceiling (A04).
6. Fixtures: three test guests; echo/collect through
   resolve + port calls; trap guest; concurrent Exclusive calls.
7. Evidence: focused validation, TM-07/TM-08/TM-06 review, candidate
   evidence. Stop before G6.

## Explicit exclusions

No WASI grants, fuel/memory/table quotas, or signature/trust-root
verification (PR-052). No guest SDK, authoring templates, or
published reference components (PR-053). No lockfile discovery, G6,
or `0.0.4` cut (PR-054). No registry download or marketplace. No
middleware or model-provider world. No native dylib ABI. No seventh
port. No kernel/runtime/SDK dependency on the host. No default SDK
feature that pulls Wasmtime. No G5/G6 decision. Do not restore
`tools/architecture/`.

## Validation

- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — no `wasmtime`, no `finstack-ai-plugin-host`
- `cargo tree -p finstack-ai --locked --no-default-features`
  — same
- `cargo tree -p finstack-ai-native-examples -p finstack-ai-wit --locked`
  — no `wasmtime`
- `cargo tree -p finstack-ai-plugin-host --locked`
  — has `wasmtime`; no `wasmtime-wasi`
- `cargo test -p finstack-ai-plugin-host --offline --locked`
- `cargo test -p finstack-ai-wit --offline --locked` (no host
  regression)
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check-wit`
- `mise run check-wasm`
- `uv run --no-project python tools/wasm_package/check.py graph`

Do not require `mise run ci` or Playwright to close the candidate.
Workspace clippy remains the gate. Adding this crate will make
`cargo test --workspace` compile Wasmtime; that is opt-in leaf
membership, not a default-bundle feature.

## Suggested authorization sentence

When ready to admit and implement:

```
Run PR-051; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```
