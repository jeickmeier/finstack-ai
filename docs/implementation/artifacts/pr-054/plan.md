# PR-054 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-054-plugin-discovery-alpha`
Intended baseline: local `main` at `5613c28408af692738cbf494d1e8ecd2c1cd947a`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-054. The closed PR-053 envelope
is not reused. PR-054 is the only active logical PR.

## Execution envelope

Authorized 2026-08-14 by `implement the plan` using the same local-only
integrated envelope as PR-049–PR-053. Suggested text that was accepted:

```
Run PR-054; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io publish,
tag, G5 inference, G6 inference. Do not start PR-055+. Do not write a
`G6-D-*` decision in the implementation or evidence commit. Named
`G6-D-*` is a separate owner action.

## Admission (when authorized)

- Dependency PR-053 is `Done` at local `main` merge
  `3297eb44fb127ff6b36af1a705cda38442efdf95`. Closeout commit
  `5613c28408af692738cbf494d1e8ecd2c1cd947a` is the intended baseline.
  The post-merge wasm refresh `a7d86b256f2bd805fc8ff1dc4dbf9acc8f82d77c`
  is already on `main`.
- Phase 7 entrance is already `Passed` (3/3):
  `PH7-E-entrance-ports-d16eb3f550bd`,
  `PH7-E-entrance-record-context-81f85ef9d2cf`, and
  `PH7-E-entrance-adr-035-3c4a20fedca4`. Do not re-record.
- Phase 7 exit is `0/4`. This PR writes the exit review. Do not mark
  the phase `Done` in the implementation commit; closeout may record
  exit `Passed` (4/4) and Phase 7 `Done` only after the review exists
  at the integrated commit. That is not G6.
- G5 is `Not ready`. That does not block Phase 7. Do not infer G5 or G6.
- ADR-035 / ADR-010 / ADR-011 stay `In progress` / `Partial` until a
  named `G6-D-*` exists. Never mark them Implemented in this PR's
  implementation or evidence commit. ADR-014 stays Missing: WIT worlds
  remain neither remote nor process DTOs.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  keeps Wasmtime/`wasmtime-wasi` in the `plugins/` host leaf, does not
  add a native dylib loader, does not change WIT compatibility policy
  or published world versions, and does not add a seventh port. A new
  plugin-lock schema family is a new contract, not a WIT policy change.
- Threat Model section 18 is triggered (plugin signing / discovery /
  substitution). Primary **TM-08** (lockfile resolution, bytes↔digest
  binding, duplicate identity, downgrade). Also **TM-07** (published
  hostile suite completion) and **TM-06** labels. Do not change host
  grant math, signature policy, or resource-limit defaults. Complete
  the review before merge.

## Traceability

Implementation Plan PR-054; FR-PLG completion; Architecture §15.2–15.5
and ADR-010/011; TDD §27.1 (lockstep `@0.0.4` / workspace release),
§27.6–27.7, §31.5, §32.1 plugin conformance, §33 benchmarks.
TM-06 / TM-07 / TM-08. SEC-INV-006 / 007 / 011 / 012.
G6 rows: Implementation Plan §7, Engineering Standards §15,
Threat Model §13.2.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

Implementation Plan A04 says the `0.0.4` checkpoint and G6 have
passed. Engineering rules: gates are never inferred; PR authorization
is not a gate decision. Lock the PR-048 reading of that sentence:

- A04 is the Phase 7 exit review plus G6 readiness pack plus staged
  unpublished lockstep `0.0.4` artifacts.
- Named `G6-D-*`, checkpoint cut, publish, and tag are **not** this
  PR unless the owner separately authorizes them.

- PR-054-A01: Runtime startup consumes a resolved lockfile and
  performs no registry search/download. `PluginHost::load_enabled(path)`
  reads one local JSON lockfile, loads only `enabled: true` entries
  from paths resolved relative to the lockfile directory, and returns
  `ReadyWasm` values. It does not walk a plugin directory, does not
  fetch, and does not consult crates.io / npm / pypi / git. A path
  containing `://`, a scheme, or `..` is `plugin_lock_invalid`.
  `cargo tree -p finstack-ai-plugin-host` still has no `libloading`
  and no HTTP client added for discovery (`reqwest`, `ureq`, `hyper`
  as a direct dep). Default SDK / kernel / runtime / wit /
  native-examples trees still have no `finstack-ai-plugin-host`.
