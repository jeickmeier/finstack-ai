# PR-064 independent security review

| Field | Value |
| --- | --- |
| Reviewer | **Claude Opus 5**, independent-review subagent, session distinct from the PR-064 implementation session |
| Review date | 2026-08-15 |
| Workspace | `/Users/jeickmeier/Projects/finstack-ai` |
| Plan baseline | documentation pack v0.21 / PLAN-0.19 |
| Review unit | PR-064 (`PR-064-T-review-b6e4d09a2c18`), Threat Model §13.3 |
| Findings register | [findings.md](findings.md) |
| Envelope | local-only; `external actions=none` |

## 1. Method and independence

This is a **first-party independent review**: a written report by a named
reviewer who is not the person or session that implemented the reviewed
surfaces. It is **not** a commissioned external audit and no external audit
firm was engaged; commissioning one remains a separately named external
action. A self-review by the implementer would not satisfy Threat Model
§13.3 and is not what this file records.

Method: manual reading of first-party source, tests, fixtures, task
definitions, CI workflow, and public documentation at the current working
tree. No exploit proof-of-concept was written. No credential, token, or
secret value appears in this report or in [findings.md](findings.md). No
GHSA or CVE identifier is asserted; none exists for this project.

Reviewed material:

- `docs/planning/06-finstack-ai-security-threat-model.md` §§2–18
  (read-only; not edited);
- `docs/implementation/threat-model-g7-review.md`;
- `SECURITY.md` severity rubric and supported versions;
- `docs/site/security-trust-levels.md`, `docs/site/security-deployment.md`,
  `docs/site/provider-security.md`, `docs/site/wasm.md`,
  `docs/site/plugin.md`;
- `crates/finstack-ai-protocol`, `crates/finstack-ai-server`,
  `crates/finstack-ai-runtime`, `crates/finstack-ai-test`;
- `extensions/toolsets/finstack-ai-tools-filesystem`,
  `extensions/toolsets/finstack-ai-tools-shell`;
- `plugins/finstack-ai-plugin-host`, `plugins/finstack-ai-wit`;
- `bindings/finstack-ai-python`, `bindings/finstack-ai-wasm`;
- `mise.toml`, `.github/workflows/ci.yml`, `fuzz/`.

The reviewer had read access only and made no code change. Remediation of
every `Open` finding belongs to the PR-064 implementer.

## 2. Scope checklist — Threat Model §13.3

Every §13.3 bullet is covered. `Evidenced` means an implemented control
plus a first-party test or checked-in artifact. `Finding` links a
`FIND-064-*` row.

| §13.3 bullet | Disposition | Primary evidence | Findings |
| --- | --- | --- | --- |
| Journal / recovery integrity and external completion | Evidenced, with accepted residuals | §3.1 | FIND-064-011, FIND-064-014, FIND-064-018, FIND-064-009 |
| Remote framing, authentication hooks, authorization context | Evidenced, with findings | §3.2 | FIND-064-003, FIND-064-005, FIND-064-012, FIND-064-013 |
| Interaction authorization and privileged tool dispatch | Evidenced | §3.3 | FIND-064-014 |
| Filesystem / shell and artifact boundaries | Evidenced, with findings | §3.4 | FIND-064-002, FIND-064-007, FIND-064-016 |
| WIT / Wasmtime permissions, limits, signatures, cache identity | Evidenced; FIND-064-001 Closed by implementer | §3.5 | FIND-064-001, FIND-064-008, FIND-064-015 |
| Python / JavaScript callback lifecycle and browser credential guidance | Evidenced | §3.6 | FIND-064-020 |
| Secret / redaction behaviour | Evidenced | §3.7 | FIND-064-017 |
| Dependency / build / release provenance | **Finding (Medium)** | §3.8 | FIND-064-004, FIND-064-006, FIND-064-010 |

