# PR-066 RC soak note

Date: 2026-08-15
Owner: me@jeickmeier.com
Task: `PR-066-T-soak-d471877bcf40`

This note binds soak evidence for PR-066 principal change 1 and
Implementation Plan §21 item 12. It does not record `G8-D-*`.

## Result

**Gap.** No named external adopter project has validated the
`0.1.0` → `1.0.0` migration path.

## Search (2026-08-15)

Reused the Phase 9 entrance honesty bar in
[`preview-feedback-review.md`](../../preview-feedback-review.md).
Checked again for out-of-tree `0.1.0` or `1.0.0` use:

- `jeickmeier/finstack-ai` GitHub issues: none from external adopters
- crates.io / PyPI / npm: unpublished (no registry credentials)
- Hosted PRs `#1`–`#9`: maintainer-only
- Sibling repos under `jeickmeier` and local `/Users/jeickmeier/Projects/*`:
  no `finstack-ai`, `finstack_ai`, or `@finstack/ai` dependency outside
  this workspace

In-tree starters (`examples/rust-minimal`, `examples/python-minimal/*`,
`examples/durable-interaction`, `examples/browser-minimal`,
`examples/ts-alpha-install`, plugin templates) are first-party. They
do not satisfy “external adopters.”

## RC identity

The local unpublished `1.0.0` candidate is this branch
(`codex/pr-066-1.0.0-ga`) from baseline
`04962c743d2f03d59b74feb9c71f872cccfcc003`. Staged checksums, when
recorded, live beside this file. They are not a published RC.

## Soak window

No external soak window was observed. Do not invent a 30-day
requirement or named adopters.

## Unreviewed public API change

No unreviewed public API change landed during a soak, because no
external soak ran. The public-API change backlog has no
`change before preview` row opened after G7. Compatibility fixtures
and `COMP-1.0-D-contract-freeze-00b78667ecc4` remain the freeze
record.

## Disposition

Do not claim PR-066-A01 Passed. Do not claim §21 item 12. Do not
write `G8-D-*`. Prepare the candidate and stop before A01/A04.
