# finstack-ai documentation pack v0.8

This package contains the implementation-oriented design set for **finstack-ai**, a Rust-based agent microkernel with first-class Rust, Python, and WebAssembly interfaces.

## Documents

1. **[Engineering Standards](00-finstack-ai-engineering-standards.md)** — normative repository-wide code, dependency, testing, compatibility, documentation, and review rules.
2. **[Product Requirements](01-finstack-ai-product-requirements.md)** — product goals, users, scope, requirements, releases, and acceptance criteria.
3. **[Architecture Specification](02-finstack-ai-architecture-specification.md)** — microkernel boundary, extension ports, composition, durability, bindings, isolation, and deployment architecture.
4. **[Technical Design](03-finstack-ai-technical-design.md)** — Rust workspace, types, reducer/effect engine, runtime, extension traits, storage, bindings, tests, and benchmarks.
5. **[Implementation Plan](04-finstack-ai-implementation-plan.md)** — phased delivery program and pull-request sequence.
6. **[Security and Threat Model](06-finstack-ai-security-threat-model.md)** — assets, trust boundaries, threats, controls, residual risks, and gate-specific security evidence.
7. **[Future Capabilities Design Validation](05-finstack-ai-future-capabilities-design-validation.md)** — supporting architectural stress test and composition rationale for subagents, delegated work, memory, RAG, context compaction, human-in-the-loop workflows, background jobs, multi-agent teams, sandbox/computer use, multi-channel operation, and guardrails/evaluations.

## Authority and change control

The documentation set has one authority chain:

1. The **PRD** owns product requirements, scope, non-goals, and acceptance outcomes.
2. Accepted **ADRs** own architectural and technical decisions and their rationale. An ADR supersedes conflicting older prose, but the same change must reconcile every affected primary document.
3. The **Architecture Specification** owns system boundaries, invariants, trust boundaries, and component responsibilities within the PRD.
4. The **Engineering Standards** own repository-wide implementation, dependency, quality, compatibility, documentation, and review rules within the preceding authorities.
5. The **Technical Design** owns the concrete implementation baseline within the PRD, accepted ADRs, Architecture Specification, and Engineering Standards.
6. The **Security and Threat Model** owns threat assumptions, control obligations, residual risks, and security evidence traceability within the preceding authorities. Concrete mechanisms remain in the Architecture Specification and Technical Design.
7. The **Implementation Plan** owns sequencing, dependencies, and delivery gates. It cannot silently override a requirement, standard, control obligation, or design decision.
8. The **Future Capabilities Design Validation** is supporting analysis and rationale. All accepted changes are incorporated into the authoritative documents; this supporting document cannot add, alter, or reopen a normative decision.

This README is a routing and authority document, not an additional source of product or technical requirements. A conflict between primary documents blocks the affected implementation work until it is reconciled. Changes to public semantics, records, protocols, extension boundaries, or support policy must update document versions, traceability, ADR status, and implementation gates in the same review unit.

## Version note

Version 0.8 is the reconciled pre-implementation baseline. It closes dependency/feature direction, canonical data and digest rules, durable command/recovery contracts, authorization propagation, bundle and child-run composition, middleware/compaction recovery, WIT/release versioning, and public-preview scope; it also removes pending guidance sections and advances every controlled document version. Version 0.7 assigned model-context compaction to `before_model` middleware and defined its normalized outcome/checkpoint, immutability, safety, delivery, threat-control, and conformance rules. Version 0.6 added the normative Engineering Standards and the pre-implementation Security and Threat Model. Version 0.5 incorporated generic deferred effects, run lineage, generalized interactions, and `before_finalize`. Documentation pack v0.4 resolved the ten foundational product decisions that were previously open.
