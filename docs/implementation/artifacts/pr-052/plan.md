# PR-052 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-052-permissions-limits-signatures`
Intended baseline: local `main` at `868c5b9727a1466ab7ac233d16e269191ccb056b`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-052. The closed PR-051 envelope
is not reused. PR-052 is the only active logical PR.

## Execution envelope

Authorized 2026-08-14 by `implement the plan` using the same local-only
integrated envelope as PR-049–PR-051. Suggested text that was accepted:

```
Run PR-052; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, tag, G5 inference,
G6 inference. Do not start PR-053+.

## Admission (when authorized)

- Dependency PR-051 is `Done` at local `main` merge
  `cf7eaebab383724fa4b7b19cb204febec41a768a`. Closeout commit
  `868c5b9727a1466ab7ac233d16e269191ccb056b` is the intended baseline.
- Phase 7 entrance is already `Passed` (3/3):
  `PH7-E-entrance-ports-d16eb3f550bd`,
  `PH7-E-entrance-record-context-81f85ef9d2cf`, and
  `PH7-E-entrance-adr-035-3c4a20fedca4`. Do not re-record.
- G5 is `Not ready`. That does not block Phase 7. Do not infer G5 or G6.
- ADR-035 stays `In progress` / `Partial`. Never mark Implemented
  (G6 owns closeout).
- ADR-010 / ADR-011 stay Partial. This PR is the WASI/limits/signature
  slice of ADR-010; it does not finish G6. ADR-014 stays Missing: WIT
  worlds remain neither remote nor process DTOs.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  keeps Wasmtime/`wasmtime-wasi` in the `plugins/` leaf, does not add a
  native dylib loader, does not change WIT compatibility policy, and
  does not add a seventh port.
- Threat Model section 18 is triggered (permissions, signing, trust
  roots, sandboxing, resource limits). Primary **TM-07** (deny-by-default
  WASI, explicit linked capabilities, fuel/epoch/memory/table/instance
  limits) and **TM-08** (digest/signature verification against configured
  trust roots). Also TM-06 labels and SEC-INV-006/007/008/011/012.
  Complete the review before merge.
  Lockfile discovery, guest SDK, reference components, hostile
  conformance suite completion, and the `0.0.4` cut remain PR-053–PR-054.

## Traceability

Implementation Plan PR-052; FR-PLG security; Architecture §15.3 and
§17 (not TDD middleware §17); TDD §27.3 (host capability interfaces
are not ambient WASI), §27.6 (host limits), and §31.5 (signature
policy is host configuration). TM-07 / TM-08 / TM-06. SEC-INV-006,
007, 008, 011, 012.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-052-A01: A component cannot access filesystem/network without an
  explicitly linked capability. Deny-by-default WASI: no ambient
  preopens, environment, or sockets. Ungranted `wasi:filesystem` /
  `wasi:sockets` / `wasi:http` / `wasi:cli` imports still fail
  instantiate as `plugin_instantiate_failed`. A permission *name* in
  the manifest is not a linked capability; linking happens only for
  the intersection of manifest request and host application grant,
  and only when the grant carries the concrete resource (preopen path
  or HTTP allowlist). Default host grants are `{logging, blobs}` so
  existing PR-051 fixtures keep working.
- PR-052-A02: Resource exhaustion terminates the call without
  destabilizing the host. Fuel, epoch-as-deadline, linear-memory,
  table, and instance ceilings are enforced on each store. Exhaustion
  maps to `plugin_resource_limit`. The host process continues and can
  create another store. Epoch interruption remains the cancellation
  channel; fuel/memory/table/instance are the resource-limit claim
  PR-051 deferred. `call_timeout_ms` is enforced (it is parsed today
  and unused).
- PR-052-A03: Strict signature mode rejects unsigned and untrusted
  packages. `SignaturePolicy::Permissive` keeps PR-050 behavior
  (unsigned allowed; malformed metadata fails). `SignaturePolicy::Strict`
  requires a present signature that verifies against a configured
  trust root. No trust roots in Strict ⇒ every package fails.
- PR-052-A04: Permission grants are visible in audit events without
  exposing secrets. Implementation Plan “run/effect diagnostics” and
  “audit events” map to existing observable `Metadata` (Architecture
  §17.4 durable-or-observable; TDD §12 `Diagnostic` / descriptor
  metadata never become `RunEvent` or journal bodies). Put
  `plugin.identity`, `plugin.manifest_digest`, and
  `plugin.granted_permissions` on `ToolsetDescriptor.metadata` /
  `ContextProviderDescriptor.metadata` and on `ToolError` /
  `ContextError` metadata. Context already carries
  `ComponentInvocation` on the descriptor (feeds `EffectRequested`).
  Do not add a `RunEvent` variant, kernel field, or public SDK
  constructor. Do not record signature bytes, public keys, or secret
  values.

Principal changes that are not extra acceptance IDs, but are required
to prove the four bullets:

- Host-owned grant intersection and selective WASI linking.
- Engine/store limit wiring and cache-fingerprint update.
- Ed25519 verify over the existing PR-050 manifest digest payload.
- `honor_deadline` honors `context.deadline` and `call_timeout_ms`.

This PR does **not** claim that WASM alone provides complete
application-level safety.

## Locked design

### Crate and graph

Workspace version stays `0.0.3`. `publish = false`. Native-only; do
not add `finstack-ai-plugin-host` to `mise run check-wasm`.

```text
finstack-ai-plugin-host
  -> wasmtime 47.0.3
  -> wasmtime-wasi 47.0.3   # NEW; deny-by-default ctx only
  -> ed25519-dalek          # NEW; host-side verify only
  -> finstack-ai-wit
  -> finstack-ai (default-features = false)
  -> finstack-ai-runtime (default-features = false)
  -> finstack-ai-kernel
