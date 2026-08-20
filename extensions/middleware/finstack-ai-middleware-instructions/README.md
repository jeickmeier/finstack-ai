# finstack-ai-middleware-instructions

Injects tenant/policy instructions (compliance footer, as-of date, locale,
tenant rules) at the `prepare_context` stage via `AddInstructions`. Pure and
deterministic: all text is frozen into the configuration at construction.
Injected items land as protected System messages inserted **before** the
trailing current-user message; the context compactor must preserve them
byte-identically, and the current user message remains last and protected, so
compaction still lands. The component id is fixed
(`finstack.middleware.instructions`), so register at most one instance per
chain and use multiple entries for multiple policies.
Design: `docs/superpowers/specs/2026-08-20-policy-instructions-design.md`.
