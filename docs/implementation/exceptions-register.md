# Exceptions register

This register implements the waiver process in [Engineering Standards section 14](../planning/00-finstack-ai-engineering-standards.md#14-exceptions-and-waivers). No exception is currently requested, approved, expired, or open.

## Scope and limits

An exception is a time-bounded deviation from a specific Engineering Standards rule. It cannot silently change a product requirement, architecture decision, technical design, security obligation, compatibility promise, phase, gate, or acceptance outcome. Those changes require the applicable ADR and versioned planning reconciliation.

An exception cannot authorize a new primary port, external I/O in the kernel, silent data loss, unbounded resource use, unauthenticated privileged input, or an undocumented public compatibility break. Such a request must be rejected or handled through the stronger change-control path identified by the Engineering Standards.

## Status values

| Status | Meaning |
| --- | --- |
| `Requested` | Complete request awaiting the approval decision required by current repository governance. |
| `Approved` | Approver accepted the bounded risk, compensating control, owner, and expiry. |
| `Rejected` | Approval was denied; affected work remains non-compliant. |
| `Expired` | Expiry date or gate was reached before verified closure. |
| `Closed` | The deviation was removed and closure evidence was verified. |

Only `Approved` and unexpired exceptions can support a `Waived` acceptance disposition. An expired exception blocks the affected logical PR, phase, or gate until it is closed or replaced through a new review. Exceptions must be closed before G8.

## Current exception index

Exception IDs use `EX-short-slug-xxxxxxxxxxxx`, where the final 12 lowercase hexadecimal characters are generated randomly when the request is created. This prevents parallel branches from claiming the same counter value. IDs are immutable and never reused.

| Exception | Status | Exact rule | Affected scope / PR | Reason compliance is impractical | Risk | Compensating control | Owner | Approver | Created | Expiry date / gate | Removal issue / task | Public compatibility affected | Security affected | ADR | Architecture allowlist id | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

<!-- Example shape only; remove this comment when adding the first real row.
| EX-short-slug-xxxxxxxxxxxx | Requested | document section and normative sentence | PR-NNN and exact surface | bounded factual reason | concrete impact | enforceable temporary control | person/team | reviewer/approver | YYYY-MM-DD | date or GN | issue or scoped task ID | Yes/No plus detail | Yes/No plus control IDs | ADR-NNN or — | ALW-id or — | scoped evidence ID or — |
-->

Every field is required before approval. `No` in either impact field still requires an explicit determination; `—` is not valid for an approved exception.

Architecture and dependency suppressions are enforced by the surviving graph checks, not a retired `architecture` task or `tools/architecture/allowlist.toml`. A waiver additionally requires:

- an accepted ADR identifier in the `ADR` column;
- a matching entry in [`deny.toml`](../../deny.toml) (`exceptions = []` today; cargo-deny six target triples) whose exception identity equals this register's Exception ID; and
- a non-waivable rejection for kernel I/O / forbidden kernel dependencies and six-port contract ownership, enforced by [`scripts/wasm_package/check.py`](../../scripts/wasm_package/check.py) `check_graph()` plus the four native graph tests in `crates/finstack-ai-test/tests/journal_v1.rs`, `extensions/stores/finstack-ai-store-sqlite/tests/faults.rs`, `plugins/finstack-ai-plugin-host/src/lib.rs`, and `plugins/finstack-ai-guest-sdk/src/lib.rs`.

## Exception event log

Record every request, approval, rejection, extension request, expiry, and closure as an append-only event. Editing the current index does not replace this history.

| Date | Exception | Event | Actor | Decision or change | Evidence | Follow-up |
| --- | --- | --- | --- | --- | --- | --- |

<!-- Example shape only; remove this comment when adding the first real row.
| YYYY-MM-DD | EX-short-slug-xxxxxxxxxxxx | Requested/Approved/Rejected/Expired/Closed | person | concise decision | scoped evidence ID or review link | issue or scoped task ID or — |
-->

## Lifecycle

1. The requester creates a complete `Requested` row and links the affected delivery work.
2. The reviewer or approver required by current repository governance reviews the rule, scope, risk, compensating control, compatibility and security impact, and removal plan.
3. Approval is recorded in the event log and supported by evidence. The affected acceptance row may then use `Waived` while the exception remains valid.
4. The owner tracks the removal task and compensating-control evidence through the delivery and evidence registers.
5. At expiry, the exception becomes `Expired` unless closure evidence has already been verified.
6. Closure records proof that the implementation conforms, updates affected acceptance rows, and removes the waiver from completion calculations without deleting history.

An extension is a new approval decision with a revised risk assessment and expiry; it is never an unreviewed date edit.