```

Forbidden edges stay:

- kernel / runtime / SDK / protocol / bindings / rust-minimal /
  `finstack-ai-wit` / native-examples → `wasmtime`, `wasmtime-wasi`,
  or `finstack-ai-plugin-host`
- any `libloading` / native dylib loader
- default SDK feature that pulls the host

`wasmtime-wasi` is allowed **only** in `finstack-ai-plugin-host`.
Pin it to the same 47.0.3 line as `wasmtime` (re-check at implement;
MSRV ≤ 1.97.1). `default-features = false`. Do not enable ambient
stdio/env/network inherit helpers as crate features.

Update the PR-051 graph test: default bundles still contain no
`wasmtime` / `wasmtime-wasi` / `finstack-ai-plugin-host`. The host
tree **may** contain `wasmtime-wasi` and must still contain no
`libloading`.

Do not add WIT/plugin-host types to the public-rust-api corpus
(stays 131). Do not add `ExtensionTrust::IsolatedWasm`. Isolation
stays a property of `PluginHost`.

### Permission names and grants

Extend `ALLOWED_PERMISSIONS` in `plugins/finstack-ai-wit/src/manifest.rs`
to the Architecture §15.3 catalog, using the names PR-050 already
reserved as undeclared:

```text
logging, blobs, http, filesystem, network, secrets, clock, random
```

Unknown names still fail `plugin_registration_invalid`. Wit crate
tests that treated `http` / `filesystem` / `network` / `secrets` /
`clock` / `random` as undeclared must flip to “parses, does not
sandbox.” Compatibility fixtures under
`fixtures/compatibility/wit/v0.0.4/manifest` stay valid; do not add
a WIT world or package version bump.

Grant math (host-owned, not kernel):

```text
requested = manifest.permissions
offered   = PluginHostConfig.application_grants
granted   = requested ∩ offered
```

`PluginHostConfig::try_new` stays the existing three-argument
constructor. Defaults: grants `{logging, blobs}`,
`SignaturePolicy::Permissive`, empty trust roots, no filesystem
preopens, no HTTP allowlist, no socket grant. Add chainable `with_*`
methods that return `Result` for validation. Do not grow `try_new`
to a ten-argument constructor.

A host may offer more (`clock`, `random`, `http`, `filesystem`, …)
via `with_application_grants`. `secrets` may appear in a manifest;
this PR ships no secret provider, so `with_application_grants`
rejects `secrets` as `plugin_registration_invalid`. That reserves
the name without inventing a secret store (PR-056).

Concrete resource config (TDD §27.6), also via `with_*`:

| Grant | Required extra config before linking |
| --- | --- |
| `filesystem` | at least one preopen `{guest_path, host_path, read, write}` |
| `http` | non-empty hostname allowlist (no credentials, no URLs with userinfo) |
| `network` | explicit socket-grant flag; this PR does not open ambient sockets |
| `clock` / `random` | none; linking the matching WASI interface is enough |
| `logging` / `blobs` | none; keep existing finstack host imports |

Tests use a tempdir for any preopen. Do not inherit `$HOME`, stdio,
or the process environment. A positive “granted + preopen ⇒
`wasi:filesystem` is linked” instantiate test is allowed if cheap;
the guest need not perform real I/O. Do not ship a general-purpose
filesystem tool (PR-053).

Load-time rules:

| Situation | Result |
| --- | --- |
| Guest imports `wasi:filesystem` / `wasi:sockets` / `wasi:http` / `wasi:cli` and that capability is not in `granted` **or** lacks concrete resources | `plugin_instantiate_failed` |
| Manifest requests `filesystem`/`http`/`network` and host does not offer it | `plugin_permission_denied` at `load`, before compile |
| Host offers `filesystem` but configures no preopen | do not link `wasi:filesystem` |
| Host offers `http` but configures no allowlist | do not link `wasi:http` |
| Host offers `network` but sets no socket-grant flag | do not link `wasi:sockets` |
| `clock` / `random` in `granted` | link only those WASI interfaces |
| `logging` / `blobs` in `granted` | keep existing finstack host imports |
| `logging` / `blobs` requested but not granted | do not add those host imports; instantiate fails if the guest imports them |

Do **not** add new `finstack:ai-*` WIT packages. Clock/random/fs/http
use WASI interfaces when linked. Logging/blobs stay
`finstack:ai-host@0.0.4`.

Do **not** call a blanket `add_to_linker` that defines every WASI
package. Prefer per-interface link helpers. If wasmtime-wasi 47 only
exposes a blanket add, wrap linking so ungranted FS/network/CLI
imports remain undefined.

In-process `finstack-ai-wit` adapters still inherit host authority.
Expanding the allowed permission *names* there is parse-only. Do not
call in-process guests sandboxed.

### WASI context

```text
PluginHost::try_new
  -> Engine (fuel + epoch + StoreLimits support)
  -> toolset/context linkers with finstack host imports