Implementation Plan focus areas (plugins, filesystem/shell tools,
protocols, secret handling) are covered by §§3.2, 3.4, 3.5, and 3.7. They
are a subset of the §13.3 list, not a reduction of it.

## 3. Surface reviews

### 3.1 Journal / recovery integrity and external completion

The record envelope carries a domain-separated payload digest plus an
envelope checksum over an explicit replay-field projection that excludes
the diagnostic commit timestamp, and each envelope cites the previous
checksum
(`crates/finstack-ai-protocol/src/journal.rs`). Chain verification
recomputes both values and rejects sequence gaps, so truncation,
reordering, and single-record edits fail closed rather than producing
guessed state (SEC-INV-010). The checksum chain is *tamper evidence*, not
authentication; that limit is an accepted §16 residual, recorded as
FIND-064-009.

External completion routing
(`crates/finstack-ai-runtime/src/ingress.rs`) performs, in order: locator
digest and submitted-command digest computation, idempotency-horizon
expiry, session recovery, exact session/lane/run/tenant identity match,
authorization-evidence match (principal plus policy version plus decision
identity), target-effect existence, then commit. Every rejection path is
non-existence-revealing (`unknown_locator` regardless of which check
failed) and is audited through `SecurityAuditGate` with digests rather than
payloads. Duplicate submissions are idempotent; conflicting settlements and
late completions are durably rejected.

Crash and recovery coverage is a named prefix catalog in
`crates/finstack-ai-test/tests/crash_prefix.rs`, including an inventory
test (`crash_prefix_catalog_covers_every_effect_kind_and_lane_operation`)
that asserts every `EffectKind` and every public lane operation has at
least one prefix restoring to a `LegalRestore` class.

External effects remain at-least-once or explicitly uncertain (ADR-013).
No exactly-once claim is made anywhere in this review.

### 3.2 Remote framing, authentication hooks, authorization context

Framing (`crates/finstack-ai-protocol/src/frame.rs`) reads a 4-byte
big-endian length and rejects an oversize declaration **before** any
payload buffer is allocated; `crates/finstack-ai-server/src/io.rs` honours
that ordering. Separate pre-auth (16 KiB) and post-auth (256 KiB) ceilings
apply. The envelope is family-tagged with `deny_unknown_fields`, and a
`Process` body decoded as `Remote` fails with `unknown_payload_family`
before session acquisition. Version selection has no fallback below either
floor, and mandatory features are required at hello.

The connection state machine (`crates/finstack-ai-server/src/connection.rs`)
authenticates before any session payload, enforces a handshake deadline and
an authenticate-attempt cap, refuses bearer credentials on plaintext TCP,
audits each failure, and never writes a token into an audit event. After
authentication it re-checks the client-declared `tenant_scope` against the
authenticated context, then re-checks scope again inside the replica for
both reconnect and every command. Commands carry a domain-separated digest
over the normalized command, and receipt reuse with a different digest
fails closed as an idempotency conflict.

Two issues. `StaticAuthVerifier` compares the bearer token with ordinary
string inequality, which is not constant time (FIND-064-005). The
reference replica retains one settlement receipt per `command_id`
indefinitely and scans that map linearly on every command, so an
authenticated principal can drive unbounded memory and rising per-command
cost (FIND-064-003). The second is filed `Open`; it is a bounds gap and is
deliberately not accepted, because an unbounded resource is not waivable.

### 3.3 Interaction authorization and privileged tool dispatch

`InteractionResolutionRouter` mirrors the completion router: known-target
check, tenant and locator identity, authorization-evidence match, expiry,
audited non-existence-revealing rejection, and durable rejection records
for conflicts. `crates/finstack-ai-test/tests/interaction.rs` exercises
the privileged path end to end — approval request parks without dispatch,
denial never dispatches, expired resolution never dispatches, expiry on
restore never dispatches, duplicate resolution is idempotent, conflicting
resolution fails closed and is audited, run-level cancel while awaiting
closes without dispatch, and a late privileged resolution after cancel
fails closed. Possession of an interaction identifier alone is not
sufficient to resolve it.

