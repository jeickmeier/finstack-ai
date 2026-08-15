# PR-053 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-053-guest-sdk-reference-components`
Intended baseline: local `main` at `d0e165d7be3ff3dbac4688c16351f6ca002896aa`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-053. The closed PR-052 envelope
is not reused. PR-053 is the only active logical PR.

## Execution envelope

Authorized 2026-08-14 by `implement the plan` using the same local-only
integrated envelope as PR-049–PR-052. Suggested text that was accepted:

```
Run PR-053; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io publish,
tag, G5 inference, G6 inference. Do not start PR-054+. Do not cut
workspace `0.0.4`.

## Admission (when authorized)

- Dependency PR-052 is `Done` at local `main` merge
  `5e531591350a8838ff7da06f50a5dc2d52b31892`. Closeout commit
  `d0e165d7be3ff3dbac4688c16351f6ca002896aa` is the intended baseline.
- Phase 7 entrance is already `Passed` (3/3):
  `PH7-E-entrance-ports-d16eb3f550bd`,
  `PH7-E-entrance-record-context-81f85ef9d2cf`, and
  `PH7-E-entrance-adr-035-3c4a20fedca4`. Do not re-record.
- G5 is `Not ready`. That does not block Phase 7. Do not infer G5 or G6.
- ADR-035 stays `In progress` / `Partial`. Never mark Implemented
  (G6 owns closeout). This PR documents exact-world `@0.0.4` pinning
  and a migration example; it does not add `@0.0.5` or `@1.0.0`.
- ADR-010 / ADR-011 stay Partial. This PR is the guest-authoring slice
  of ADR-010; it does not finish G6. ADR-014 stays Missing: WIT worlds
  remain neither remote nor process DTOs.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  keeps Wasmtime/`wasmtime-wasi` in the `plugins/` host leaf, does not
  add a native dylib loader, does not change WIT compatibility policy
  or published world versions, and does not add a seventh port.
- Threat Model section 18 is triggered for a published
  filesystem-consuming guest (sandboxing / high-privilege-adjacent
  filesystem battery). Primary **TM-06** (published references stay
  isolated T3; they are not the trusted native filesystem battery)
  and **TM-07** (template and calculator import only world-required
  host interfaces; filesystem sandbox imports `wasi:filesystem@0.2.12`
  and still fail closed without grant+preopen). Do not change host
  grant math, signature policy, or resource-limit defaults. Complete
  the review before merge.
  Lockfile discovery, hostile conformance-suite completion, G6, and
  the `0.0.4` cut remain PR-054.

## Traceability

Implementation Plan PR-053; FR-PLG authoring; Architecture §15.1–15.5
(guest authoring, not host); TDD §27.1 (generated guest bindings,
pinned package versions) and milestone 7 (reference components).
ADR-010 / ADR-035. TM-06 / TM-07. SEC-INV-006 / 007 / 011 / 012.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-053-A01: A new component project can build and run from the
  template. `mise run check-plugin-template` copies
  `plugins/templates/toolset-plugin/` to a tempdir, rewrites only the
  guest-sdk path if needed, `cargo build --target wasm32-unknown-unknown
  --release`, encodes with the existing `tools/plugin_wasm/encoder`,
  then loads the component through `PluginHost` / `WasmToolsetAdapter`
  and successfully calls the template's echo tool. The template
  requests only `{logging}`. A context-plugin template exists and
  builds the same way; the toolset template is the A01 run proof.
- PR-053-A02: Reference components pass native-trait and WIT
  conformance equivalents.
  - Calculator: `WasmToolsetAdapter` over the published calculator
    wasm passes `finstack_ai_test::check_toolset_conformance` for the
    same add/multiply/divide-by-zero/overflow cases as
    `finstack-ai-tools-calculator` (`evaluate` semantics, codes
    `calculator_invalid_arguments` / `calculator_arithmetic_error`,
    result `{"result": ...}`). Native `CalculatorToolset` stays the
    trusted in-process battery; the plugin uses identity
    `finstack.plugin.calculator` and tool id
    `finstack.plugin.calculator` so the two batteries can coexist.
  - Context: `WasmContextAdapter` over the published context-provider
    wasm passes `check_context_conformance` for the same two-item
    quoted-source + reference contribution as the in-process
    `finstack-ai-wit` `ReferenceContext` guest (same item JSON).
  - Filesystem sandbox: `WasmToolsetAdapter` lists and reads a file
    under a host tempdir preopen when `filesystem` is granted; without
    grant+preopen, instantiate stays `plugin_instantiate_failed` /
    load stays `plugin_permission_denied`. This is **not** claimed
    equivalent to `finstack-ai-tools-filesystem` (trusted native,
    Unix no-follow). Read-only list/read only; no write, glob, or
    shell.
- PR-053-A03: Generated code versions are pinned and reproducible.
  `wit-bindgen` `0.57.1` (`default-features = false`,
  `macros` + `realloc`). Vendored guest WIT is byte-identical to
  `plugins/finstack-ai-wit/wit/v0.0.4/`. `mise run check-guest-sdk`
  and `mise run check-plugin-wasm` (now including reference
  components) fail on drift. Workspace crate version stays `0.0.3`.
  Packages stay `@0.0.4`. `@1.0.0` generation remains blocked.
- PR-053-A04: Examples avoid direct dependence on host-private crates.
  Guest SDK, templates, and reference component `Cargo.toml` files
  depend on `finstack-ai-guest-sdk` (and through it `wit-bindgen` /
  `serde` / digest crates) only. They must not depend on
  `finstack-ai-plugin-host`, `finstack-ai-wit`, `finstack-ai`,
  `finstack-ai-kernel`, `finstack-ai-runtime`, `wasmtime`, or
  `wasmtime-wasi`. Host-side proofs live in
  `finstack-ai-plugin-host` tests, not in guest manifests.

Principal changes that are not extra acceptance IDs, but are required
to prove the four bullets:

- Rust guest SDK with generated-binding macros, catalog/schema/error
  helpers, logging size checks, and native test fixtures.
- Published calculator, filesystem-sandbox, and context-provider
  components under `plugins/reference/`.
- Component build templates, pinned toolchain instructions, and
  local host test commands (`mise` + documented `cargo test` filters).
- Version-negotiation / migration examples in guest-sdk docs
  (exact-world `@0.0.4`; a later `@0.0.5` would retarget; this PR
  does not add that world).

`Publish` means in-tree author-facing artifacts with `publish = false`.
It does not mean crates.io, npm, or a workspace version cut.

## Locked design

### Crate and graph

Workspace version stays `0.0.3`. `publish = false`. Do not add
`finstack-ai-plugin-host` or `finstack-ai-guest-sdk` to
`mise run check-wasm`. Do not add guest-sdk or plugin types to the
public-rust-api corpus (stays 131). Do not add
`ExtensionTrust::IsolatedWasm`. Isolation stays a property of
`PluginHost`.

```text
finstack-ai-guest-sdk          # NEW workspace member; native-compilable
  -> serde / serde_json
  -> serde_json_canonicalizer
  -> sha2
  -> thiserror
  -> wit-bindgen 0.57.1        # re-export + macros only; no wasmtime

