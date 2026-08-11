# PR-022 local delivery evidence

PR-022 implements strict declarative agent and capability specifications,
finite bundle/catalog resolution, exact credential-free locks, typed committed
result decoding, and direct non-kernel child, budget, and artifact services.
The immutable implementation candidate is commit
`8b98abfa674086da00893dbcd56b92e2d7f0f526` with tree
`994dfc467dbf7a9b9efc8d998a31ecf675eb92c6` on
`codex/pr-022-agent-spec`.

The exact local command inventory is in
[`candidate-validation.txt`](candidate-validation.txt), and the security and
threat-model disposition is in [`security-review.txt`](security-review.txt).
Local integration remains pending.

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | Builder serialization is parsed through the strict JSON ingress and produces the identical domain-separated canonical fingerprint | Passed on candidate |
| A02 | `CapabilityRef` is the sole agent capability-reference shape; every referenced definition resolves from an exact installed bundle and is emitted in the lock | Passed on candidate |
| A03 | Strict fixtures and negative resolver tests reject unknown fields, schema/version mismatch, duplicate capability IDs, unresolved refs, and incompatible component versions before start | Passed on candidate |
| A04 | Minimal model/store builder creates an empty capability list; optional declarative store is required only when resolving an executable agent | Passed on candidate |
| A05 | Capability/spec serialization contains only identifiers, references, instructions, activation metadata, and configuration data; executable handles remain in resolved runtime plans | Passed on candidate |
| A06 | Finite bundle requirements/conflicts and unresolved agent/capability references fail during resolution; child placement is validated before preparation | Passed on candidate |
| A07 | All compatible-lane, isolated-session, and remote-session handshakes commit one exact locator, attach equal retries, and durably reject conflicting request digests without lookup by run ID | Passed on candidate |
| A08 | Canonical lock export/import reconstructs exact fingerprints; tampered engine, bundle/component version, schema, configuration, capability, and service selections fail closed; secret canaries are rejected | Passed on candidate |
| A09 | `RunResult::decode` uses the committed `FinalResultRecorded` bytes and exact `SchemaRef`, with stable schema, type, and nested-path errors and no rerun/mutation | Passed on candidate |
| A10 | Bundle construction requires direct budget/artifact handles only when declared; missing required services and missing executable stores fail before start | Passed on candidate |
| A11 | Ambiguous reserve acknowledgement reconciles to one receipt, child invocation follows settlement, committed usage charges once, terminal release occurs once, and equal retries reuse durable receipts | Passed on candidate |
| A12 | Artifact staging precedes the caller's journal append, scope/content/metadata are exact, oversized input rejects without truncation, failed-append orphans are collected, pinned references survive, and missing/corrupt/cross-scope reads fail | Passed on candidate |

The implementation keeps the deterministic kernel I/O-free and retains the six
primary ports. `AgentInvoker`, `BudgetLedger`, and `ArtifactStore` are direct
runtime service contracts; the kernel owns only durable identifiers, records,
relations, and replay validation. `Model` capability activation remains
reserved. No remote registry, UI, publication, hosted run, or independent
review is claimed.