Native toolsets re-verify authority at call time: both the filesystem and
shell toolsets reject a call whose principal declares a tenant scope other
than the committed effect's scope. A principal without a declared scope is
accepted, which is correct — the run's journal-authoritative tenant scope
governs, and scope is bound upstream at handle acquisition.

### 3.4 Filesystem / shell and artifact boundaries

The filesystem toolset is capability-style throughout. A root directory
descriptor is opened once with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` and
retained; every subsequent component is reached with `openat` from that
descriptor using the same no-follow flags, so no operation ever
re-resolves an absolute path. Path parsing rejects absolute paths, `..`,
`.`, empty and duplicate separators, backslashes, NUL bytes, and
over-length paths or components. Protected patterns (`.git`, `.env`,
`.ssh` by default) are enforced both at validation and while walking.
Reads, walks, and searches are bounded by byte, entry, depth, and
match ceilings, and cancellation is checked between steps.
`extensions/toolsets/finstack-ai-tools-filesystem/src/tests.rs` includes
the TM-03 race cases: `symlink_swap_between_authorization_and_open_never_reads_outside`,
`rename_after_open_reads_the_authorized_object_not_replacement`,
`symlink_root_is_rejected_at_construction`, and
`traversal_symlink_escape_and_protected_paths_fail_closed`.

The shell toolset is deny-by-default: a non-empty allowlist is required,
basenames need either an exact allowlisted path or an explicitly opted-in
absolute search path, the environment is cleared and repopulated only from
an explicit map, stdin is null, argv count and argument size are bounded,
and the tool spec declares `NonIdempotentWrite` with `AtMostOnce` retry
safety and `ApprovalRequirement::Policy`. Oversized results are staged to
the artifact store rather than inlined.

The shell working-directory path is the exception. `Root::authorize_cwd`
validates the relative path correctly, component by component, with
`openat` and `O_NOFOLLOW` from the retained root descriptor — and then
drops that descriptor and returns a joined path *string*.
`apply_authorized_cwd` re-opens that string, where `O_NOFOLLOW` protects
only the final component. Intermediate components are re-resolved with
symlink following after the check, so a multi-segment `cwd` can be
redirected outside the authorized root by an attacker who can replace an
intermediate directory with a symlink between validation and exec. The
shell crate has no symlink or cwd test at all, unlike the filesystem
crate. This is FIND-064-002. Separately, the combined-output ceiling is
applied only after the child exits and both pipes are drained, so a
flooding child manifests as `shell_timeout` rather than
`shell_limit_exceeded` (FIND-064-007).

Artifact references (`crates/finstack-ai-runtime/src/artifact.rs`) are
bound to a domain-separated scope digest over tenant scope, session, run,
and sensitivity, plus a content digest and length. `stage_required_artifact`
re-validates the reference the store returns and rejects any adapter that
rewrites scope, content, or metadata, so a store cannot silently widen
access.

### 3.5 WIT / Wasmtime permissions, limits, signatures, cache identity

Permissions are deny-by-default and layered correctly. The manifest's
requested permissions are intersected with the host-offered grant set
(`require_offered`), `secrets` cannot be offered at all, and a granted
*name* still does not link an interface unless a concrete resource backs
it — filesystem needs a preopen, HTTP needs a hostname allowlist, sockets
need an explicit flag (`can_link`). `link_granted_wasi` never
blanket-links filesystem, sockets, HTTP, or CLI, and an unresolved
`wasi:*` import fails instantiation closed. Fuel, epoch interruption, and
a store limiter bound memory, tables, and instances; traps, fuel
exhaustion, and memory-grow refusal map to stable codes and the host
continues. These are covered by tests in
`plugins/finstack-ai-plugin-host/src/instantiate.rs` and
`src/grants.rs`, and by the hostile guest fixtures.

Package identity has three layers: the manifest self-digest, an optional
ed25519 detached signature verified against configured trust roots under
`Permissive` or `Strict` policy, and a local lockfile that pins the
SHA-256 of the component bytes and rejects absolute paths, `..`, URL
schemes, and duplicate identities. `load_locked` re-hashes the component
file and the manifest before loading.

Note for the record, without proposing a planning change: the manifest
digest and the signing payload both cover only `{identity, version,
worlds}`. They do not cover `permissions`, `configuration_schema`, or the
component bytes. Byte binding therefore comes exclusively from the
lockfile's `component_digest`, and permission binding comes exclusively
from the host-offered grant set. Because `require_offered` fails closed on
anything the host has not offered, editing `permissions` in a signed
manifest cannot escalate authority — it can only cause a denial. The
practical consequence is narrower than it first appears, but operators
should not read a verified signature as an attestation of the component
binary.

The material problem is the compiled-component cache. `PluginHost::load`
computes a cache key over `{abi, digest, engine, target}` and, on a hit,
passes the stored bytes straight to `unsafe Component::deserialize`. In
directory mode those bytes are read from the filesystem with no integrity
check; the filename is the only binding, the directory is created with
default permissions, and the artifact is never re-verified against the
component digest that produced it. The manifest digest, the ed25519
signature, and the lockfile digest all run against the *source* bytes and
are entirely bypassed on a cache hit. Wasmtime documents `deserialize` as
unsafe for untrusted input. This is FIND-064-001. The default in-memory
cache (`cache_dir: None`) is unaffected, and no in-tree caller enables
directory mode outside tests. The implementer closed FIND-064-001 by
never deserializing directory-cache bytes; see findings.md §1
Remediation (landed).

### 3.6 Python / JavaScript callback lifecycle and browser credential guidance

Python callbacks (`bindings/finstack-ai-python/src/callbacks.rs`) have an
explicit per-callback timeout validated into `(0, 86400]` seconds, a
cancellation signal exposed to the callback, and a bounded settle window
so a callback that ignores cancellation cannot delay the Rust run
indefinitely. Blocking user code runs through `run_blocking` rather than on
the async executor, the interpreter is attached only inside narrow scopes,
and exceptions normalize to stable codes (`python_callback_cancelled`,
`python_callback_timeout`). `bindings/finstack-ai-python/tests/`
exercises callbacks, handles, interactions, and durable restart.

Browser guidance is present and correct. `docs/site/wasm.md` states that
provider keys must not be embedded in the bundle and must terminate at a
trusted same-origin proxy, labels host callbacks T2 with page authority
and no isolation, and marks the IndexedDB adapter experimental and not
crash-durable. `mise run check-wasm` runs `tools/wasm_package/check.py
secrets` over the generated bundle. `docs/site/provider-security.md`
rejects credentials embedded in configuration URLs.

### 3.7 Secret / redaction behaviour

Secrets are represented by references, not values, across configuration,
records, and audit events. The remote `AuthContext` deliberately holds no
raw token, and audit events carry digests of locators and submissions
rather than payloads. Journal diagnostic export
(`crates/finstack-ai-runtime/src/observer_export.rs`) has three payload
modes and a canary test proving that `MetadataOnly` and `Redacted` omit
the record body and do not emit the canary value. Redaction happens before
data reaches an exporter queue. Tool and shell errors use fixed
`&'static str` messages with stable codes, so a credential cannot reach an
error string by interpolation. I found no path that writes a secret into a
record, event, audit row, bundle, or diagnostic export.

