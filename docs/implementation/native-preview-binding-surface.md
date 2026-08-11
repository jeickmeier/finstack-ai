# Native developer-preview binding surface

Status: candidate at `0.0.1-dev` (PR-026)

The candidate Rust surface is:

- declarative `AgentSpec`, `AgentBuilder`, `BundleSpec`, `BundleCatalog`, exact
  `ResolvedAgentLock`, and `BundleResolver`;
- typed registry/component resolution and direct `ResolvedRunPlan` handles;
- native `Agent`, `NativeAgentBuilder`, `AgentRunRequest`, `AgentRunOutput`, and
  stable `AgentRunError` codes;
- provider-neutral Model/Toolset contracts through `finstack_ai::runtime`;
- canonical `finstack-ai-provider-openai-compatible`,
  `finstack-ai-tools-calculator`, `finstack-ai-tools-filesystem`, and
  `finstack-ai-store-memory` packages.

This is a developer-preview compatibility candidate, not a 1.0 stability
promise. An incompatible public change requires an ADR trigger assessment and
the repository compatibility-governance workflow before implementation.
Additive fixes remain allowed when they preserve deterministic kernel semantics,
the six-port boundary, commit-before-effect ordering, and exact-lock behavior.

Excluded from this candidate are Python/WASM runtime parity, a durable database
store, remote/plugin ABI guarantees, Model-activated capabilities, and automatic
approval of side-effecting tools.