load(bytes, manifest)
  -> validate_manifest (PR-050)
  -> intersect grants
  -> verify signature per SignaturePolicy
  -> compile/cache (PR-051)
instantiate
  -> fresh deny-by-default WasiCtx (no inherit env/stdio/network, no preopens)
  -> add only granted, resource-backed WASI interfaces
  -> apply StoreLimits + fuel + epoch deadline
```

No ambient environment or filesystem is inherited (Architecture
§15.3 / SEC-INV-006). Guest paths, URLs, and capability names stay
untrusted.

### Resource limits

Keep `PluginResourceLimits.{max_output_bytes, call_timeout_ms}`.
`0` stays invalid. Add optional fields with `#[serde(default)]`:

```text
max_memory_bytes: Option<u64>
max_tables: Option<u32>
max_instances: Option<u32>
fuel: Option<u64>
```

These are host-side manifest fields, not WIT records. Existing
fixtures without them still parse.

When a field is omitted, `PluginHostConfig` supplies experimental
defaults (host-owned, not a published SLA):

| Field | Default if omitted |
| --- | --- |
| `fuel` | `1_000_000` (raise the host default if echo/add/collect cannot complete; do not disable fuel) |
| `max_memory_bytes` | `16 * 1024 * 1024` |
| `max_tables` | `1` |
| `max_instances` | `1` |
| `call_timeout_ms` | `5_000` if `resource_limits` is absent |
| `max_output_bytes` | existing context `map_budget` / 1 MiB RawJson ceiling |

`max_concurrent_instances` (PR-051) is the host concurrent-`Store`
ceiling. `max_instances` is the Wasmtime per-store instance ceiling.
Do not conflate them.

Engine: enable `consume_fuel` and a `StoreLimits` / resource limiter.
Update `CONFIG_FINGERPRINT` so the compile cache misses (for example
`async=true,epoch=true,fuel=true,limiter=true,compiler=cranelift`).
Do not enable wasmtime's implicit filesystem `cache` feature.

`honor_deadline` must honor `context.deadline` (it ignores it today
with `let _ = context.deadline`). Wasm adapters populate
`ComponentConstructionContext.deadline` from the earlier of the
incoming construction deadline and `now + effective call_timeout_ms`.
`with_cancellation` also watches that Instant (or a timeout that
cancels the signal) and increments the engine epoch. Fired deadline
or cancel still maps to `plugin_lifecycle_timeout`. Fuel / limiter
exhaustion maps to `plugin_resource_limit`. Do not conflate the two.

Fuel-out / limiter exhaustion → `plugin_resource_limit`, not
`plugin_trap`. After exhaustion, a new store must still run.

A WAT or fixture guest that burns fuel (tight loop) and one that
grows memory past the ceiling are enough for A02. Do not require a
published hostile suite (PR-054 / G6).

### Signatures and trust roots

`PluginSignature` stays `{algorithm, key_id, signature}`. Verification
lives in `finstack-ai-plugin-host`, not the wit crate (keep wit
crypto-free). Wit still rejects empty/malformed signature fields.