### 3.8 Dependency / build / release provenance

Positive controls: every GitHub Action is pinned to a commit SHA;
`mise.toml` pins the Rust toolchain, Python, uv, Node, `wasm-bindgen-cli`,
and `cargo-fuzz`; CI runs with `persist-credentials: false` and
`contents: read`; every cargo invocation uses `--locked` against a
checked-in `Cargo.lock`; `mise run release-rehearsal` performs a two-run
local staging checksum comparison; and `mise run docs-license` sweeps the
project's own package license metadata.

Gap: there is no continuous dependency advisory, license, or source-policy
check. Neither `mise.toml` nor `.github/workflows/ci.yml` runs
`cargo-audit`, `cargo-deny`, or an OSV scan, so Threat Model §13.1's
"dependency advisory/license/source policy" continuous check and TM-18's
"dependency sources, advisories, licenses, and exceptions are checked
continuously" are not satisfied by tooling today (FIND-064-004).
Separately, PR-064 restored `mise run fuzz-smoke` with all seven targets,
but neither `mise run ci` nor the CI workflow invokes it, so §13.1's
parser and deserializer fuzz check is also not continuous
(FIND-064-006). Unpublished registries and the absence of hosted
provenance remain the accepted TM-18 residual (FIND-064-010).

## 4. Residual exclusions