- PR-054-A02: Ambiguous duplicate packages fail closed. Two lockfile
  entries with the same `identity` fail at resolve as
  `plugin_lock_duplicate`, including when one is `enabled: false`.
  `load_locked` of a disabled entry is `plugin_lock_disabled`.
  On-disk component bytes whose SHA-256 is not the lock
  `component_digest` fail as `plugin_lock_digest_mismatch`
  (substitution / downgrade). Parsed `manifest.digest` that is not
  the lock `manifest_digest` fails the same code.
- PR-054-A03: Reference plugins pass on supported desktop targets.
  The local Darwin arm64 envelope used by PR-049–PR-053 is the
  supported desktop proof. `load_enabled` on
  `plugins/reference/plugin.lock.json` loads calculator and
  context-provider; those adapters still pass
  `check_toolset_conformance` / `check_context_conformance`.
  Filesystem-sandbox is present in the lock with `enabled: false`
  (requires an explicit host filesystem grant). A separate test
  lock or programmatic `LockedPlugin { enabled: true, .. }` plus
  preopen still proves the sandbox. Do not claim Linux/Windows CI.
- PR-054-A04: Phase 7 exit review (all four exit bullets), G6
  readiness pack, and staged unpublished lockstep `0.0.4` artifacts
  exist under `docs/implementation/artifacts/pr-054/`. Result
  language is `READY FOR NAMED DECISION`. Do not write `G6-D-*`.

Principal changes that are not extra acceptance IDs, but are required
to prove the four bullets:

- Host-side lockfile parse/resolve and `load_locked` / `load_enabled`.
- Installation tool `tools/plugin_lock/lock.py` that only hashes local
  files and writes/checks the lockfile. Runtime loading does not call
  that tool.
- Published hostile conformance module that names the G6 rows and
  fills the remaining ABI-mismatch, cancellation, large-payload, and
  lockfile-substitution cases.
- Plugin-host Criterion benches (warning-only; not PR-063).
- Plugin-lock schema family and compatibility fixtures.
- Workspace / Python / JS / guest crate version `0.0.3` → `0.0.4`.
  WIT packages stay `@0.0.4`. `@1.0.0` generation stays blocked.

## Locked design

### Crate and graph

Workspace version becomes `0.0.4` in the implementation candidate so
staged artifacts match the checkpoint name. That is not a cut and not
a publish. `publish = false` stays. Do not add `finstack-ai-plugin-host`
or `finstack-ai-guest-sdk` to `mise run check-wasm`. Do not add plugin
or lockfile types to the public-rust-api corpus (stays 131). Do not add
`ExtensionTrust::IsolatedWasm`. Isolation stays a property of
`PluginHost`.

```text
tools/plugin_lock/lock.py          # NEW installer; local files only
  -> hashlib / json / pathlib
  -> must not import plugin-host
  -> must not fetch

plugins/reference/plugin.lock.json # NEW checked-in resolved lock

finstack-ai-plugin-host            # lockfile.rs + load_enabled
  -> existing wasmtime / wasmtime-wasi / ed25519-dalek / wit / sha2
  -> serde + serde_json for lockfile parse
  -> must NOT depend on finstack-ai-guest-sdk
  -> must NOT add reqwest / ureq / hyper / libloading

finstack-ai-guest-sdk              # version bump only
plugins/reference/*                # version bump; lockfile pin
plugins/templates/*                # version bump
```

Forbidden edges stay as PR-053:

- kernel / runtime / SDK / protocol / bindings / rust-minimal /
  `finstack-ai-wit` / native-examples → `finstack-ai-guest-sdk`,
  `wasmtime`, `wasmtime-wasi`, or `finstack-ai-plugin-host`
- guest-sdk / templates / reference components → host-private crates
- any default SDK feature that pulls the host or the guest SDK
- any `libloading` / native dylib loader
- any lockfile or installer HTTP client

`ResolvedAgentLock` (Architecture §16 / agent composition) is a
different document. Do not reuse that type or schema.

### Lockfile document

JSON, `deny_unknown_fields`, `lockfile_version: 1`.

```json
{
  "lockfile_version": 1,
  "plugins": [
    {
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "enabled": true,
      "component": "calculator/component.wasm",
      "component_digest": "<hex sha256 of component bytes>",
      "manifest": "calculator/plugin.manifest.json",
      "manifest_digest": "<PluginManifest.digest>"
    },
    {
      "identity": "finstack.plugin.reference.context",
      "version": "0.0.4",
      "enabled": true,
      "component": "context-provider/component.wasm",
      "component_digest": "<hex sha256>",
      "manifest": "context-provider/plugin.manifest.json",
      "manifest_digest": "<PluginManifest.digest>"
    },
    {
      "identity": "finstack.plugin.filesystem.sandbox",
      "version": "0.0.4",
      "enabled": false,
      "component": "filesystem-sandbox/component.wasm",
      "component_digest": "<hex sha256>",
      "manifest": "filesystem-sandbox/plugin.manifest.json",
      "manifest_digest": "<PluginManifest.digest>"
    }
  ]
}
```