```text
pub enum SignaturePolicy {
    Permissive, // default
    Strict,
}
```

Signed payload: the same canonical `{identity, version, worlds}`
bytes already hashed by `manifest_digest_hex`. Algorithm lock:
`ed25519`. `signature` is lowercase hex of the 64-byte signature.
`key_id` is a non-secret label looked up in
`PluginHostConfig.trust_roots: BTreeMap<String, [u8; 32]>`.

| Policy | Unsigned | Present + verifies | Present + bad/unknown key |
| --- | --- | --- | --- |
| Permissive | allow | allow | `plugin_signature_untrusted` |
| Strict | `plugin_signature_untrusted` | allow | `plugin_signature_untrusted` |
| Strict, empty trust roots | fail | fail | fail |

Do not sign component bytes here. Component-bytes digest remains the
PR-051 cache identity. Lockfile binding of bytes↔manifest is PR-054.
Do not search a registry. Do not implement publisher discovery.

### Diagnostics (A04)

Do not add `RunEvent` variants, journal fields, or public SDK
constructors. `Diagnostic` values still must not become journal
bodies (TDD §12).

Build `Metadata` via `Metadata::parse` with a bounded object:

```json
{
  "plugin.identity": "finstack.plugin.echo.toolset",
  "plugin.manifest_digest": "<hex>",
  "plugin.granted_permissions": ["logging"]
}
```

Attach it to `ToolsetDescriptor.metadata` /
`ContextProviderDescriptor.metadata` at adapter construction, and to
`ToolError` / `ContextError` metadata (today both use
`Metadata::empty()`). Tests read those objects. `ToolsetDescriptor`
has no `ComponentInvocation` field; metadata is the shared
observation channel. No signature bytes, keys, preopen paths that
embed host home directories, or secret values.

### Stable codes (host-side, not WIT exports)

| Code | When |
| --- | --- |
| `plugin_permission_denied` | request not in host grants, or grant lacks concrete resources |
| `plugin_resource_limit` | fuel / memory / table / instance exhaustion |
| `plugin_signature_untrusted` | Strict unsigned, or signature/key mismatch |

Add matching `PluginHostError` variants and `code()` arms. Reuse
`plugin_instantiate_failed`, `plugin_trap`,
`plugin_lifecycle_timeout`, `plugin_registration_invalid`,
`plugin_manifest_digest_mismatch`, `plugin_payload_too_large`.
Wrap into `ToolError` / `ContextError` with `ErrorCategory::Plugin`
the same way PR-050/PR-051 wrap codes. Do not add public SDK
constructors. Do not add these codes to the public-rust-api corpus.

### Test guests and fixtures (not the PR-053 SDK)

Keep echo / reference-context / trap. Add test-only guests under
`plugins/finstack-ai-plugin-host/fixtures/guests/`. Not workspace
members. `publish = false`. Examples must not depend on them.

1. `wasi-fs-ungranted` — WAT (or encoded component) that imports
   `wasi:filesystem` (47.x / 0.2 equivalent). Prove at instantiate:
   no filesystem grant + preopen → `plugin_instantiate_failed`.
   Does not need to be a full toolset world.
2. `wasi-sockets-ungranted` — same shape for `wasi:sockets` or
   `wasi:http`.
3. `fuel-burner` — Rust toolset guest (same toolchain as trap) with
   a tight-loop / `memory.grow` tool so A02 can trip
   `plugin_resource_limit` through the adapter, then run echo on a
   fresh store.

Prefer WAT for the negative WASI guests so they do not pull
wit-bindgen WASI into the happy-path echo toolchain. Extend
`GUEST_NAMES` in `tools/plugin_wasm/generate.py` only for guests
that use that Rust encode path.

JSON fixtures (not wasm) for A03:

- unsigned manifest (Permissive allow, Strict deny)
- signed-valid (both allow when the test root is configured)
- signed-wrong-key / tampered payload (both deny)

Regenerate Rust-guest wasm through `mise run gen-plugin-wasm` /
`check-plugin-wasm`. WAT negative guests are checked in as source
and encoded in the host test harness or a small helper; they do
not join `GUEST_NAMES` unless they use the Rust encode path.

Do not add WIT fixtures to the public-rust-api corpus. Do not cut
`0.0.4`. Do not publish crates.

### Labels (TM-06)

`PluginHost` remains isolated Wasmtime (T3). In-process wit adapters
remain host-authority. Adding `wasmtime-wasi` does not make the
in-process path a sandbox. The exclusion stands: WASM alone is not
complete application-level safety.

