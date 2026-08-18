# PR-097 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-097 (draft W11 / PR-092).

## Purpose

Keep `finstack-ai-middleware-verify` but stop shipping it as an
example battery.

## Principal changes

- Manifest is `publish = false`.
- README, crate rustdoc, and site shipping-leaves table call it a
  fixture.

## Acceptance mapping

- PR-097-A01: manifest is `publish = false`; site docs call it a
  fixture.
- PR-097-A02: `examples/rust-minimal` and the runtime middleware-driver
  reference still compile.

## Explicit exclusions

Deleting the crate. Changing `StageOutcome` semantics.

## Dependencies

PR-093 (InteractionId fix).
