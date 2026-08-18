# PR-090 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-090 (draft W4 / PR-085).

## Purpose

Hold one SQLite connection, claim cron ticks with a compare-and-set
update, and rename `CronExpression` to `IntervalSchedule`.

## Principal changes

- `SqliteCronStore` opens the file, applies pragma/DDL once, and holds
  `Mutex<Connection>`.
- `CronScheduleStore::try_claim` defaults to `StoreUnavailable`.
  Memory and SQLite implement CAS. SQLite uses `BEGIN IMMEDIATE`.
- `next_after` uses `checked_mul`.
- `CronExpression` renamed to `IntervalSchedule`.

## Acceptance mapping

- PR-090-A01: Two SQLite stores on one file claim exactly one fire.

## Explicit exclusions

Temporal engine. Gateway deletion.

## Compatibility class

Pre-1.0 public rename.

## Dependencies

PR-085.
