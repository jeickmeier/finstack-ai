# PR-064 independent-review findings

Reviewer: **Claude Opus 5** (independent-review subagent, session distinct
from the PR-064 implementation session).
Date: 2026-08-15. Method and scope: [independent-review.md](independent-review.md).

Severity uses the [SECURITY.md](../../../../SECURITY.md) rubric. Surface
uses the Threat Model §13.3 vocabulary (`plugin`, `fs-shell`, `protocol`,
`secret`, or another §13.3 bullet). No exploit proof-of-concept, credential,
token, or secret value appears in this file. No GHSA or CVE identifier is
asserted; none exists for this project.

## Register

| ID | Severity | Surface | Status | Remediation date | Evidence |
| --- | --- | --- | --- | --- | --- |
| FIND-064-001 | High | plugin | **Closed** | Closed 2026-08-15 | [§1](#find-064-001) · `plugins/finstack-ai-plugin-host/src/{cache.rs,host.rs}` |
| FIND-064-002 | Medium | fs-shell | Open | PR-064 (recommended) | [§2](#find-064-002) · `extensions/toolsets/finstack-ai-tools-shell/src/lib.rs` |
| FIND-064-003 | Medium | protocol | **Closed** | Closed 2026-08-17 | [§3](#find-064-003) · `crates/finstack-ai-server/src/session.rs` |
| FIND-064-004 | Medium | dependency/build/release provenance | **Closed** | Closed 2026-08-15 | [§4](#find-064-004) · `deny.toml`; `mise run supply-chain` |
| FIND-064-005 | Low | protocol | Open | PR-065 (release engineering) | [§5](#find-064-005) · `crates/finstack-ai-server/src/auth.rs` |
| FIND-064-006 | Low | dependency/build/release provenance | Open | PR-064 (recommended) | [§6](#find-064-006) · `mise.toml`, `.github/workflows/ci.yml` |
| FIND-064-007 | Low | fs-shell | **Closed** | Closed 2026-08-17 | [§7](#find-064-007) · `extensions/toolsets/finstack-ai-tools-shell/src/lib.rs` |
| FIND-064-008 | High | plugin | **Accepted** | Not applicable — permanent non-goal; see §8 | [§8](#find-064-008) · `docs/site/security-trust-levels.md` |
| FIND-064-009 | Medium | journal/recovery integrity | **Accepted** | Not applicable — permanent residual; see §9 | [§9](#find-064-009) · `crates/finstack-ai-protocol/src/journal.rs` |
| FIND-064-010 | Medium | dependency/build/release provenance | **Accepted** | First registry publish (PR-065/PR-066) | [§10](#find-064-010) · `docs/implementation/release-rehearsal.md` |
| FIND-064-011 | Medium | journal/recovery integrity | **Accepted** | Not applicable — ADR-013 durable decision; see §11 | [§11](#find-064-011) · `crates/finstack-ai-test/tests/crash_prefix.rs` |
| FIND-064-012 | Medium | protocol | Closed | Closed 2026-08-15 | [§12](#find-064-012) · `crates/finstack-ai-protocol/src/frame.rs`, `fuzz/fuzz_targets/remote_frame.rs` |
| FIND-064-013 | High | protocol | Closed | Closed 2026-08-15 | [§13](#find-064-013) · `crates/finstack-ai-server/src/tests.rs` |
| FIND-064-014 | High | interaction authorization / privileged dispatch | Closed | Closed 2026-08-15 | [§14](#find-064-014) · `crates/finstack-ai-test/tests/interaction.rs` |
| FIND-064-015 | High | plugin | Closed | Closed 2026-08-15 | [§15](#find-064-015) · `plugins/finstack-ai-plugin-host/src/instantiate.rs` |
| FIND-064-016 | Medium | fs-shell | Closed | Closed 2026-08-15 | [§16](#find-064-016) · `extensions/toolsets/finstack-ai-tools-filesystem/src/tests.rs` |
| FIND-064-017 | High | secret | Closed | Closed 2026-08-15 | [§17](#find-064-017) · `crates/finstack-ai-runtime/src/observer_export.rs` |
| FIND-064-018 | Medium | journal/recovery integrity | Closed | Closed 2026-08-15 | [§18](#find-064-018) · `crates/finstack-ai-test/tests/crash_prefix.rs` |
| FIND-064-019 | Medium | observer/exporter bounds | Closed | Closed 2026-08-15 | [§19](#find-064-019) · `crates/finstack-ai-runtime/src/observer_queue.rs` |
| FIND-064-020 | Medium | Python/JS callback lifecycle | Closed | Closed 2026-08-15 | [§20](#find-064-020) · `bindings/finstack-ai-python/src/callbacks.rs` |

### Counts

| Severity | Open | Accepted | Closed | Total |
| --- | --- | --- | --- | --- |
| Critical | 0 | 0 | 0 | 0 |
| High | 0 | 1 | 5 | 6 |
| Medium | 2 | 3 | 6 | 11 |
| Low | 3 | 0 | 0 | 3 |
| Total | 5 | 4 | 11 | 20 |

**PR-064-A01 status: satisfied.** Every Critical/High finding is
`Closed` or `Accepted`. FIND-064-001 is `Closed` by the directory-cache
deserialize fix. Open rows remaining are Medium/Low and do not block
A01. No `G8-D-*` is written from this register.

### Register notes

- No `Accepted` finding waives an Engineering Standards rule, so this
  review creates no row in
  [`docs/implementation/exceptions-register.md`](../../exceptions-register.md).
  Nothing here waives kernel I/O, a seventh port, or an unbounded queue.
  The unbounded settlement map (FIND-064-003) is filed `Open` for that
  reason and is deliberately not accepted.
- FIND-064-008, FIND-064-009, and FIND-064-011 are permanent documented
  non-goals or residuals (Threat Model §2.3 and §16). They have no
  remediation date because there is no planned remediation; each carries a
  re-review trigger instead, stated in its section. This is recorded
  explicitly rather than filling the field with a fabricated date.
- `Closed` rows are controls this review probed for a specific weakness,
  found implemented, and confirmed enforced by a named first-party test.
  They are recorded so the §13.3 coverage claim is auditable, not because a
  defect was fixed in this PR.

---

## FIND-064-001

**Compiled plugin components are deserialized from an unverified on-disk cache**

| Field | Value |
| --- | --- |
| ID | FIND-064-001 |
| Severity | High |
| Surface | plugin (WIT/Wasmtime permissions, limits, signatures, cache identity) |
| Status | **Closed** |
| Owner | PR-064 implementer |
| Owning module | `plugins/finstack-ai-plugin-host/src/cache.rs`, `plugins/finstack-ai-plugin-host/src/host.rs` |
| Remediation date | Closed 2026-08-15 |
| Evidence | `ComponentCache::trusted_precompiled`; `host::tests::directory_cache_tamper_is_not_deserialized`; `cache::tests::only_memory_artifacts_are_trusted_for_deserialize` |
| Threat model | TM-07, TM-08, SEC-INV-006, §8.4 |

### Defect

`PluginHost::load` computes a cache key over `{abi, digest, engine,
target}` and, when the key hits, passes the stored bytes directly to
`unsafe Component::deserialize`. In directory mode
(`PluginHostConfig::try_new(Some(dir), ..)`) those bytes are read from the
filesystem with `fs::read`; the filename is the only binding between the
artifact and the component it was compiled from. Three problems compound:

1. **No integrity check on the artifact.** The stored `.cwasm` is never
   re-verified against the component digest that produced it, nor against
   any host-held hash or MAC. A cache key is a naming scheme, not
   authentication.
2. **Every package-identity control is bypassed on a hit.** The manifest
   self-digest, the ed25519 signature verified against configured trust
   roots, and the lockfile `component_digest` all run against the *source*
   component bytes. On a cache hit the source bytes are never compiled and
   the attacker-supplied artifact is what actually executes, under the
   signed and locked plugin identity and with that plugin's granted
   capabilities.
3. **`Component::deserialize` is documented-unsafe on untrusted input.**
   The `// SAFETY:` comment in `deserialize_component` asserts the bytes are
   this engine's own `precompile_component` output. That invariant holds for
   the in-memory cache and does not hold for the directory cache. A crafted
   artifact is undefined behaviour and can escape the WebAssembly sandbox
   into native execution in the host process.

The cache directory is created with `fs::create_dir_all` and default
permissions, and no documentation identifies it as a trust boundary —
`docs/site/security-deployment.md` and `docs/site/plugin.md` list plugin
trust roots and deny-by-default WASI but say nothing about the compiled
cache. A deployer can therefore reasonably place it on a shared volume,
a shared build agent, or a container mount that a lower-privileged process
can write.

### Preconditions and limits

Requires write access to the configured cache directory by a principal
that is not the host process owner. Threat Model §5 explicitly includes
"a local attacker with access to journal, snapshot, artifact, **cache**, or
log files", so this actor is in scope and is not covered by the
trusted-in-process residual (FIND-064-008), which concerns malicious
*code*, not a data file. Deployments using the default in-memory cache
(`cache_dir: None`) are unaffected; no in-tree caller enables directory
mode outside tests.

### Suggested remediation

Any one of these closes the finding; the first two are complementary:

- Verify before deserializing: record the artifact hash alongside the entry
  at `put` time in host-owned state and compare on `get`, or store a keyed
  MAC over the artifact using a per-host key, and treat a mismatch as a
  cache miss that recompiles from the verified source bytes.
- Harden the directory: create it with `0o700`, and fail closed at
  `ComponentCache::directory` when the resolved directory is group- or
  world-writable.
- Document the compiled-component cache directory as a trust boundary in
  `docs/site/security-deployment.md` and in the `PluginHostConfig::try_new`
  rustdoc, and narrow the `// SAFETY:` comment so it states the actual
  precondition rather than an invariant the directory backend does not hold.

Add a regression test that writes a foreign artifact under a valid cache
key and asserts the host does not deserialize it.

### Remediation (landed)

`PluginHost::load` deserializes only `ComponentCache::trusted_precompiled`
bytes. That method returns the in-memory backend and always returns
`None` for directory mode. On a directory miss (every directory load),
`load` runs `Engine::precompile_component` on the already-verified source
bytes, `put`s the artifact for later identity/existence hits, and
deserializes those in-process bytes. A sidecar hash is not used: a
writer who can replace `.cwasm` can replace the sidecar.

`host::tests::directory_cache_tamper_is_not_deserialized` overwrites the
directory `.cwasm` with `0xFF` bytes and asserts `load` still succeeds
from source. `cache::tests::only_memory_artifacts_are_trusted_for_deserialize`
proves directory `get` hits are not trusted for deserialize. The
`// SAFETY:` comment on `deserialize_component` now states the in-process
precompile precondition.

---

## FIND-064-002

**Shell `cwd` is re-resolved by path after handle-relative authorization**

| Field | Value |
| --- | --- |
| ID | FIND-064-002 |
| Severity | Medium |
| Surface | fs-shell |
| Status | Open |
| Owner | PR-064 implementer |
| Owning module | `extensions/toolsets/finstack-ai-tools-shell/src/lib.rs` (`unix::Root::authorize_cwd`, `unix::open_no_follow_dir`, `apply_authorized_cwd`) |
| Remediation date | PR-064 (recommended) |
| Evidence | independent-review.md §3.4; absence of any symlink or cwd case in `extensions/toolsets/finstack-ai-tools-shell/src/tests.rs` |
| Threat model | TM-03 ("symlink/rename race tests"), §8.1 |

### Defect

`Root::authorize_cwd` validates the caller-supplied relative path
correctly: it rejects absolute paths, `..`, `.`, empty segments, `//`,
backslashes, and NUL, then walks the path one component at a time with
`openat(O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)` starting from a duplicate of
the retained root descriptor. It then **drops** the validated descriptor and
returns `self.path.join(relative)` — a path string.

`apply_authorized_cwd` re-opens that string with
`open_no_follow_dir`, which calls `open(path, O_DIRECTORY | O_NOFOLLOW |
O_CLOEXEC)`. `O_NOFOLLOW` constrains only the **final** component of a
path. Every intermediate component is re-resolved by the kernel with
symlink following, after the no-follow check has already passed.

An attacker who can write inside the authorized root and replace an
intermediate directory with a symlink between validation and the re-open
can therefore redirect the child process's working directory outside the
root. A single-component `cwd` is safe because `O_NOFOLLOW` covers it; the
race needs two or more segments.

Impact is scope escape rather than program escape: the executable is still
allowlisted and the environment is still cleared, but an allowlisted
program's relative reads and writes now land outside the authorized root.

This is the same authorize-then-reopen shape the filesystem toolset
deliberately avoids by keeping the descriptor, and it is covered there by
`symlink_swap_between_authorization_and_open_never_reads_outside` and
`rename_after_open_reads_the_authorized_object_not_replacement`. The shell
crate has no equivalent test; `tests.rs` contains no `symlink` or `cwd`
case at all.

### Suggested remediation

Keep the validated `OwnedFd` from `authorize_cwd` and `fchdir` that
descriptor in `pre_exec`, instead of discarding it and reopening by path.
`apply_authorized_cwd` already performs `fchdir`, so the change is to
thread the authorized descriptor through rather than the joined
`PathBuf`. Note that `SandboxedCommand::cwd` is public, so either carry the
descriptor beside it in host-internal state or introduce an internal
authorized-cwd type rather than changing the public field. Add the
symlink-swap race test that the filesystem toolset already has.

---

## FIND-064-003

**Reference server retains settlement receipts without bound**

| Field | Value |
| --- | --- |
| ID | FIND-064-003 |
| Severity | Medium |
| Surface | protocol (remote framing, authentication hooks, authorization context) |
| Status | **Closed** |
| Owner | PR-064 implementer |
| Owning module | `crates/finstack-ai-server/src/session.rs` (`SessionReplica::receipts`, `apply_command`) |
| Remediation date | Closed 2026-08-17 |
| Evidence | `SessionReplica::with_receipt_cap`; `session::tests::receipt_map_fails_closed_at_cap`; `session::tests::receipt_conflict_is_constant_time_lookup` |
| Threat model | SEC-INV-007, TM-16, §8.5 ("outstanding-operation limits are configurable and observable"), NFR-REL-004 |

### Defect

`SessionReplica::receipts` is a `HashMap<(String, Digest),
RemoteCommandResult>` that gains one entry per distinct `command_id` and is
never evicted, expired, or capped. Two consequences for an authenticated
remote principal on a long-lived session:

- **Memory.** Each accepted command retains its identifier and receipt for
  the lifetime of the replica. There is no retention horizon, no capacity
  ceiling, and no configurable outstanding-operation limit.
- **CPU.** The conflict check scans every key linearly
  (`self.receipts.keys().any(|(command_id, _)| ..)`) on every command that
  is not an exact replay, so per-command cost grows linearly with history.

The connection is otherwise well bounded — pre-auth and post-auth frame
ceilings, handshake deadline, authenticate-attempt cap, and an item/byte
credit window with an ack deadline — which makes the missing settlement
bound the one unbounded resource on the authenticated path. The runtime's
external-completion router has an `IdempotencyHorizon`
(`crates/finstack-ai-runtime/src/ingress.rs`); the reference replica has no
equivalent.

This finding is **Closed**. The replica now indexes receipts by
`command_id` (O(1) conflict) and fails closed at a configurable
retention cap (`DEFAULT_RECEIPT_CAP`, `with_receipt_cap`).

This was filed `Open` and deliberately **not** `Accepted`: an unbounded
resource is not a waivable deviation.

### Suggested remediation

Give the replica a configurable retention bound analogous to
`IdempotencyHorizon` — a capacity ceiling plus expiry, with an explicit
fail-closed outcome (not silent forgetting that would turn a replay into a
new command) — and index receipts by `command_id` so the conflict check is
a single lookup instead of a scan.

---

## FIND-064-004

**No continuous dependency advisory, license, or source-policy check**

| Field | Value |
| --- | --- |
| ID | FIND-064-004 |
| Severity | Medium |
| Surface | dependency/build/release provenance |
| Status | **Closed** |
| Owner | Release engineering |
| Owning module | `mise.toml`, `.github/workflows/ci.yml` |
| Remediation date | Closed 2026-08-15 by PR-065 |
| Evidence | `deny.toml`; `mise run supply-chain`; `.github/workflows/ci.yml` Supply chain step |
| Threat model | TM-18, §12, §13.1 first bullet |

### Defect

Threat Model §13.1 lists "dependency advisory/license/source policy" as a
**continuous** check, and TM-18's required controls include "dependency
sources, advisories, licenses, and exceptions are checked continuously".
No such check exists. Neither `mise.toml` nor `.github/workflows/ci.yml`
invokes `cargo-audit`, `cargo-deny`, `osv-scanner`, or any equivalent, and
there is no `deny.toml` or advisory-exception file in the repository.
`mise run docs-license` sweeps this project's *own* published package
license metadata; it does not evaluate dependency licenses or advisories.
`tools/wasm_package/check.py secrets` covers bundle secret scanning, a
different §13.1 bullet.

The surrounding supply-chain posture is otherwise strong and should not be
restated as weak: GitHub Actions are pinned to commit SHAs, the Rust
toolchain and every auxiliary tool are pinned in `mise.toml`, CI runs with
`persist-credentials: false` and `contents: read`, every cargo invocation
uses `--locked` against a checked-in `Cargo.lock`, and
`mise run release-rehearsal` compares two staging runs by checksum. The
missing piece is advisory and license policy over the dependency graph.

### Suggested remediation

Add a pinned advisory and license gate — `cargo-deny` covers advisories,
licenses, sources, and bans in one tool with a checked-in `deny.toml` — as
a `mise` task, wire it into `mise run ci` and the CI workflow, and record
any accepted advisory in the configuration with a reason rather than
silently.

### Remediation

PR-065 restored the pre-`f915cbf` `deny.toml`, pinned
`github:EmbarkStudios/cargo-deny` `0.20.2`, added `mise run supply-chain`
(workspace + isolated `fuzz/` manifest), wired it into `mise run ci` and
`.github/workflows/ci.yml`, and recorded the procedure in
`docs/implementation/release-engineering.md`.

---

## FIND-064-005

**Reference bearer verifier compares the token in non-constant time**

| Field | Value |
| --- | --- |
| ID | FIND-064-005 |
| Severity | Low |
| Surface | protocol (authentication hooks) |
| Status | Open |
| Owner | Release engineering |
| Owning module | `crates/finstack-ai-server/src/auth.rs` (`StaticAuthVerifier::verify`) |
| Remediation date | PR-065 (release engineering) |
| Evidence | independent-review.md §3.2 |
| Threat model | §8.5, §10 |

### Defect

`StaticAuthVerifier` rejects a bearer credential with
`if token.as_str() != self.bearer`. `str` inequality short-circuits on the
first differing byte, so comparison time correlates with the length of the
matching prefix. A remote client that can present many candidates over an
authenticated transport has, in principle, a timing oracle on the
configured secret.

Mitigating context, which is why this is Low rather than Medium: the type
is documented as the "Test/reference verifier"; applications supply their
own `AuthVerifier`; bearer credentials are refused on plaintext TCP; and
the connection enforces an authenticate-attempt cap of three plus a
handshake deadline, so per-connection sampling is limited and network
jitter dominates at this granularity. It is nevertheless a hardening gap in
a publicly exported type that carries no "do not use in production" label.

### Suggested remediation

Compare digests instead of raw strings — hash both the presented token and
the configured value with the existing `Digest` path and compare the fixed
32-byte outputs — or use a constant-time equality primitive. Add a rustdoc
line stating that the verifier is a reference implementation and that
applications own credential storage and rotation.

---

## FIND-064-006

**Fuzz smoke is not wired into `mise run ci` or the CI workflow**

| Field | Value |
| --- | --- |
| ID | FIND-064-006 |
| Severity | Low |
| Surface | dependency/build/release provenance (verification program) |
| Status | Open |
| Owner | PR-064 implementer |
| Owning module | `mise.toml` (`[tasks.ci]`), `.github/workflows/ci.yml` |
| Remediation date | PR-064 (recommended) |
| Evidence | independent-review.md §3.8; `mise.toml` `[tasks.fuzz-smoke]`; `fuzz/fuzz_targets/` |
| Threat model | §13.1 fifth bullet ("parser and deserializer fuzz targets") |

### Defect

PR-064 restored the isolated `cargo-fuzz` workspace with all seven targets
(`record_replay_json`, `kernel_transition_json`, `run_event_json`,
`message_json`, `remote_frame`, `wit_input`, `recovery_sequence`) and a
deterministic `mise run fuzz-smoke` task with a fixed seed and bounded
runs, time, length, and RSS. Neither `mise run ci` nor
`.github/workflows/ci.yml` invokes it, so a parser regression is not caught
by the required check set and §13.1's fuzz bullet is not continuous.

This is Low because the harness, corpora, and determinism are already in
place; only the wiring is missing.

### Suggested remediation

Add `mise run fuzz-smoke` to `[tasks.ci]` and to the CI workflow, or add a
separate scheduled job if nightly-toolchain installation makes the required
path too slow. Consumers must still not require nightly for ordinary
builds.

---

## FIND-064-007

**Shell output ceiling is applied only after both pipes are drained**

| Field | Value |
| --- | --- |
| ID | FIND-064-007 |
| Severity | Low |
| Surface | fs-shell |
| Status | **Closed** |
| Owner | Release engineering |
| Owning module | `extensions/toolsets/finstack-ai-tools-shell/src/lib.rs` (`run_process`) |
| Remediation date | Closed 2026-08-17 |
| Evidence | Incremental bounded pipe read; `tests::hostile_stdout_is_killed_at_the_byte_cap` |
| Threat model | TM-03 ("bounded output/time"), SEC-INV-007 |

### Defect

`run_process` polls `try_wait` and reads `stdout` and `stderr` to end only
after the child has exited, then compares the combined length against
`max_output_bytes`. Two consequences:

- A child that writes more than the OS pipe capacity blocks on write, never
  exits, and is reaped by the timeout branch. The caller sees
  `shell_timeout` (`ErrorCategory::Deadline`) rather than
  `shell_limit_exceeded` (`ErrorCategory::Limit`), so an output flood is
  reported as a deadline breach and the configured output ceiling never
  fires.
- `read_to_end` has no explicit cap; retained bytes are bounded only by the
  operating system's pipe buffer.

The outcome is fail-closed and the blast radius is small — memory is capped
by pipe capacity and the process is killed at the deadline — so this is a
correctness and diagnosability issue rather than an availability one.

### Suggested remediation

Read both pipes incrementally while the child runs, using a bounded reader
that stops at `max_output_bytes`, and kill with `shell_limit_exceeded`
once the ceiling is crossed. Incremental reading also removes the
pipe-capacity deadlock for well-behaved children that simply produce a lot
of output.

---

## FIND-064-008

**Accepted residual: in-process native, Python, and JavaScript code is trusted, not sandboxed**

| Field | Value |
| --- | --- |
| ID | FIND-064-008 |
| Severity | High |
| Surface | plugin / other §13.3 (trust boundaries) |
| Status | **Accepted** |
| Owner | Deploying application / host administrator |
| Remediation date | Not applicable — permanent product non-goal. Re-review trigger: Threat Model §18 (a new trust class, deployment topology, privileged host capability, or change to plugin sandboxing), and re-confirmation at each subsequent gate. |
| Evidence | independent-review.md §4.1; `docs/site/security-trust-levels.md`; `docs/site/plugin.md` |
| Threat model | NFR-SEC-001, TM-06, §2.3 first bullet, §16 row 1 |

### Residual

`finstack-ai` does not sandbox arbitrary native Rust, Python, or JavaScript
code running in-process. T1 native extensions and T2 host-language
callbacks inherit full process or page authority: they can read process
memory, reach credentials the process holds, and subvert application
policy. **This review offers no guarantee against a malicious native
in-process extension.** Only the optional Wasmtime host
(`finstack-ai-plugin-host`) is an isolation boundary, and only for guests
loaded through it under deny-by-default WASI.

Accepted as an explicit, documented product non-goal rather than a defect.

### Compensating controls

- A single T0–T5 trust matrix at `docs/site/security-trust-levels.md`,
  linked from every starter and guide that registers an extension, stating
  plainly that in-process code is never a sandbox.
- Extension registration is explicit and opt-in; component identity and
  trust classification are visible in diagnostics.
- Minimal and default builds do not silently include privileged batteries,
  providers, telemetry exporters, plugin engines, or network services
  (SEC-INV-012); the default SDK bundle does not depend on Wasmtime.
- The T3 path exists for untrusted code, with deny-by-default WASI,
  resource-backed grants, fuel, epoch deadlines, and store limits
  (FIND-064-015).
- `docs/site/security-deployment.md` records the shell and computer-use
  sandbox choice as a deployment gate whose fail-closed default is
  "native in-process execution remains trusted (T1)".

This acceptance waives no Engineering Standards rule and creates no
exceptions-register row.

---

## FIND-064-009

**Accepted residual: journal checksums are tamper evidence, not authentication**

| Field | Value |
| --- | --- |
| ID | FIND-064-009 |
| Severity | Medium |
| Surface | journal/recovery integrity |
| Status | **Accepted** |
| Owner | Store owner / deployer |
| Remediation date | Not applicable — permanent residual. Re-review trigger: a deployment that requires adversarial tamper evidence, or a Threat Model §18 change to journal or snapshot semantics. |
| Evidence | independent-review.md §§3.1, 4.2; `crates/finstack-ai-protocol/src/journal.rs`; `docs/implementation/threat-model-g7-review.md` TM-12 row |
| Threat model | TM-12, §16 row 2 |

### Residual

The record chain detects accidental corruption, truncation, reordering, and
unsynchronized modification, and classifies them as explicit errors rather
than guessed state. It does **not** authenticate history against an
attacker who can rewrite records, metadata, snapshots, and the head
together, because the chain is an unkeyed hash the attacker can recompute.

Already recorded as an accepted residual in the G7 implemented-control
review; re-affirmed here rather than re-litigated.

### Compensating controls

- Atomic append, sequence and version checks, deterministic encoding, and
  fail-closed corruption classification (SEC-INV-010).
- Snapshots are never authoritative; mismatch discards and rebuilds.
- `docs/site/security-deployment.md` makes journal and artifact encryption
  and key rotation a deployment gate whose fail-closed default explicitly
  says not to claim encryption from checksums.
- Deployers protect storage with access control, encryption, and backups,
  and add external signatures or anchoring where adversarial tamper
  evidence is required.

---

## FIND-064-010

**Accepted residual: registries are unpublished, so hosted provenance is unexercised**

| Field | Value |
| --- | --- |
| ID | FIND-064-010 |
| Severity | Medium |
| Surface | dependency/build/release provenance |
| Status | **Accepted** |
| Owner | Release owner |
| Remediation date | First registry publish (PR-065 / PR-066) |
| Evidence | independent-review.md §4.4; `docs/implementation/release-rehearsal.md`; `SECURITY.md` supported versions; `docs/implementation/threat-model-g7-review.md` TM-18 row |
| Threat model | TM-18, §12, §14 |

### Residual

No crates.io, PyPI, or npm artifact exists for this project. Registry-side
provenance attestation, package-ownership verification, signature
publication, and release revocation therefore could not be exercised by
this review, and no claim is made that they work. SBOMs and checksums exist
only as local rehearsal output.

This is the same unpublished-checkpoint residual accepted at G6 and G7 and
is scoped to publication mechanics. It does **not** cover the missing
continuous dependency advisory check, which is filed separately and
`Open` as FIND-064-004.

### Compensating controls

- `mise run release-rehearsal` verifies two-run local staging checksum
  identity before any publish.
- Release artifacts are produced from tagged reviewed source; tag `v0.1.0`
  exists locally.
- All CI actions and toolchains are pinned to immutable versions or commit
  SHAs, and CI jobs run without persisted credentials.
- `SECURITY.md` states the supported-version policy and the private
  reporting path, and records that publication remains blocked on owner
  registry credentials.

---

## FIND-064-011

**Accepted residual: external effects remain at-least-once or explicitly uncertain**

| Field | Value |
| --- | --- |
| ID | FIND-064-011 |
| Severity | Medium |
| Surface | journal/recovery integrity and external completion |
| Status | **Accepted** |
| Owner | Adapter author / deploying application |
| Remediation date | Not applicable — durable decision recorded in ADR-013. Re-review trigger: any change to effect, completion, or terminal semantics (Threat Model §18). |
| Evidence | independent-review.md §§3.1, 4.3; `crates/finstack-ai-test/tests/crash_prefix.rs`; `crates/finstack-ai-runtime/src/ingress.rs` |
| Threat model | §2.3 third bullet, §16 row 3 |

### Residual

Commit-before-effect ordering guarantees that no external effect executes
before durable intent, but a crash between dispatch and settlement leaves
the external outcome at-least-once or explicitly uncertain. Stable
identifiers, normalized outcome digests, and reconciliation reduce
duplicate visibility; they do not make external systems exactly-once. No
exactly-once claim is made or implied anywhere in this review, and the
crash-prefix catalog is not evidence of one.

### Compensating controls

- Committed durable intent precedes every external effect (SEC-INV-002).
- Equivalent duplicates are idempotent and conflicting or late privileged
  inputs fail closed and are audited (SEC-INV-004), proven by the
  completion and interaction routers and their conflict tests.
- Uncertainty suspends the run rather than guessing an outcome.
- Adapters are expected to use provider idempotency where available and to
  surface uncertainty for operator or application resolution.
- The crash-prefix catalog proves every `EffectKind` and public lane
  operation restores to a legal class with no silent loss (NFR-REL-002).

---

## FIND-064-012

**Closed: pre-auth frame allocation, oversize, unknown family, and downgrade**

| Field | Value |
| --- | --- |
| ID | FIND-064-012 |
| Severity | Medium |
| Surface | protocol |
| Status | Closed |
| Remediation date | Closed 2026-08-15 |
| Evidence | `crates/finstack-ai-protocol/src/frame.rs` (`one_over_pre_auth_ceiling_fails_before_payload_allocation`, `exact_pre_auth_ceiling_is_accepted`, `truncated_header_fails_closed`); `crates/finstack-ai-protocol/src/handshake.rs` (`unknown_version_fails_before_session`, `missing_mandatory_feature_fails_closed`, `unknown_family_string_fails_decode`); `crates/finstack-ai-server/src/tests.rs::oversized_pre_auth_length_fails_before_allocation`; `fuzz/fuzz_targets/remote_frame.rs` |
| Threat model | TM-09, TM-16, SEC-INV-007 |

The declared length is checked against the ceiling before any payload
buffer is allocated, and `crates/finstack-ai-server/src/io.rs` preserves
that ordering. Pre-auth and post-auth ceilings are separate. Envelopes are
family-tagged with `deny_unknown_fields`; a `Process` body decoded as
`Remote` fails with `unknown_payload_family` before session acquisition.
Version selection honours both downgrade floors with no fallback, and
mandatory features are required at hello. Closed — the controls exist and
named tests plus a fuzz target enforce them.

---

## FIND-064-013

**Closed: cross-tenant session acquisition and command acceptance**

| Field | Value |
| --- | --- |
| ID | FIND-064-013 |
| Severity | High |
| Surface | protocol |
| Status | Closed |
| Remediation date | Closed 2026-08-15 |
| Evidence | `crates/finstack-ai-server/src/tests.rs` (`unknown_locator_does_not_reveal_existence`, `bearer_over_plaintext_is_rejected_and_audited`, `second_writer_is_busy_without_confirming_session`, `command_idempotency_replays_and_conflicts`); `crates/finstack-ai-server/src/replica.rs` (`plan_reconnect`, `apply_command`) |
| Threat model | TM-19, SEC-INV-008, §8.5 |

Tenant scope is checked three times on the authenticated path: the
client-declared scope against the authenticated context in `post_auth`,
again in `plan_reconnect`, and again in `apply_command` against both the
authenticated context and the replica's own scope. Every failure — unknown
session, wrong tenant, busy writer — collapses to the same
`UnknownLocator` response, so error responses do not reveal whether an
identifier exists. Authentication precedes session acquisition, and bearer
credentials are refused on plaintext TCP. Closed.

---

## FIND-064-014

**Closed: spoofed, replayed, expired, or late privileged completion and interaction resolution**

| Field | Value |
| --- | --- |
| ID | FIND-064-014 |
| Severity | High |
| Surface | interaction authorization and privileged tool dispatch; external completion |
| Status | Closed |
| Remediation date | Closed 2026-08-15 |
| Evidence | `crates/finstack-ai-test/tests/interaction.rs` (`denied_approval_never_dispatches_the_protected_tool`, `expired_resolution_never_dispatches_the_protected_tool`, `expire_if_due_on_restore_never_dispatches`, `duplicate_resolution_is_idempotent`, `conflicting_resolution_fails_closed_and_is_audited`, `late_privileged_resolution_after_run_cancel_fails_closed`, `run_level_cancel_while_awaiting_interaction_closes_without_dispatching`); `crates/finstack-ai-runtime/src/ingress.rs` |
| Threat model | TM-10, TM-11, TM-14, SEC-INV-003, SEC-INV-004 |

Both routers require exact locator identity, matching tenant scope,
matching authorization evidence (principal, policy version, decision
identity), a known target, and a non-expired horizon before any commit.
Duplicates are idempotent, conflicts are durably rejected and audited, and
rejections are non-existence-revealing. Possession of an identifier alone
never authorizes an action. The named tests prove the privileged tool is
not dispatched on the deny, expiry, cancel, and late-resolution paths.
Closed.

---

## FIND-064-015

**Closed: ambient WASI, unresolved imports, and resource-limit bypass in the plugin host**

| Field | Value |
| --- | --- |
| ID | FIND-064-015 |
| Severity | High |
| Surface | plugin |
| Status | Closed |
| Remediation date | Closed 2026-08-15 |
| Evidence | `plugins/finstack-ai-plugin-host/src/instantiate.rs` (`unresolved_wasi_import_fails_closed`, `ungranted_filesystem_and_sockets_fail_closed`, `fixture_wats_fail_closed_without_grants`, `filesystem_links_only_with_preopen`, `fuel_exhaustion_is_contained_and_host_continues`, `memory_grow_past_ceiling_is_contained`, `guest_unreachable_is_contained`); `plugins/finstack-ai-plugin-host/src/grants.rs` (`default_grants_are_logging_and_blobs`, `secrets_cannot_be_offered`, `unoffered_filesystem_is_denied`) |
| Threat model | TM-07, SEC-INV-006, §8.4 |

Guests receive no ambient filesystem, network, environment, process, or
secret access. Manifest-requested permissions are intersected with the
host-offered grant set and fail closed on anything not offered; `secrets`
cannot be offered at all; and a granted name still does not link an
interface without a concrete backing resource. Fuel, epoch interruption,
and a store limiter bound execution, memory, tables, and instances, and
traps are contained with the host able to build a fresh store. Closed for
the permissions-and-limits control set; the cache-artifact gap is tracked
separately as FIND-064-001.

---

## FIND-064-016

**Closed: filesystem traversal, symlink race, and protected-path escape**

| Field | Value |
| --- | --- |
| ID | FIND-064-016 |
| Severity | Medium |
| Surface | fs-shell |
| Status | Closed |
| Remediation date | Closed 2026-08-15 |
| Evidence | `extensions/toolsets/finstack-ai-tools-filesystem/src/tests.rs` (`traversal_symlink_escape_and_protected_paths_fail_closed`, `symlink_swap_between_authorization_and_open_never_reads_outside`, `rename_after_open_reads_the_authorized_object_not_replacement`, `symlink_root_is_rejected_at_construction`, `environment_and_protected_secret_files_do_not_leak`, `oversized_output_requires_and_uses_exact_scoped_artifact_service`); `src/policy.rs`, `src/unix.rs` |
| Threat model | TM-03, TM-20, §8.1 |

Access is handle-relative throughout: one retained root descriptor plus
`openat` with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` per component, so no
operation re-resolves an absolute path and a symlink swapped in mid-walk
cannot redirect it. Path parsing rejects absolute paths, `..`, `.`, empty
and duplicate separators, backslashes, NUL, and over-length paths and
components; protected patterns are enforced at validation and while
walking. Byte, entry, depth, and match ceilings bound every operation and
cancellation is polled between steps. Oversized output goes to the scoped
artifact service rather than inline. Closed.

---

## FIND-064-017

**Closed: secret leakage into records, events, exports, and bundles**

| Field | Value |
| --- | --- |
| ID | FIND-064-017 |
| Severity | High |
| Surface | secret |
| Status | Closed |
| Remediation date | Closed 2026-08-15 |
| Evidence | `crates/finstack-ai-runtime/src/observer_export.rs` canary test (asserts `Redacted` omits both the body and the canary value); `tools/wasm_package/check.py secrets` via `mise run check-wasm`; `crates/finstack-ai-server/src/auth.rs` (`AuthContext` holds no token); `crates/finstack-ai-server/src/connection.rs` (audit events carry locator and submission digests, never payloads or tokens) |
| Threat model | TM-04, TM-05, SEC-INV-005, §10, §11 |

Secrets are represented by references. Redaction happens before data
enters an exporter queue, and the diagnostic export has explicit
metadata-only, redacted, and full modes with a canary test on the redacted
path. The remote authentication context deliberately carries no raw token,
and audit rows carry digests rather than payloads. Tool and shell errors
use fixed `&'static str` messages with stable codes, so a credential
cannot reach an error string by interpolation. The browser bundle is
secret-scanned in CI. I found no path writing a secret value into a
record, event, audit row, bundle, or diagnostic export. Closed.

---

## FIND-064-018

**Closed: silent acceptance of a corrupt or truncated journal prefix**

| Field | Value |
| --- | --- |
| ID | FIND-064-018 |
| Severity | Medium |
| Surface | journal/recovery integrity |
| Status | Closed |
| Remediation date | Closed 2026-08-15 |
| Evidence | `crates/finstack-ai-protocol/src/journal.rs` (payload digest, envelope checksum, previous-checksum chain, chain verification with expected sequence); `crates/finstack-ai-test/tests/crash_prefix.rs::crash_prefix_catalog_covers_every_effect_kind_and_lane_operation`; `crates/finstack-ai-test/tests/journal_v1.rs`; `fuzz/fuzz_targets/recovery_sequence.rs` |
| Threat model | TM-12, TM-13, SEC-INV-010, NFR-REL-002 |

Recovery either produces valid state or an explicit corruption error; it
never guesses. The checksum projection excludes the diagnostic commit
timestamp so replay is deterministic, and each envelope cites its
predecessor, so truncation and reordering are detected. The crash-prefix
catalog asserts that every `EffectKind` and every public lane operation
has at least one named prefix restoring to a `LegalRestore` class, with no
silent loss. Closed for the fail-closed control; the authentication limit
of an unkeyed chain is the accepted residual FIND-064-009.

---

## FIND-064-019

**Closed: observer or exporter blockage, unbounded queue, or payload leak**

| Field | Value |
| --- | --- |
| ID | FIND-064-019 |
| Severity | Medium |
| Surface | other §13.3 (observability) |
| Status | Closed |
| Remediation date | Closed 2026-08-15 |
| Evidence | `crates/finstack-ai-runtime/src/observer_queue.rs` (capacity validated into `(0, 1_000_000]`, explicit `Block` / `DropProgress` / `Disconnect` overflow policy, drop counter, non-semantic overflow diagnostic); `crates/finstack-ai-runtime/src/observer_export.rs` |
| Threat model | TM-17, SEC-INV-009, NFR-REL-004, §11.1 |

Export queues are adapter-owned and bounded, with a validated capacity and
an explicit overflow policy per adapter. Overflow produces a counted drop
or a disconnect plus a non-semantic diagnostic — never a `RunEvent` kind —
so a slow or failing observer cannot block or alter execution or delay
durable completion. Redaction precedes the queue. Closed.

---

## FIND-064-020

**Closed: Python and JavaScript callback lifecycle and browser credential guidance**

| Field | Value |
| --- | --- |
| ID | FIND-064-020 |
| Severity | Medium |
| Surface | Python/JavaScript callback lifecycle and browser credential guidance |
| Status | Closed |
| Remediation date | Closed 2026-08-15 |
| Evidence | `bindings/finstack-ai-python/src/callbacks.rs` (validated per-callback timeout, cancellation signal exposed to the callback, bounded settle window, `run_blocking` for user code, narrow interpreter attach scopes, stable `python_callback_cancelled` / `python_callback_timeout` codes); `bindings/finstack-ai-python/tests/` (`test_callbacks.py`, `test_callback_conformance.py`, `test_handles.py`, `test_interactions.py`, `test_durable_restart.py`); `docs/site/wasm.md`; `docs/site/provider-security.md`; `docs/site/security-trust-levels.md` |
| Threat model | TM-05, TM-06, §8.2, §8.3 |

Callback entry has explicit lifetime, cancellation, timeout, and exception
normalization, and a callback that ignores cancellation cannot delay the
Rust run past its bounded settle window. Blocking user code does not run on
the async executor. Browser guidance states that provider keys must not be
embedded in the bundle and must terminate at a trusted same-origin proxy,
labels host callbacks T2 with page authority and no isolation, and marks
the IndexedDB adapter experimental and not crash-durable. Closed.

---

## Related

- [independent-review.md](independent-review.md)
- [PR-064 execution plan](plan.md)
- [Threat Model G7 review](../../threat-model-g7-review.md)
- [Exceptions register](../../exceptions-register.md)
- [SECURITY.md](../../../../SECURITY.md)
