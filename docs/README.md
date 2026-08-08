# finstack-ai documentation pack v0.3

This package contains the implementation-oriented design set for **finstack-ai**, a Rust-based agent microkernel with first-class Rust, Python, and WebAssembly interfaces.

## Documents

1. **Product Requirements** — product goals, users, scope, requirements, releases, and acceptance criteria.
2. **Architecture Specification** — microkernel boundary, extension ports, composition, durability, bindings, isolation, and deployment architecture.
3. **Technical Design** — Rust workspace, types, reducer/effect engine, runtime, extension traits, storage, bindings, tests, and benchmarks.
4. **Implementation Plan** — phased delivery program and pull-request sequence.
5. **Future Capabilities Design Validation** — architectural stress test for subagents, delegated work, memory, RAG, human-in-the-loop workflows, background jobs, multi-agent teams, sandbox/computer use, multi-channel operation, and guardrails/evaluations; includes the smallest recommended microkernel refinements.

## Version note

Version 0.3 adds the Future Capabilities Design Validation document. Its central recommendation is to preserve the existing six extension ports and make only narrowly scoped additions for generic deferred effects, run lineage, generalized typed interactions, and a pre-terminal `before_finalize` middleware checkpoint.
