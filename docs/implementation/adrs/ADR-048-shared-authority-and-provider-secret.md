# ADR-048: Shared authority check and provider secret

## Status

Accepted

## Date

2026-08-18

## Accountable role

Core/runtime lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

Tenant-scope authority checks are copy-pasted. Shell and filesystem name
`verify_authority`; calculator and e2b inline the same predicate; MCP,
subagent, and skills have none. A later wave cannot call a helper that
does not exist on the Tool port.

Dedicated providers each own a `SecretString`. The gateway additionally
owns `Authentication`, `CredentialReference`, and `CredentialStore`.
Promotion into `provider_util` requires those types plus `SecretString`
to move together, or the store cannot live there without generics.
`CredentialStore` values are `Authentication`. Shared secret validation
must not own a stable adapter code; each crate maps rejection to its own
string.

## Decision

- `verify_authority(&ToolCallContext)` lives in `ports/tool/`. It is the
  shared predicate: principal `tenant_scope` vs locator `tenant_scope`.
  A principal with no tenant scope is accepted. A mismatch returns
  `TOOL_POLICY_DENIED`. Leaf crates switch to this function in their
  owning waves; this record does not migrate them.
- `provider_util/secret.rs` owns `SecretString`,
  `SecretRejected { Empty, TooLong, NonAscii }`, `SECRET_MAX_BYTES`, and
  `secret_is_valid`. The historical helper keeps its NUL check.
  `SecretString` also rejects non-ASCII. The shared type must not own a
  stable error code.
- `provider_util/credentials.rs` owns
  `Authentication { None, Bearer, ApiKey }`, `CredentialReference`, and
  `CredentialStore`. The store is host-supplied and never reads
  environment variables. Leaf crates re-export so
  `finstack_ai_provider_openai::SecretString` keeps working until they
  switch imports. Runtime crate-root re-exports land with this record
  so the freeze gate sees the types. Leaf re-exports wait for the
  credential wave.

## Consequences

- Later toolset waves call one function instead of copying the
  predicate.
- Dedicated providers gain a named-credential path without a new crate.
- Existing `try_new` error codes stay byte-for-byte via `SecretRejected`
  mapping in the leaf crates.

## Rejected alternatives

**`ReceiverModelStream` in `provider_util`.** Rejected: no current
requirement for a shared stream adapter.

**`apply_budget` in `provider_util`.** Rejected: budget application stays
on the owning port.

**`tool_catalog_digest` in `provider_util`.** Rejected: it drops to two
consumers after gateway deletion.

**Give `SecretString` a stable error code.** Rejected: each crate already
owns its adapter code string.

**A seventh port or a new `RecordBody` for authority.** Rejected: the
check is a Tool-port predicate on committed context.

## Compatibility and schema-change classification

Additive public runtime function `verify_authority` plus crate-root
`SecretString`, `SecretRejected`, `Authentication`,
`CredentialReference`, `CredentialRejected`, and `CredentialStore`.
Leaf crate re-exports wait for the credential wave. Pre-1.0 extensions
may switch imports without a major bump. No journal `RecordBody`, WIT
world, or remote-protocol meaning change.

## Security classification

- References: SEC-INV-001, SEC-INV-004; TM-02 (tenant isolation on tool
  dispatch); TM-04 (secrets stay references / redacted)
- Threat Model review trigger: leaf migrations that start calling
  `verify_authority` (shell, subagent, MCP, e2b, filesystem) complete
  their own §18 reviews. This record does **not** invent a review id.
- Residual: MCP, subagent, and skills still omit the check until their
  owning waves.

## Affected requirements, design, and delivery

- Affected Technical Design: Tool port, provider batteries. Planning
  files are amended in the same review unit that accepts this ADR
  (Phase 14).
- Implementation: `verify_authority` lands with this authorization
  slice. Secret and credential types land in `provider_util`. Leaf
  usage is later waves.
- Unchanged: no seventh port; no `Agent::gateway` retarget; no leaf
  crate migration in this slice.

## Supersession metadata

None. This ADR supersedes no prior ADR.

## Reconsideration conditions

May change only through a new superseding ADR. Moving authority into
the kernel, giving `SecretString` a stable code, or adding
`ReceiverModelStream` / `apply_budget` / `tool_catalog_digest` requires
that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to authorize Phase 14
- Implementation evidence: Missing (local types only; no invented
  evidence or review id)
