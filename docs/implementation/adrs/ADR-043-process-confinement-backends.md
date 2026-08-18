# ADR-043: Process confinement backends

## Status

Accepted

## Date

2026-08-17

## Accountable role

Runtime/security owner

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

FR-08 requires local process confinement for the trusted-native shell
toolset without adding a seventh port, without remote/E2B sandboxes, and
without upgrading the shell crate's T1 label. The filesystem toolset
already holds a capability-scoped root. The shell already has a labeled
unconfined `std::process` runner.

Platform primitives differ:

- Linux: Landlock plus `no_new_privs`
- macOS: Seatbelt (`sandbox_init` / `sandbox_free_error`)
- Windows: restricted token plus Job Object
- Other targets: no supported primitive

Apple documents `sandbox_init` as deprecated. The API remains the only
in-process Seatbelt entry that can run in a forked child before `exec`
without a helper binary.

## Decision

Process confinement is an **internal runtime service**, not a port:

`ProcessConfinement::spawn(command, profile) -> Result<Child, ConfinementError>`

- `profile.root` is the same authorized directory the filesystem/shell
  toolset already holds. The service does not invent a second root.
- The host backend is selected at compile time. Missing or unusable
  primitives **fail closed**: the service returns a stable
  `process_confinement_unavailable` error and never spawns unconfined.
- Linux applies Landlock path-beneath rules and `no_new_privs` in the
  child `pre_exec`.
- macOS applies a Seatbelt profile in the child `pre_exec`. The ADR
  records Apple's deprecation: a future replacement (App Sandbox
  entitlements, `sandbox-exec` helper, or a new Apple API) requires a
  superseding ADR. Deprecated is accepted for 1.2-oriented local
  confinement; it is not treated as an unconfined fallback.
  Deny-default Seatbelt aborts under current Darwin + dyld shared
  cache. The enforced profile is allow-default, deny `file-read-data`
  on user-writable trees, then re-allow the capability root.
- Windows requires restricted-token creation and assigns the child to a
  Job Object. `std::process::Command` cannot take the restricted token
  (`CreateProcessAsUser` is a remaining gap). A host that cannot create
  the token or the job refuses to spawn.
- The existing unconfined `std::process` runner stays reachable and is
  labeled `ProcessSandboxKind::UnconfinedStdProcess`.
- FR-03 stdio is a follow-on consumer of the same service. No E2B or
  remote sandbox.

The shell crate remains T1 trusted-native. Confinement is a local OS
control, not isolation.

## Consequences

- `ShellToolset::try_with_confinement` consumes the service and fails
  closed without a root or backend.
- Hostile reads outside the declared root fail on supported targets.
- Unsupported targets and the forced-unavailable constructor never
  spawn.
- Seatbelt deprecation is an accepted residual risk until Apple ships a
  supported in-process replacement.
- Windows token-on-child remains incomplete; Job Object assignment is
  the live Windows boundary.

## Rejected alternatives

**Seventh port.** Rejected: confinement is host machinery, not a kernel
port.

**Remote / E2B sandbox.** Rejected: FR-08 is local OS primitives only.

**Upgrade the shell crate to a T3/isolation label.** Rejected: native
shell still inherits host authority; Landlock/Seatbelt/Job Object do
not make it isolated.

**Fail open to unconfined `std::process` when the primitive is
missing.** Rejected: that would silently remove the control.

**Ship confinement inside the shell crate.** Rejected: FR-03 stdio
should consume the same service.

## Compatibility and schema-change classification

No journal kind, kernel input, or schema change. Additive runtime
service and optional shell constructor.

## Security classification

- References: SEC-INV-006, SEC-INV-012; TM-03, TM-06
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: Local confinement notes for TM-03
  (filesystem/shell escape) and TM-06 (native code mistaken for
  isolated). This record is **threat-model notes only**. It does **not**
  invent a passed threat-model review id.
- Residual: Seatbelt is Apple-deprecated; deny-default is not viable on
  current Darwin so macOS denies user-writable `file-read-data` rather
  than default-deny; Windows does not yet apply the restricted token to
  the child process; Landlock ABI/kernel support varies; T1 label is
  unchanged.

## Affected requirements, design, and delivery

- Affected requirements: FR-08
- Affected Technical Design: Technical Design toolset/shell confinement
  notes. Planning files are not edited.
- Implementation: FR-08 Task 16
- Follow-on consumer: FR-03 stdio

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR. Adding a seventh port,
remote sandbox, T1→isolated relabel, or fail-open fallback requires
that path. Replacing Seatbelt after Apple removal also requires a
superseding ADR.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to execute FR-08 Task 16
  locally only, without publication
- Implementation evidence: Missing (Task 16 landed locally; no evidence
  id; no threat-model review id)