plugins/reference/*            # NOT workspace members; cdylib guests
  -> finstack-ai-guest-sdk (path)

plugins/templates/*            # NOT workspace members; copied by mise
  -> finstack-ai-guest-sdk (path)

finstack-ai-plugin-host        # unchanged deps; loads wasm bytes
  -> wasmtime / wasmtime-wasi / ed25519-dalek / finstack-ai-wit / ...
  # must NOT depend on finstack-ai-guest-sdk
```

Forbidden edges:

- kernel / runtime / SDK / protocol / bindings / rust-minimal /
  `finstack-ai-wit` / native-examples → `finstack-ai-guest-sdk`,
  `wasmtime`, `wasmtime-wasi`, or `finstack-ai-plugin-host`
- guest-sdk / templates / reference components → `finstack-ai`,
  kernel, runtime, `finstack-ai-wit`, `finstack-ai-plugin-host`,
  `wasmtime`, `wasmtime-wasi`, `libloading`
- any default SDK feature that pulls the host or the guest SDK
- any `libloading` / native dylib loader

Add `finstack-ai-guest-sdk` to workspace `members` and
`workspace.dependencies`. Add `wit-bindgen` `0.57.1` to
`workspace.dependencies` with `default-features = false` and
features `["macros", "realloc"]`. Guest-sdk is the only workspace
crate that uses it.

Update the PR-052 graph test: default bundles and wit/native-examples
still contain no `wasmtime` / `wasmtime-wasi` /
`finstack-ai-plugin-host`. They must also contain no
`finstack-ai-guest-sdk`. Host tree still has wasmtime + wasmtime-wasi,
still has no `libloading`, and must not gain `finstack-ai-guest-sdk`.
Add `cargo tree -p finstack-ai-guest-sdk` in the guest-sdk crate
tests: no host-private crates, no tokio, no wasmtime.

### Guest SDK

Create `plugins/finstack-ai-guest-sdk`. Native library crate
(`crate-type` default `lib`). Compiles on the host target so
`cargo test -p finstack-ai-guest-sdk` and workspace clippy work.
No `cdylib`. No `export!`. Guests invoke the macros in *their* crate.

Public surface (names locked):

```rust
pub const WIT_BINDGEN_VERSION: &str = "0.57.1";
pub const WIT_PACKAGE_VERSION: &str = "0.0.4";

pub const MAX_STRING_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_RAW_JSON_BYTES: usize = 1_048_576;
pub const MAX_METADATA_BYTES: usize = 64 * 1024;

pub struct GuestError { /* code, message, retryable */ }
pub fn plugin_error(code: &str, message: &str) -> GuestError;

