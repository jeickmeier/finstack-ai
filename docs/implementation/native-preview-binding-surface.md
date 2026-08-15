# Native developer-preview binding surface

Status: tagged lockstep **`0.1.0`** public preview (`v0.1.0`). Python and
WASM halves closed at G4. Durability beta closed at G5. Plugin alpha
closed at G6. Public preview closed at G7 via
`G7-D-public-preview-f7c7e70b9e04`. crates.io / PyPI / npm publication
remains blocked on owner registry credentials.

The candidate Rust surface is:

- declarative `AgentSpec`, `AgentBuilder`, `BundleSpec`, `BundleCatalog`,
  exact `ResolvedAgentLock`, and `BundleResolver`;
- typed registry/component resolution and direct `ResolvedRunPlan`
  handles;
- native `Agent`, `NativeAgentBuilder`, `AgentRunRequest`,
  `AgentRunOutput`, and stable `AgentRunError` codes;
- provider-neutral Model/Toolset contracts through `finstack_ai::runtime`;
- first-party leaves: OpenAI-compatible and Anthropic providers,
  calculator / filesystem / shell toolsets, memory and SQLite stores,
  repository and memory context providers, compaction and verify
  middleware, log / metrics / OpenTelemetry observers, local and
  Temporal-shaped workflow adapters, and the loopback/Unix reference
  server.

Binding surfaces that share this lockstep version:

- Python wheel `finstack-ai==0.1.0` (experimental alpha; rust-backed and
  trusted callback paths);
- `@finstack/ai` `0.1.0` (experimental alpha; worker default;
  IndexedDB experimental / non-durable);
- experimental WIT package names `finstack:ai-*@0.0.4` with crate version
  `0.1.0`.

This is a developer-preview compatibility candidate, not a 1.0 stability
promise. An incompatible public change requires an ADR trigger assessment
and the repository compatibility-governance workflow before
implementation. Additive fixes remain allowed when they preserve
deterministic kernel semantics, the six-port boundary,
commit-before-effect ordering, and exact-lock behavior.

Excluded from this candidate are a seventh port, `@1.0.0` WIT worlds,
exactly-once delivery, IndexedDB durability, SharedArrayBuffer, a
marketplace, and automatic approval of side-effecting tools. See
[`preview-compatibility-policy.md`](preview-compatibility-policy.md).
