# finstack-ai documentation

Use the documentation in two layers, plus a public guide index:

| Layer | Entry point | Purpose |
| --- | --- | --- |
| Public guides | [site/README.md](site/README.md) | Install, language quick starts, security, support, FAQ. |
| Planning baseline | [planning/README.md](planning/README.md) | Product scope, architecture, engineering rules, security obligations, sequencing, and acceptance criteria. Read-only during normal coding. |
| Implementation control | [implementation/README.md](implementation/README.md) | Delivery state, ownership, evidence, ADRs, and exceptions. |

Public RFCs live under [rfcs/README.md](rfcs/README.md). License, DCO,
conduct, maintainers, and security policy are linked from the
[repository README](../README.md), [CONTRIBUTING.md](../CONTRIBUTING.md),
[CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md), and
[GOVERNANCE.md](../GOVERNANCE.md).

The implementation layer does not restate or override the planning
baseline. A status entry links to its governing planning material; any
change to scope, architecture, compatibility, security obligations, or
acceptance criteria must first pass the planning baseline's change-control
process.

Accepted decisions currently remain canonical in
[Architecture Specification section 25](planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary).
The implementation ADR database indexes those decisions without replacing
their text. Standalone ADR records live under
[implementation/adrs/](implementation/adrs/) and the
[ADR register](implementation/adr-register.md).
