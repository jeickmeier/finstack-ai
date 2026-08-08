# ADR record directory

This is the canonical location for standalone ADR records. It intentionally contains no ADR records at initialization: accepted ADR-001 through ADR-037 are indexed in the [ADR database](../adr-register.md), while creating and verifying their standalone records remains delivery work under logical PR-004.

Files use `ADR-NNN-short-topic.md`. The ADR database must link each file before its record state changes from `Indexed` to `Standalone`; the file must also satisfy the standalone-record requirements in that database.

Do not create a second decision by copying planning prose here. A standalone record captures the stable decision context, consequences, change-control metadata, security and compatibility classification, and links to the canonical planning sections. New or superseding decisions follow the planning authority chain and receive the next approved ADR number.