Use the real published identities already on those manifests. Do not
invent a second identity for calculator.

Field rules:

- `lockfile_version` must be `1`. Other values are `plugin_lock_invalid`.
- `identity` must start with `finstack.plugin.` and match the parsed
  manifest.
- `version` must be `0.0.4` and match the parsed manifest. This is
  interface-version negotiation for plugin alpha: the lock pins the
  experimental package version; `@1.0.0` remains blocked at parse.
- `enabled` is required. Missing/unknown fields fail closed.
- `component` and `manifest` are relative to the lockfile's parent
  directory. Reject empty, NUL, `..`, absolute paths, and any string
  containing `://` or a URL scheme.
- `component_digest` is `component_digest(bytes)` from
  `plugins/finstack-ai-plugin-host/src/cache.rs` (hex SHA-256 of the
  component file). Do not invent a second hash.
- `manifest_digest` is the existing host-computed
  `PluginManifest.digest` (JCS of `{identity, version, worlds}`), not
  a hash of the manifest file bytes. `validate_manifest` still runs.

Resolve algorithm (`resolve_lockfile(path) -> ResolvedPluginLock`):

1. Read the file. Missing file → `plugin_lock_not_found`.
2. Parse with `deny_unknown_fields`. Oversize the JSON against
   `MAX_RAW_JSON_BYTES` before parse.
3. Reject duplicate `identity` values in the array
   (`plugin_lock_duplicate`), including disabled duplicates.
4. Resolve and sanitize each path. Do not read component bytes yet
   except when the caller asks to load.
5. Return the full set. `enabled()` filters; it does not search.

`PluginHost::load_locked(&self, entry: &LockedPlugin)`:

1. Reject `enabled == false` as `plugin_lock_disabled`.
2. Read component bytes. Verify `component_digest(bytes)`.
3. Read and `parse_manifest` / `validate_manifest`.
4. Require `manifest.digest == entry.manifest_digest`,
   `manifest.identity == entry.identity`,
   `manifest.version == entry.version`.
5. Infer `PluginWorld` from the intersection of declared worlds
   (`toolset-plugin` / `context-plugin`). A lock entry whose worlds
   do not match the component fails as `plugin_lock_invalid` /
   existing world-mismatch `plugin_registration_invalid`.
6. Call existing `load(bytes, manifest, world)`.

`PluginHost::load_enabled(&self, lockfile: &Path) -> Vec<ReadyWasm>`
is the A01 startup surface: `resolve_lockfile` then `load_locked`
for each enabled entry, in lockfile order. Extra `component.wasm`
files sitting next to the lock are ignored.

Do not add directory-glob discovery. Do not add a registry name
field. Do not add automatic updates.

### Installation tooling

Create `tools/plugin_lock/lock.py` and mise tasks:

```text
mise run gen-plugin-lock
mise run check-plugin-lock
```

`generate` walks a caller-supplied list of local component
directories (default: the three `plugins/reference/*` dirs), reads
each `component.wasm` and `plugin.manifest.json`, and writes
`plugins/reference/plugin.lock.json`. Sandbox `enabled` defaults to
`false`; calculator and context-provider default to `true`.

`check` regenerates into a temp file and requires byte-identical
lockfile contents (stable JSON key order: serde/`json.dumps` with
sorted keys, 2-space indent, trailing newline).

The tool uses only the standard library. It must not import
`finstack-ai-plugin-host`, must not run `cargo`, and must not open a
network socket. Host tests prove runtime loading; the tool only
writes/checks the pin file.

### Conformance suite

Add `plugins/finstack-ai-plugin-host/src/conformance_tests.rs` as the
published G6 hostile/conformance index. Prefer calling existing
helpers over copying fixtures. Every row below must have a named
test. Existing PR-050–PR-053 tests that already prove a row may be
moved or re-exported; do not leave a G6 row as "see some other file"
without a `conformance_tests::` name.

