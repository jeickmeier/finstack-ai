---
name: finstack-documentation-maintainer
description: Maintains finstack documentation across API docs, README/spec/changelog cleanup, stale reference audits, generated docs, examples, notebooks, and financial/math references. Use when adding or reviewing documentation, cleaning AI/process language, validating code references, completing public API docs, or checking docs against current source and tests.
---

# Documentation Maintainer

Use this skill to make documentation accurate, concise, source-backed, and useful to maintainers or API users.

## Modes

### API Documentation

Use when public Rust/Python/WASM APIs lack descriptions, arguments, returns, examples, error behavior, or references. Public financial/math APIs should cite canonical sources through `docs/REFERENCES.md` when applicable.

Python `.pyi` stubs are the primary IDE-facing surface (hover, signature help, mypy), and Python users cannot see the Rust source. Give every public binding a detailed stub docstring — summary plus documented parameters, return value, raised exceptions, and behavioral notes (units, conventions, missing-data handling, supported `op`/`method` strings) — not a one-line summary, even for thin wrappers that delegate to Rust. Match the docstring flavor (NumPy or Google) already used in the module.

The same bar applies to pure-Python binding modules (`.py` files such as pandas convenience layers): they have no separate stub, so their own function and class docstrings are the only IDE help and must document parameters, returns, raises, and behavioral notes in full.

### Cleanup And Stale-Reference Audit

Use when docs contain stale paths, outdated commands, AI/process/meta language, duplicated explanations, or unverified claims.

### Generated Or Derived Docs

Use when the documented surface is generated from contracts, bindings, stubs, or notebooks. Prefer fixing the source generator or contract over hand-editing generated output.

### Examples And Notebooks

Use when examples, snippets, notebooks, or command docs need validation against current code.

## Core Standard

Good finstack docs are:

- accurate to source, tests, parity contracts, generated bindings, and examples,
- concise enough that every paragraph earns its place,
- explicit about behavior, constraints, inputs, outputs, errors, and invariants,
- free of process commentary, AI phrasing, and implementation-history chatter unless the document is a changelog, plan, or migration guide,
- clear about financial conventions, formulas, units, and sources.

## Workflow

1. Identify the target documents and audience.
2. Inventory every referenced symbol, file, command, config key, test, example, and citation.
3. Verify references against current source before rewriting.
4. Choose the mode: API docs, cleanup, generated docs, examples, or mixed.
5. Rewrite in present tense around current behavior.
6. Remove or narrow claims that cannot be verified.
7. Run the smallest relevant docs, lint, build, parity, doctest, notebook, or targeted unit check.
8. Report what changed, what was checked, and what remains unverified.

## Remove Or Rewrite

- "This document explains..."
- "We added..."
- "The implementation now..."
- "As part of this change..."
- broad filler such as "comprehensive", "robust", "seamless", or "powerful" unless technically precise,
- stale status language such as "currently being implemented" unless the document is an active plan,
- implementation details that do not change reader behavior.

## Output Format

```markdown
## Documentation Result

### Scope
<docs edited or reviewed and intended audience>

### Changes
- <accuracy or coverage improvement>
- <cleanup or stale-reference fix>

### References Checked
- <source/test/command/config/example checked>

### Verification
- `<command>`: pass/fail/not run

### Residual Risk
<claims, examples, or generated surfaces not checked>
```

## Resources

- `references/api-documentation.md` - API doc coverage standards.
- `examples/api-documentation.md` - API documentation examples.
- `references/finstack-doc-surfaces.md` - repo-specific doc surfaces and verification hints.
- `outputs/documentation-report.md` - example completed documentation report.
