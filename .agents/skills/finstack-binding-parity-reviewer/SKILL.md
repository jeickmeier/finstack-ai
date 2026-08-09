---
name: finstack-binding-parity-reviewer
description: Reviews finstack cross-language API parity across canonical Rust crates, PyO3 bindings, WASM bindings, Python stubs, package exports, JS facades, parity_contract.toml, tests, and examples. Use when touching finstack-quant-py, finstack-quant-wasm, public Rust APIs exposed through bindings, .pyi files, binding parity tests, or when the user mentions binding drift, Python/WASM parity, or Rust canonical API alignment.
---

# Binding Parity Reviewer

Use this skill to keep Rust as the canonical API while ensuring Python and WASM expose the intended same behavior.

## Routing

Use this instead of generic refactor or code review when the task touches:

- `finstack-quant-py/src/bindings/`
- `finstack-quant-wasm/src/api/`
- `finstack-quant-py/finstack_quant/**/*.pyi`
- Python `__init__.py` exports
- `finstack-quant-wasm/index.js` or generated TypeScript declarations
- `finstack-quant-py/parity_contract.toml`
- parity tests under `finstack-quant-py/tests/parity`

Use `finstack-quant-finance-review` first when the main risk is pricing or risk correctness. Use `finstack-refactor` first when the change is purely internal and no public binding surface changes.

## Core Rule

Rust owns domain behavior. Bindings perform type conversion, wrapper construction, registration, error mapping, docstring/text-signature exposure, and host-language ergonomic adapters only.

Logic in Python or WASM bindings is a parity bug unless it is strictly host-language ergonomics.

## Review Workflow

1. Identify the canonical Rust item: crate, module, type, function, error, serde shape, and tests.
2. Trace every exposed mirror:
   - PyO3 wrapper and module registration.
   - `.pyi` stub and Python package export.
   - WASM wrapper, `js_name`, JS facade, and TypeScript declaration if applicable.
   - Parity contract entry and parity tests.
   - Examples, notebooks, or docs that promise the public shape.
3. Check for logic drift:
   - financial calculations in bindings,
   - validation duplicated outside Rust,
   - inconsistent defaults,
   - error behavior that differs by host language,
   - missing or differently named methods.
4. Check naming and accessor conventions:
   - Rust/Python `snake_case`,
   - WASM `camelCase` with `#[wasm_bindgen(js_name = ...)]`,
   - `get_*` accessors where project conventions require them,
   - fully qualified metric keys.
5. Recommend the smallest sync slice and the narrowest verification commands.

## Severity

- **Blocker**: Python or WASM computes business logic differently from Rust; exposed API name or behavior diverges for a stable public surface.
- **Major**: Missing binding/stub/export/parity-contract entry for an intended public API; inconsistent error mapping; duplicate validation outside Rust.
- **Minor**: Documentation, text signature, or example drift; naming inconsistency that is confusing but not behavior-breaking.
- **Nit**: Local style issue that does not affect parity.

## Output Format

```markdown
## Binding Parity Findings

### Blocker
- `path::symbol`: issue, evidence, impact, and concrete sync fix.

### Major
- `path::symbol`: issue, evidence, impact, and concrete sync fix.

### Minor
- `path::symbol`: issue, evidence, impact, and concrete sync fix.

## Surfaces Checked
- Rust:
- PyO3:
- Python stubs/exports:
- WASM/JS/TS:
- Parity contract/tests:
- Examples/docs:

## Verification
- Targeted commands to run:
- Checks not run and why:
```

## Resources

- `references/parity-contract.md` - finstack parity contract and sync surfaces.
- `reference.md` - PyO3 wrapper and conversion patterns.
- `examples.md` - binding anti-patterns and fixes.
- `outputs/binding-drift-review.md` - example completed parity review.