| Row | Proof | Expected code |
| --- | --- | --- |
| Lifecycle initialize/health/shutdown | existing wit/host lifecycle | `plugin_lifecycle_timeout` on fired deadline |
| Malformed manifest | existing `parse_manifest` deny-unknown / omitted digest | `plugin_registration_invalid` / `plugin_manifest_digest_mismatch` |
| Permission denial | filesystem name without host grant | `plugin_permission_denied` |
| Ungranted WASI instantiate | existing ungranted filesystem/sockets/cli | `plugin_instantiate_failed` |
| Trap | trap-toolset | `plugin_trap` |
| Resource limit | fuel-burner | `plugin_resource_limit` |
| Signature unsigned/untrusted | existing Strict fixtures | `plugin_signature_untrusted` |
| Cancellation | fire `CancellationSignal` during an echo/calculator call | `plugin_lifecycle_timeout` |
| Large payload | args-json / collect item over `MAX_RAW_JSON_BYTES` through the wasm adapter | existing size-reject code, before allocation |
| ABI mismatch | calculator wasm loaded as `PluginWorld::Context` | world-mismatch / instantiate fail |
| Lockfile duplicate | two calculator identities | `plugin_lock_duplicate` |
| Lockfile substitution | flip one byte of wasm or swap digest | `plugin_lock_digest_mismatch` |
| Lockfile registry path | `component: "https://example.invalid/p.wasm"` | `plugin_lock_invalid` |
| Disabled not loaded | default reference lock | sandbox absent from `load_enabled` |
| Cache ABI | existing cache-key ABI field still distinguishes worlds | miss on ABI change |

Do not add new WASI grants, `wasi:http` linking, or a secret
provider to write these rows.

### Benchmarks

Create `plugins/finstack-ai-plugin-host/benches/plugin_host.rs` with
Criterion, matching
`extensions/toolsets/finstack-ai-tools-calculator/benches/calculator.rs`:

- compile-cache miss then hit for echo-toolset
- echo tool call
- calculator add `[1,2,3]`

Warning-only. No named budget. Not PR-063. Add `criterion` as a
plugin-host dev-dependency from workspace. Do not put benches on
`check-wasm`. Record the command and a short note in
`artifacts/pr-054/`; do not treat Criterion noise as a merge gate.

### Compatibility fixtures

Add an experimental plugin-lock family:

| Path | Owns |
| --- | --- |
| `schemas/plugin-lock/v1/plugin-lock.schema.json` | JSON Schema 2020-12, reject-unknown |
| `fixtures/compatibility/plugin-lock/v1/` | valid lock, duplicate, URL path, bad version, missing enabled |
| `schemas/schema-families.toml` | new `plugin-lock` family, `experimental-0.x` |
| `docs/implementation/compatibility-governance.md` | one row |

Valid fixtures may use placeholder hex digests; host tests use real
bytes. `mise run schema-governance` must stay clean.

Do not change the WIT family or `@0.0.4` world bytes.

### 0.0.4 staging and G6 readiness (A04)

Follow PR-048 / PR-038, not a silent gate.

Version bump in the implementation candidate (lockstep metadata only):

- `Cargo.toml` `[workspace.package].version` and
  `[workspace.dependencies]` `0.0.3` → `0.0.4`
- `bindings/finstack-ai-python/pyproject.toml`
- `bindings/finstack-ai-wasm/js/package.json` (and lock)
- non-member guest/template/reference/`tools/plugin_wasm/encoder`
  `Cargo.toml` versions
- `examples/python-minimal/python-callback/pyproject.toml` pin
- `fixtures/compatibility/wit/v0.0.4/packages/valid--published-packages.json`
  `crate_version`
- `plugins/finstack-ai-wit/COMPATIBILITY.md`
- `CHANGELOG.md` Unreleased: stage unpublished `0.0.4`; named G6,
  checkpoint cut, publish, and tag remain owner decisions

Do not rebuild the Python wheel matrix. Do not `npm publish` or
`cargo publish`. A staged unpublished npm tarball plus checksums /
SBOM under `docs/implementation/artifacts/pr-054/staging/` is enough
to match PR-048's "available and not cut" language. If packing the
JS tarball is expensive, record `js_half` / `python_half` the same
way PR-048 did and still bump the version metadata.

Write:

- `phase7-exit-review.txt` covering the four Phase 7 exit bullets
- `g6-readiness-review.txt` covering Implementation Plan §7, Engineering
  Standards §15 G6, and Threat Model §13.2 G6
- Result language: `READY FOR NAMED DECISION`

Phase 7 exit bullets this PR must evidence, without writing `G6-D-*`:

1. Reference components load and pass conformance (A03 + suite).
2. Permissions and resource limits are enforced by the host
   (PR-052 evidence reused plus conformance rows).
3. Wasmtime is absent from minimal/native builds unless selected
   (existing graph tests).
4. Plugin packaging and compatibility diagnostics are documented
   (lockfile, COMPATIBILITY.md, host README, schema family).

### Docs

Update, do not invent a second authoring guide:

