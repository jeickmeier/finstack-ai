# finstack-ai documentation

Use the documentation in two layers:

| Layer | Entry point | Purpose |
| --- | --- | --- |
| Planning baseline | [planning/README.md](planning/README.md) | Defines what will be built, why, under which constraints, in what order, and with what acceptance outcomes. |
| Implementation control | [implementation/README.md](implementation/README.md) | Records current delivery state, ownership, actual issues and pull requests, verification evidence, and time-bounded exceptions. |

The implementation layer does not restate or override the planning baseline. A status entry links to its governing planning material; any change to scope, architecture, compatibility, security obligations, or acceptance criteria must first pass the planning baseline's change-control process.

Accepted decisions currently remain canonical in [Architecture Specification section 25](planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary). The implementation ADR database indexes those decisions without replacing their text. Standalone ADR records are created and verified as part of logical PR-004.
