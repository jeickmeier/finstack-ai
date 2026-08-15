# Concept

`finstack-ai` is a deterministic agent microkernel. The kernel owns semantic
state, records, events, and effects. The runtime owns the six primary ports
and executes effects only after the matching request records commit. The SDK
owns composition. Leaf providers, tools, stores, observers, and workflow
drivers stay outward-facing.

## Six ports

Model, Toolset, ContextProvider, Middleware, JournalStore, and Observer.
There is no seventh port. Workflow engines are runtime drivers, not kernel
DAG owners.

## Commit-before-effect

The runtime commits durable intent before dispatching an external effect.
Recovery is at-least-once. Exactly-once is not claimed.

## G-01: no required workflow or plugin

A default kernel, runtime, and SDK graph does not depend on workflow crates,
Temporal, Restate, Wasmtime, or a remote server. Those are opt-in leaves.

## Trust

In-process native, Python, and JavaScript extensions inherit host authority.
They are not a sandbox. See [trust levels](security-trust-levels.md).

## Starter

[Rust minimal](../../examples/rust-minimal/README.md) is the smallest public
composition. Workspace version is **0.1.0 unpublished**.