## File map

Create:

- `docs/implementation/artifacts/pr-052/README.md` (this folder)
- `plugins/finstack-ai-plugin-host/src/grants.rs`
- `plugins/finstack-ai-plugin-host/src/signature.rs`
- `plugins/finstack-ai-plugin-host/src/limits.rs` — StoreLimits / fuel /
  timeout wiring (or fold into `instantiate.rs` if smaller)
- `plugins/finstack-ai-plugin-host/fixtures/guests/{wasi-fs-ungranted,wasi-sockets-ungranted,fuel-burner}/`
- `plugins/finstack-ai-plugin-host/fixtures/manifests/` — unsigned /
  signed-valid / signed-untrusted JSON

Modify:

- `plugins/finstack-ai-wit/src/manifest.rs` — allowed permission names;
  optional limit fields
- `plugins/finstack-ai-wit/src/lifecycle.rs` — `honor_deadline` honors
  `context.deadline`
- `plugins/finstack-ai-plugin-host/src/host.rs` — config, grants,
  signature policy, engine fuel/limiter
- `plugins/finstack-ai-plugin-host/src/instantiate.rs` — selective WASI
  link, trap/limit map
- `plugins/finstack-ai-plugin-host/src/adapters.rs` — metadata, timeout
- `plugins/finstack-ai-plugin-host/src/cache.rs` — fingerprint
- `plugins/finstack-ai-plugin-host/src/error.rs` — three new codes
- `plugins/finstack-ai-plugin-host/src/lib.rs` — graph test
- `Cargo.toml` / `Cargo.lock` — `wasmtime-wasi`, `ed25519-dalek`
- `plugins/README.md`, host/wit READMEs
- `tools/plugin_wasm/generate.py` — `fuel-burner` only if it uses
  the Rust encode path; leave WAT guests out of `GUEST_NAMES`
- `docs/implementation/*` registers only after admit / evidence

Do not modify `docs/planning/`. Do not restore `tools/architecture/`.
Do not edit `examples/rust-minimal`. Do not add plugin-host to
`check-wasm`.

## Tasks (when admitted)

Task IDs are minted at admit, not now.

1. Tracking: confirm PR-051 Done, record exclusions, open
   `codex/pr-052-permissions-limits-signatures` from the named
   baseline. Do not re-record Phase 7 entrance.
2. Grants: permission names, host/application intersection, selective
   WASI link, ungranted FS/network instantiate fail-closed (A01).
3. Limits: fuel, StoreLimits, `call_timeout_ms` / deadline, fingerprint
   update, exhaustion contained (A02).
4. Signatures: Permissive/Strict, ed25519 trust roots, unsigned and
   untrusted fixtures (A03).
5. Diagnostics: grant/identity metadata on descriptors and port
   errors; no secrets (A04).
6. Fixtures: three new guests + signed manifests; echo still works
   under default grants; graph tests updated for `wasmtime-wasi`.
7. Evidence: focused validation, TM-07/TM-08/TM-06 review, candidate
   evidence. Stop before G6.

## Explicit exclusions

No guest SDK, authoring templates, or published reference components
(PR-053). No lockfile discovery, G6, or `0.0.4` cut (PR-054). No
registry download or marketplace. No middleware or model-provider
world. No native dylib ABI. No seventh port. No kernel/runtime/SDK
dependency on the host. No default SDK feature that pulls Wasmtime.
No `ExtensionTrust` variant. No secret provider. No claim that WASM
alone is complete application-level safety. No G5/G6 decision. Do
not restore `tools/architecture/`.

## Validation

- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — no `wasmtime`, no `wasmtime-wasi`, no `finstack-ai-plugin-host`
- `cargo tree -p finstack-ai --locked --no-default-features` — same
- `cargo tree -p finstack-ai-native-examples -p finstack-ai-wit --locked`
  — no `wasmtime`, no `wasmtime-wasi`
- `cargo tree -p finstack-ai-plugin-host --locked`
  — has `wasmtime` and `wasmtime-wasi`; no `libloading`. Flip the
  PR-051 host graph test that currently forbids `wasmtime-wasi`.
- `cargo test -p finstack-ai-plugin-host --offline --locked`
- `cargo test -p finstack-ai-wit --offline --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check-wit`
- `mise run check-plugin-wasm`
- `mise run check-wasm`
- `uv run --no-project python tools/wasm_package/check.py graph`

Do not require `mise run ci` or Playwright to close the candidate.
Workspace clippy remains the gate.

## Suggested authorization sentence

When ready to admit and implement:

```
Run PR-052; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```
