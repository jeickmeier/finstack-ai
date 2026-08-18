# ADR-045: First-class SDK constructors

## Status

Accepted

## Date

2026-08-18

## Accountable role

Bindings lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

Product G-01 stays: a developer can still run an agent without
configuring a gateway, plugin host, database, or workflow. The planning
sentence is not edited.

Python already exposes `Agent.openai` / `anthropic` / `ollama` as
PyO3-owned factories with no WASM counterpart. That is binding debt.
This program must not add a fourth Python-only factory. First-class
gateway, remote-child, and E2B surfaces were requested as inherent
Rust methods that both bindings wrap.

I/O leaves stay out of the `wasm-host` cargo graph. Platform inability
is a Rust error, not a missing method.

## Decision

Rust owns every constructor and child-placement route. Python and WASM
are argument-mapping facades over the same inherent methods and the
same error codes.

Frozen names:

- `Agent::openai`, `Agent::anthropic`, `Agent::ollama` (lifted from
  Python in D1)
- `Agent::gateway`
- `Agent::e2b_sandbox`
- remote `AgentRun::prepare_child` / `start_child` when a route is
  configured (placement `remote_child_session`)

Implementation of HTTP, process, and E2B I/O lives in leaf crates
compiled only under `native-tokio`:

- `finstack-ai-provider-gateway` (existing)
- `finstack-ai-remote-child` at `extensions/interop/finstack-ai-remote-child`
- `finstack-ai-sandbox-e2b` at `extensions/toolsets/finstack-ai-sandbox-e2b`

The `wasm-host` inherent methods still exist. They return one stable
code, `agent_run_unsupported_plan`. Bindings must not special-case
“method missing.”

`FORBIDDEN_WASM` must include the gateway, remote-child, and E2B
crates (gateway is already listed).
`cargo tree -p finstack-ai --features wasm-host` stays free of them,
Temporal, Wasmtime, and `finstack-ai-server`.

Constructors never read ambient environment variables. Credential
rules match the leaves: explicit credential references, no plaintext
secrets in records, HTTPS for non-loopback endpoints.

`ChildRunPolicy` default stays Deny. No seventh port. No new
`RecordBody`. Shell stays T1. E2B is a new T4 leaf and is not
Landlock and is not isolated.

Zero-config `Agent::builder` / `from_python` continues to work with
no gateway, remote, or E2B configured.

## Consequences

- Both published bindings expose `openai`, `anthropic`, `ollama`,
  `gateway`, `e2b_sandbox`, and remote `start_child`.
- wasm-host callers receive `agent_run_unsupported_plan` from Rust.
- Default graphs do not grow Temporal, Wasmtime, or the server crate.
- Python-only factory bodies become thin wrappers after D1.

## Rejected alternatives

**Silent default-graph pull of Temporal, Wasmtime, or
`finstack-ai-server`.** Rejected: G-01 zero-config path must stay.

**Python-only or JS-only constructors.** Rejected: one Rust API;
bindings only map arguments.

**Omit the method on wasm-host.** Rejected: platform inability is a
Rust error, not a missing method.

**Read ambient env for API keys.** Rejected: same credential rules as
the leaves.

**Relabel shell as isolated or treat E2B as Landlock.** Rejected:
shell stays T1; E2B is T4 and is not isolated.

**Seventh port or new `RecordBody`.** Rejected: existing six ports and
journal kinds are sufficient.

## Compatibility and schema-change classification

No journal kind, kernel input, or schema change. Additive inherent
methods and leaf crates. Existing zero-config constructors stay.

## Security classification

- References: SEC-INV-005; TM-04 (secrets stay references; constructors
  do not read ambient env); TM-06 (E2B T4 is not isolated; shell stays
  T1); TM-09 (remote child uses PR-058 framing; no implicit discovery)
- Threat Model review trigger: complete TM-04 / TM-06 / TM-09 controls
  and tests with D2–D4. This record does **not** invent a review id.
- Residual: wasm-host fail-closed is a platform error, not a security
  boundary; native callers still supply explicit credentials.

## Affected requirements, design, and delivery

- Affected product: G-01 zero-config path (unchanged; planning file
  not edited)
- Affected Technical Design: SDK composition / six ports. Planning
  files are not edited.
- Implementation: D1–D4 constructor waves
- Unchanged: `ChildRunPolicy` default Deny; shell T1; no Temporal /
  Wasmtime / server on `wasm-host`

## Supersession metadata

None. This ADR supersedes no prior ADR. It does not change ADR-007
except to require the same inherent methods on both bindings.

## Reconsideration conditions

May change only through a new superseding ADR. Adding a seventh port,
a new `RecordBody`, ambient-env constructors, a Python-only factory,
default-graph Temporal/Wasmtime/server, or relabeling E2B as isolated
requires that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to execute D1–D4 locally
  only, without publication
- Implementation evidence: Partial (local D1 `e8caa09` / `df03258`,
  D2 `445c409` / `d4e7e0e`, D3 `295919b` / `bc2469f`, D4 `e246384` /
  `5448e37`; wasm-host fail-closed from Rust; no invented evidence or
  review id)
