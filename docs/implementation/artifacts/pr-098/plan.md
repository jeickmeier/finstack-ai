# PR-098 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-098 (draft W12 / PR-093).

## Purpose

Reconcile extension READMEs, site/provider docs, and delivery registers
for facts this phase created. Do not invent evidence ids.

## Principal changes

- Toolset README lists e2b and skill-import. Stores README exists.
- Phase 11 ledger notes dedicated crates on `main` without marking
  PR-068–PR-073 `Done`. Phase 14 rows record local uncommitted work.
- Site/provider docs: no `openai_chat` product surface; `Agent.gateway`
  dispatches; `subagent_status` name. ADR-045 constructor list no
  longer names a gateway crate.

## Acceptance mapping

- PR-098-A01: toolset and store READMEs list the in-tree crates.
- PR-098-A02: ledger status text matches `main` without manufactured
  evidence ids.
- PR-098-A03: site docs no longer advertise `openai_chat` or
  `subagent_await`.

## Explicit exclusions

Inventing evidence ids. Folding this work into open PR-067. Marking
PR-068–073 `Done` without candidates.

## Dependencies

PR-094, PR-095, PR-096, PR-097.