pub fn require_sanitized_context(
    tenant_scope: &str,
    authorization_decision_id: &str,
) -> Result<(), GuestError>;

pub struct ToolSpecParts { /* catalog fields as owned values */ }
pub fn catalog_digest(tools: &[ToolSpecParts]) -> Result<String, GuestError>;

pub fn parse_args<T: serde::de::DeserializeOwned>(
    args_json: &[u8],
) -> Result<T, GuestError>;
pub fn encode_json_result<T: serde::Serialize>(
    value: &T,
) -> Result<Vec<u8>, GuestError>;
pub fn schema_bytes(value: &serde_json::Value) -> Result<Vec<u8>, GuestError>;

pub enum LogLevel { Trace, Debug, Info, Warn, Error }
pub fn reject_log_message(message: &str) -> Result<(), GuestError>;

pub fn fixture_tenant_scope() -> &'static str;      // "tenant-a"
pub fn fixture_decision_id() -> &'static str;       // "decision-v1"
```

`catalog_digest` must match `finstack_ai_wit::catalog_digest_hex`:
JCS-canonical `{ "tools": [ canonical tool objects ] }` then
`Digest::raw_json` / the same `finstack-ai\0raw-json\0` + version
prefix the echo fixture already uses. A guest-sdk unit test locks
one golden digest against a fixed `ToolSpecParts`. If the digest
ever disagrees with the host mapper, registration fails
`plugin_catalog_digest_mismatch`; do not invent a second algorithm.

`parse_args` / `encode_json_result` / `schema_bytes` reject above
`MAX_RAW_JSON_BYTES` before allocating a second copy.
`require_sanitized_context` fails `plugin_call_context_invalid`
when either field is empty.

Macros (caller's crate, because `export!` must live there):

```rust
finstack_ai_guest_sdk::toolset_plugin!();
finstack_ai_guest_sdk::context_plugin!();
```

Each expands `wit_bindgen::generate!` for `toolset-plugin` or
`context-plugin` with `generate_all` and `path` = the guest's
`./wit` directory (see WIT vendor below). Re-export `wit_bindgen`
from the SDK so guest `Cargo.toml` files do not list it.

Do **not** call host imports from the SDK crate. After the macro,
guests call `finstack::ai_host::logging::log` (and `blobs::read`
when needed). The SDK owns size checks and the documented pattern,
not a second logging ABI.

Do **not** put calculator `evaluate` in the SDK. The SDK stays
world-generic.

Native tests in the SDK crate: digest golden, empty-context reject,
oversized args/result/log reject, schema_bytes round-trip, fixture
helpers. No Wasmtime.

### WIT vendor and generated-tree checks

Canonical packages remain `plugins/finstack-ai-wit/wit/v0.0.4/`.
Do not edit those files in this PR. Do not add a fifth finstack WIT
package. Do not change `ALLOWED_WORLDS`
(`toolset-plugin`, `context-plugin`).

`mise run gen-guest-sdk` copies the four canonical packages plus a
host-style `resolve.wit` (same shape as
`plugins/finstack-ai-plugin-host/wit/resolve.wit`, package name
`finstack:guest-sdk-bindgen@0.0.4`, not a published world) into:

```text
plugins/finstack-ai-guest-sdk/wit/
plugins/templates/toolset-plugin/wit/
plugins/templates/context-plugin/wit/
plugins/reference/calculator/wit/
plugins/reference/filesystem-sandbox/wit/
plugins/reference/context-provider/wit/
```

`mise run check-guest-sdk` fails if any copy drifts from canonical
`v0.0.4`. Hand-editing vendored WIT is forbidden.

Filesystem sandbox additionally vendors the minimal
`wasi:filesystem/types@0.2.12` and
`wasi:filesystem/preopens@0.2.12` WIT under
`plugins/reference/filesystem-sandbox/wit/deps/`. Pin `@0.2.12`
(wasmtime-wasi 47 links that version, not `@0.2.0`). Record the
upstream source and digest in that guest's README. Do not vendor
clocks, sockets, HTTP, or CLI. Do not add WASI to the calculator,
context provider, or templates.

The sandbox may use a **guest-local** world that `include`s
`toolset-plugin` and imports those two WASI interfaces. Manifest
`worlds` stays `["toolset-plugin"]`. Do not add the local world
name to `ALLOWED_WORLDS`.

### Templates

```text
plugins/templates/toolset-plugin/
  Cargo.toml
  src/lib.rs
  plugin.manifest.json
  wit/                    # generated copy
  README.md