These are explicit exclusions of this review, consistent with Threat Model
§2.3 and §16. They are stated as limits, not as controls.

1. **T1 and T2 are not sandboxed.** Native Rust, Python callbacks, and
   JavaScript host adapters run with full process or page authority. This
   review offers **no guarantee against a malicious native in-process
   extension**. Only the optional Wasmtime host is an isolation boundary,
   and only for guests loaded through it with deny-by-default WASI.
   Recorded as FIND-064-008 (NFR-SEC-001, TM-06).
2. **Journal checksums are not authentication.** The chain detects
   accidental corruption and unsynchronized modification. It does not
   defend against an attacker who can rewrite records, metadata,
   snapshots, and the head together. Recorded as FIND-064-009 (TM-12).
3. **External effects are at-least-once or explicitly uncertain.** No
   exactly-once claim is made or implied. Recorded as FIND-064-011.
4. **Registries are unpublished.** No crates.io, PyPI, or npm artifact
   exists, so registry-side provenance, signature, and revocation controls
   could not be exercised. Recorded as FIND-064-010 (TM-18).
5. Deployment-owned systems remain out of scope: identity proofing, IdP,
   KMS or secrets manager, DLP, malware scanning, tenant policy,
   retention and legal hold, network controls, and store encryption
   (Threat Model §15).
6. No dynamic testing was performed: no live fuzz campaign, no runtime
   exploitation, no hosted soak, and no penetration test. Findings rest on
   source, test, and configuration reading.
7. A malicious host application or OS administrator is outside the
   isolation guarantee.

## 5. Outcome

Twenty findings are recorded in [findings.md](findings.md). After the
implementer closed FIND-064-001: zero `Open` Critical/High, three `Open`
Medium, three `Open` Low, four `Accepted` documented residuals, and ten
`Closed` (nine controls this review already found enforced, plus the
directory-cache deserialize fix).

**PR-064-A01 is satisfied.** Every Critical/High finding is `Closed` or
`Accepted`. No `Accepted` finding waives an Engineering Standards rule,
so no `docs/implementation/exceptions-register.md` row is created by this
review; in particular nothing here waives kernel I/O, a seventh port, or
an unbounded queue — the unbounded settlement map remains `Open`
(FIND-064-003) precisely because it is not waivable.

This review is PR-064 evidence only. It is not a G8 decision, does not
record `G8-D-*`, does not cut or publish `1.0.0`, and does not edit
`docs/planning/`.

## Related

- [findings.md](findings.md)
- [PR-064 execution plan](plan.md)
- [Threat Model G7 review](../../threat-model-g7-review.md)
- [Threat Model G8 review](../../threat-model-g8-review.md)
- [Security and Threat Model](../../../planning/06-finstack-ai-security-threat-model.md)
- [SECURITY.md](../../../../SECURITY.md)
- [Trust levels](../../../site/security-trust-levels.md)
- [Security deployment gates](../../../site/security-deployment.md)
