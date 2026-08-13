# finstack-ai-test

`finstack-ai-test` is the public test-support crate for `finstack-ai` extension
authors. It provides deterministic fixtures and conformance functions without
requiring registry, runtime-driver, or kernel internals.

The six primary-port entry points are:

- `check_model_conformance`
- `check_toolset_conformance`
- `check_context_conformance`
- `check_middleware_conformance`
- `check_observer_conformance`
- `check_journal_store_conformance`

Each failure is a `PortConformanceFailure` containing the port name, a stable
contract label, and a safe detail message suitable for CI output.

Copyable examples live in the leaf fixtures:

- [`fixtures/ci/model-port-leaf`](../../fixtures/ci/model-port-leaf) implements
  a provider using only the public `Model` ABI and passes
  `check_model_conformance`.
- [`fixtures/ci/toolset-port-leaf`](../../fixtures/ci/toolset-port-leaf)
  implements a toolset using only the public `Toolset` ABI and passes
  `check_toolset_conformance`.

The crate also exports scripted model, toolset, context, middleware, observer,
and journal-store fixtures; deterministic clocks and ID sources; golden-trace
drivers; and `check_compaction_conformance` for canonical-history, protected
item, tool-pair, checkpoint, hard-budget, and shared-projection invariants.

Run the repository acceptance task with:

```text
mise run conformance
```