plugins/templates/context-plugin/
  ...
```

Toolset template: one `finstack.plugin.template.echo` tool, identity
`finstack.plugin.template.toolset`, permissions `["logging"]`,
worlds `["toolset-plugin"]`, version `0.0.4`. Uses SDK macros and
helpers only. No filesystem, network, clock, or random imports.

Context template: one bounded untrusted item, identity
`finstack.plugin.template.context`, worlds `["context-plugin"]`.

`Cargo.toml`: `publish = false`, `[workspace]`, `crate-type = ["cdylib"]`,
edition 2024, rust-version 1.97.1, path dep on
`../../finstack-ai-guest-sdk`. `check-plugin-template` copies to a
tempdir and rewrites that path to the repo guest-sdk absolute path
so the copy still builds.

No `cargo-generate` crate. No Python-to-component packaging.

### Reference components

Not workspace members. `publish = false`. Built and encoded by
extending `tools/plugin_wasm/generate.py`:

```python
GUEST_NAMES = ("echo-toolset", "trap-toolset", "fuel-burner")
REFERENCE_NAMES = ("calculator", "filesystem-sandbox", "context-provider")
```

`reference-context` is removed from `GUEST_NAMES`. Host tests that
loaded `fixtures/guests/reference-context/component.wasm` retarget
to `plugins/reference/context-provider/component.wasm`. Delete the
old fixture crate after retarget. Echo / trap / fuel-burner stay
test-only.

| Component | Identity | Worlds | Permissions | Tools / collect |
| --- | --- | --- | --- | --- |
| calculator | `finstack.plugin.calculator` | `toolset-plugin` | `logging` | one tool `finstack.plugin.calculator` / model-name `calculator` |
| filesystem-sandbox | `finstack.plugin.filesystem.sandbox` | `toolset-plugin` | `logging`, `filesystem` | `finstack.plugin.filesystem.list`, `finstack.plugin.filesystem.read` |
| context-provider | `finstack.plugin.reference.context` | `context-plugin` | `logging` | same two items as today's in-process / fixture guest |

Calculator copies the native `evaluate` rules (max 1,024 finite
operands; empty add = 0; empty multiply = 1; subtract/divide need a
first operand; divide-by-zero and non-finite → arithmetic error).
Input/output JSON schemas match the native battery. `execution-mode`
is `parallel`. Do not depend on `finstack-ai-tools-calculator`.

Filesystem sandbox: read-only. Args `{"path":"..."}`. Reject `..`,
NUL, empty, and absolute host paths in the guest (defense in depth;
the preopen is the real boundary). `list` returns
`{"entries":[...]}`. `read` returns bounded UTF-8 or hex bytes in
JSON under `max-result-bytes`. Tests use a tempdir preopen; do not
inherit `$HOME`, stdio, or the process environment. No write tool.

Context provider: keep the existing quoted-source and reference
`item-json` bytes so WIT mapping and `check_context_conformance`
stay stable.

Checked-in `component.wasm` for each reference. Manifest JSON next
to each component (unsigned; `SignaturePolicy::Permissive`). Do not
sign component bytes. Lockfile binding of bytes↔manifest is PR-054.

Echo-toolset may switch to the guest SDK (path dep) so catalog
digest is not copied a third time. Trap and fuel-burner may keep
raw `wit-bindgen` if cheaper.

### Toolchain and local host commands

Document in `plugins/finstack-ai-guest-sdk/README.md` and
`plugins/README.md`:

```text
rust-version: 1.97.1
target: wasm32-unknown-unknown
wit-bindgen: 0.57.1
WIT packages: finstack:ai-*@0.0.4
encode: tools/plugin_wasm/encoder (existing ComponentEncoder)
```

Do not add `wasm32-wasip2`, `cargo-component`, or a second encoder
unless the filesystem sandbox cannot encode WASI component imports
with the current encoder. First try: wit-bindgen component-level
`wasi:filesystem@0.2.12` imports on `wasm32-unknown-unknown`, then
the existing encoder. If that fails, extend the encoder only as far
as those two imports; do not adopt a general WASI preview adapter
stack.

Local commands (add mise tasks; do not assume they exist today):

```text
mise run gen-guest-sdk
mise run check-guest-sdk
mise run gen-plugin-wasm          # fixtures + REFERENCE_NAMES
mise run check-plugin-wasm
mise run check-plugin-template    # A01
cargo test -p finstack-ai-guest-sdk --offline --locked
cargo test -p finstack-ai-plugin-host --offline --locked
```

Document host test filters for calculator, context, sandbox, and
template. Do not add a plugin path to `examples/rust-minimal`.

### Version negotiation and migration examples

Document only. Do not implement a second world version.

- Guests pin manifest `version` `0.0.4` and worlds
  `toolset-plugin` or `context-plugin`.
- Host exact-world: unknown world or `@1.0.0` package version fails
  closed (`plugin_registration_invalid`).
- Linking `finstack:ai-host` logging/blobs is not ambient WASI.
- A later breaking pre-1.0 change increments the WIT package/world
  and requires adapters plus fixtures in the same change; historical
  `@0.0.4` fixtures stay under `fixtures/compatibility/wit/v0.0.4/`.
- WASM guests do not speak the process-protocol hello. FR-PLG-004
  version negotiation remains the later process adapter (PR-058).
- Point at `plugins/finstack-ai-wit/COMPATIBILITY.md` and ADR-035.

Put this in `plugins/finstack-ai-guest-sdk/README.md` and a short
`plugins/finstack-ai-guest-sdk/MIGRATION.md` with a worked
`@0.0.4` → hypothetical `@0.0.5` retarget (new `path`, new
`ALLOWED_WORLDS` entry, rebuild, keep old fixtures). The
hypothetical world is documentation, not code.

### Labels (TM-06)

`PluginHost` remains isolated Wasmtime (T3). In-process
`finstack-ai-wit` adapters remain host-authority. The published
filesystem sandbox is a T3 fixture that uses a granted preopen; it
is not `finstack-ai-tools-filesystem` and is not a sandbox for
in-process guests. The exclusion stands: WASM alone is not complete
application-level safety.

## File map

Create:

- `docs/implementation/artifacts/pr-053/README.md` (this folder)
- `plugins/finstack-ai-guest-sdk/` — crate, vendored `wit/`, README,
  MIGRATION.md, native tests
- `plugins/templates/toolset-plugin/`
- `plugins/templates/context-plugin/`
- `plugins/reference/calculator/`
- `plugins/reference/filesystem-sandbox/`
- `plugins/reference/context-provider/`
- `tools/plugin_wasm/sync_guest_wit.py` (or fold into generate.py)
- `tools/plugin_wasm/template_check.py`

Modify:

- `Cargo.toml` / `Cargo.lock` — guest-sdk member, wit-bindgen pin
- `mise.toml` — `gen-guest-sdk`, `check-guest-sdk`,
  `check-plugin-template`; update `gen-plugin-wasm` /
  `check-plugin-wasm` descriptions
- `tools/plugin_wasm/generate.py` — `REFERENCE_NAMES`; drop
  `reference-context` from `GUEST_NAMES`
- `plugins/finstack-ai-plugin-host/src/fixture_tests.rs` (and any
  adapter tests) — retarget context wasm; add calculator /
  sandbox / template-run proofs
- `plugins/finstack-ai-plugin-host/src/lib.rs` — graph test
- `plugins/README.md`, host/wit READMEs, guest-sdk README
- `plugins/finstack-ai-wit/COMPATIBILITY.md` — one paragraph
  pointing authors at the guest SDK (no policy change)
- echo-toolset `Cargo.toml` / `src/lib.rs` if migrated to the SDK
- `docs/implementation/*` registers only after admit / evidence

Delete after retarget:

- `plugins/finstack-ai-plugin-host/fixtures/guests/reference-context/`

Do not modify `docs/planning/`. Do not restore `tools/architecture/`.
Do not edit `examples/rust-minimal`. Do not add plugin-host or
guest-sdk to `check-wasm`. Do not grow `ALLOWED_WORLDS` or
`ALLOWED_PERMISSIONS`. Do not change grant math, signature policy,
or default resource limits.

## Tasks (when admitted)

Task IDs are minted at admit, not now.

1. Tracking: confirm PR-052 Done, record exclusions, open
   `codex/pr-053-guest-sdk-reference-components` from the named
   baseline. Do not re-record Phase 7 entrance.
2. Guest SDK: crate, WIT vendor, helpers, macros, native tests,
   graph edges (A03 / A04 foundation).
3. Templates: toolset + context skeletons, pinned toolchain docs,
   `check-plugin-template` build-and-run (A01).
4. Reference calculator and context-provider: encode, retarget
   host context tests, native-trait + WIT conformance equivalents
   (A02).
5. Filesystem sandbox: guest-local WASI imports at `@0.2.12`,
   read-only list/read, grant+preopen positive and deny-by-default
   negative (A02).
6. Docs and graph: MIGRATION.md, COMPATIBILITY pointer, echo
   migration if cheap, A04 cargo-tree/grep proofs.
7. Evidence: focused validation, TM-06/TM-07 review, candidate
   evidence. Stop before G6.

## Explicit exclusions

No lockfile discovery, G6, or `0.0.4` cut (PR-054). No crates.io /
npm / pypi publish. No Python-to-component automatic packaging.
No registry download or marketplace. No middleware or model-provider
world. No native dylib ABI. No seventh port. No kernel/runtime/SDK
dependency on the host or guest SDK. No default SDK feature that
pulls Wasmtime or the guest SDK. No `ExtensionTrust` variant. No
secret provider. No write/glob/shell filesystem battery. No claim
that WASM alone is complete application-level safety. No G5/G6
decision. Do not restore `tools/architecture/`. Do not implement
`@0.0.5` or `@1.0.0` worlds. Do not add `wasi:http` linking
(still no `wasmtime-wasi-http`).

## Validation

- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — no `wasmtime`, no `wasmtime-wasi`, no `finstack-ai-plugin-host`,
  no `finstack-ai-guest-sdk`
- `cargo tree -p finstack-ai --locked --no-default-features` — same
- `cargo tree -p finstack-ai-native-examples -p finstack-ai-wit --locked`
  — no `wasmtime`, no `wasmtime-wasi`, no guest-sdk, no plugin-host
- `cargo tree -p finstack-ai-guest-sdk --locked`
  — no `wasmtime`, no `wasmtime-wasi`, no `finstack-ai-plugin-host`,
  no `finstack-ai-wit`, no `finstack-ai`, no kernel, no runtime,
  no tokio, no `libloading`
- `cargo tree -p finstack-ai-plugin-host --locked`
  — has `wasmtime` and `wasmtime-wasi`; no `libloading`; no
  `finstack-ai-guest-sdk`
- `cargo test -p finstack-ai-guest-sdk --offline --locked`
- `cargo test -p finstack-ai-plugin-host --offline --locked`
- `cargo test -p finstack-ai-wit --offline --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check-wit`
- `mise run check-guest-sdk`
- `mise run check-plugin-wasm`
- `mise run check-plugin-template`
- `mise run check-wasm`
- `uv run --no-project python tools/wasm_package/check.py graph`

Do not require `mise run ci` or Playwright to close the candidate.
Workspace clippy remains the gate.

## Suggested authorization sentence

When ready to admit and implement:

```
Run PR-053; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```
