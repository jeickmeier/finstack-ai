---
trigger: glob
description: Enforce TypeScript/JavaScript binding quality, IDE typing, and TSDoc rules for finstack-ai.
globs: "**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs,d.ts}"
---

# TypeScript coding

Apply with `01-engineering-conformance.md` and `02-testing-and-delivery.md`. The binding is host-driven; WASM/Rust retains authoritative state.

## Production-safe TypeScript

- Follow the repository TypeScript, formatter, linter, and package policy once configured. Ship idiomatic typed ESM (and declarations) that mirror the Rust-owned surface; do not reimplement kernel or runtime semantics in JavaScript/TypeScript.
- Prefer `async`/`await`, `Promise`, and `AsyncIterable` for host effects and event streams. Propagate `AbortSignal` (or equivalent cancellation) through adapters; dropping a run object must not silently cancel a durable run.
- Keep the binding host-driven: JavaScript fulfills bounded host effects. Transfer IDs as strings and dynamic JSON as strings or `Uint8Array` per API; use blob handles for large content; batch events.
- Use strict typing. Avoid `any` on public surfaces; prefer discriminated unions, `unknown` with narrowing, and exhaustive switches (`never` in the default case) for public enums and tagged unions.
- Map errors from the same framework error codes used by Rust/Python. Preserve retryability and safe details; never embed provider credentials or secrets in browser examples, adapters, or shipped packages.
- Keep npm packages tree-shakeable where practical. Isolate generated WASM/bindgen output from hand-authored TypeScript and regenerate from the documented command only; fail on dirty generated output.
- Browser builds must stay free of Tokio, native sockets, filesystem assumptions, and provider credentials. Prefer Web Worker topology for high-volume work; shared-memory threading stays opt-in unless an ADR changes that.

## IntelliSense and IDE docs

Treat editor completion, hover docs, go-to-definition, and signature help as part of the public TypeScript API. A binding change that breaks IDE discoverability is incomplete.

- Publish a typed package surface: set accurate `types` / `exports` (and `types` conditions) in `package.json` so consumers resolve declarations without extra `@types` packages.
- Do not re-export raw wasm-bindgen or untyped glue as the public API. Provide hand-authored or generated-and-reviewed `.d.ts` / typed wrappers that name parameters, discriminate events/errors, and hide internal handles.
- Keep declaration signatures identical to runtime: parameter names, optionality, overloads, async forms, async iterables, and abort/cancellation arguments. Avoid public `any`, broad `Function`, or index-signature bags for primary APIs.
- Put TSDoc on the symbols IDEs resolve—public exports and declaration files—so hover shows summary, `@param`, `@returns`, `@throws`, and `@example` for primary workflows.
- Export a stable public surface through package entrypoints. Tree-shakeable subpath exports (for example adapters) need their own types and docs, not only a root barrel.
- Validate IDE contracts in CI once tooling exists: `tsc --noEmit` (or the repo task) against the published surface, and fail when declarations drift from implementation. Smoke-check that the IDE hover shows docs for new public APIs before handoff.

## TSDoc

Public and compatibility-controlled APIs require documentation that states purpose, failure semantics, and a tested example where the surface is user-facing. Private helpers need docs only when behavior is non-obvious. Keep comments current with code; delete stale commentary.

- Use TSDoc/JSDoc on public exports: description, `@param`, `@returns`, `@throws`, and `@example` for primary workflows. Document cancellation (`AbortSignal`), async-iteration, and resource-lifetime expectations.
- Mark experimental or candidate surfaces explicitly. Document browser versus worker constraints, transferable payload choices, and that credentials must use a trusted proxy rather than embedded keys.
- Keep declarations and docs synchronized with the Rust semantic surface; when behavior is binding-specific conversion only, say so and point to the shared contract.
