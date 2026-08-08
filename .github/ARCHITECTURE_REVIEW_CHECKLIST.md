# Architecture review checklist

Use this checklist with the pull request template. A change is consistent with the
[Architecture Specification](../docs/planning/02-finstack-ai-architecture-specification.md)
only when the following questions are answered satisfactorily.

Source: Architecture Specification §27.

1. Does it change universal agent semantics or merely add policy/integration?
2. Can it be implemented through an existing port?
3. Does it introduce external I/O into the kernel?
4. Does it add a language crossing to the hot path?
5. Does it require durable output, and if so, what record captures it?
6. Can recovery determine whether its effects completed?
7. Does it preserve the same behavior in native Rust and WASM?
8. Does it require a new public middleware stage?
9. Does it create an unbounded queue or large payload copy?
10. Can the minimal bundle still omit it completely?
11. If it changes model-visible context, is it explicit `before_model` middleware that preserves canonical history, protected content, provenance, and replay evidence?

## Mechanical gates for this repository

- [ ] `mise run architecture` passes locally and in hosted CI (`.github/workflows/ci.yml`)
- [ ] Dependency direction preserved: `kernel <- runtime <- SDK/bindings`; protocol stays outward
- [ ] No forbidden kernel dependency (direct or transitive)
- [ ] Runtime defaults remain empty; facade `native-tokio` / `wasm-host` pass-through only
- [ ] Browser WASM depends on the facade with defaults disabled and only `wasm-host`
- [ ] Exceptions, if any, have an ADR id, exceptions-register row, and allowlist entry