- `plugins/finstack-ai-plugin-host/README.md` — lockfile is now read;
  document `load_enabled`, `gen-plugin-lock` / `check-plugin-lock`,
  and the conformance filter
- `plugins/README.md` — lockfile + mise tasks
- `plugins/finstack-ai-wit/COMPATIBILITY.md` — crate version `0.0.4`;
  packages stay `@0.0.4`
- `plugins/finstack-ai-guest-sdk/README.md` / `MIGRATION.md` — one
  sentence that runtime discovery is lockfile-only

### Host defaults (do not change)

Default grants stay `{logging, blobs}`. Default `StoreLimits` /
`DEFAULT_INSTANCES` stay. Signature default stays Permissive.
Manifest worlds stay the short names `toolset-plugin` /
`context-plugin`. Do not add `filesystem-sandbox` to `ALLOWED_WORLDS`.

## Tasks (when admitted)

Task IDs are minted at admit, not now.

1. Tracking: confirm PR-053 Done, record exclusions, open
   `codex/pr-054-plugin-discovery-alpha` from the named baseline.
   Do not re-record Phase 7 entrance.
2. Lockfile types, path sanitizer, `resolve_lockfile`,
   `load_locked`, `load_enabled`, and no-registry graph proofs (A01).
3. Duplicate / disabled / digest-mismatch / URL-path fail-closed
   (A02).
4. Conformance suite module: map existing rows, add cancellation,
   large-payload, ABI-mismatch, and lockfile-substitution (G6
   hostile completion).
5. `tools/plugin_lock`, mise tasks, checked-in
   `plugins/reference/plugin.lock.json`, desktop `load_enabled`
   calculator/context proof (A03).
6. Plugin-host benches, plugin-lock schema family, authoring /
   packaging docs.
7. `0.0.4` metadata staging, Phase 7 exit review, G6 readiness
   pack, candidate evidence. Stop before `G6-D-*`.

## Explicit exclusions

No public marketplace, automatic updates, or registry download.
No native dynamic-library ABI. No `ExtensionTrust::IsolatedWasm`.
No seventh port. No kernel/runtime/SDK dependency on the host or
guest SDK. No default SDK feature that pulls Wasmtime. No secret
provider. No write/glob/shell filesystem battery. No `wasi:http`
linking. No `@0.0.5` or `@1.0.0` worlds. No claim that WASM alone
is complete application-level safety. No G5 decision. No G6
decision unless separately authorized. No crates.io / npm / pypi
publish. No git tag. Do not start PR-055+. Do not restore
`tools/architecture/`. Do not change host grant math or default
limits. Do not reuse `ResolvedAgentLock` as the plugin lockfile.

## Validation

- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — no `wasmtime`, no `wasmtime-wasi`, no `finstack-ai-plugin-host`,
  no `finstack-ai-guest-sdk`
- `cargo tree -p finstack-ai --locked --no-default-features` — same
- `cargo tree -p finstack-ai-native-examples -p finstack-ai-wit --locked`
  — no wasmtime, no guest-sdk, no plugin-host
- `cargo tree -p finstack-ai-guest-sdk --locked`
  — no wasmtime, no plugin-host, no `finstack-ai-wit`, no kernel,
  no runtime, no tokio, no `libloading`
- `cargo tree -p finstack-ai-plugin-host --locked`
  — has wasmtime 47.0.3 and wasmtime-wasi 47.0.3; no `libloading`;
  no `finstack-ai-guest-sdk`; no new HTTP client for discovery
- `cargo test -p finstack-ai-guest-sdk --offline --locked`
- `cargo test -p finstack-ai-plugin-host --offline --locked`
- `cargo test -p finstack-ai-wit --offline --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `mise run check-wit`
- `mise run check-guest-sdk`
- `mise run check-plugin-wasm`
- `mise run check-plugin-template`
- `mise run check-plugin-lock` (new)
- `mise run schema-governance`
- `mise run check-wasm`
- `uv run --no-project python tools/wasm_package/check.py graph`

Do not require `mise run ci`, Playwright, or a multi-OS hosted
matrix to close the candidate. Workspace clippy remains the gate.
Criterion is warning evidence only.

If rustfmt or guest source changes, regenerate wasm with
`mise run gen-plugin-wasm` and the lockfile with
`mise run gen-plugin-lock` before the check tasks. rustc 1.97.1
can produce a small `component.wasm` size delta on a clean rebuild;
do not treat that as a product failure, but do not hand-edit wasm.

## Suggested authorization sentence

When ready to admit and implement:

```
Run PR-054; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G6-D-*`, cut the
`0.0.4` checkpoint, publish, or tag. Do not infer those from
`implement the plan`.
