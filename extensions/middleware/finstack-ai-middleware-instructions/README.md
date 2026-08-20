# finstack-ai-middleware-instructions

Injects tenant/policy instructions (compliance footer, as-of date, locale,
tenant rules) at the `prepare_context` stage via `AddInstructions`. Pure and
deterministic: all text is frozen into the configuration at construction.
Injected items land as protected System messages that the context compactor
must preserve. Design: `docs/superpowers/specs/2026-08-20-policy-instructions-design.md`.
