# Mise Format Task Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `mise run format` actively format all Rust and Python sources
currently present in the repository.

**Architecture:** Keep the root `mise.toml` task as a direct, sequential
aggregate. Cargo formats the Rust workspace, then pinned Ruff formats the
repository's Python sources; TypeScript remains deferred until its package and
formatter configuration exist.

**Tech Stack:** mise, Cargo/rustfmt, uv, Ruff 0.12.11, Bash

## Global Constraints

- Use `uv run --no-project` for Python tooling.
- Keep Ruff pinned at `0.12.11`, matching the existing formatting checks.
- Do not add Node, TypeScript, or TypeScript formatter infrastructure before
  TypeScript sources and package configuration exist.
- Preserve the specialized Python `format-*` tasks as check-only tasks.
- Do not commit unless the user explicitly requests a commit.

---

### Task 1: Expand the aggregate format task

**Files:**
- Modify: `mise.toml:69-71`
- Test: temporary `tools/format_probe.py` removed before handoff

**Interfaces:**
- Consumes: repository-root `mise run format`
- Produces: an aggregate task that formats Rust with rustfmt and Python with
  Ruff

- [ ] **Step 1: Prove the current task does not format Python**

Run:

```bash
printf 'value={  "answer":42}\n' > tools/format_probe.py
mise run format
uv run --no-project python -c 'from pathlib import Path; assert Path("tools/format_probe.py").read_text() == "value = {\"answer\": 42}\n"'
```

Expected: the assertion fails because the current task only runs rustfmt.

- [ ] **Step 2: Remove the baseline probe**

Run:

```bash
rm tools/format_probe.py
```

Expected: `tools/format_probe.py` no longer exists.

- [ ] **Step 3: Implement the direct aggregate**

Replace the existing task with:

```toml
[tasks.format]
description = "Format Rust and Python sources"
run = """
set -euo pipefail
cargo fmt --all
uv run --no-project --with ruff==0.12.11 ruff format .
"""
```

- [ ] **Step 4: Prove the updated task formats Python**

Run:

```bash
printf 'value={  "answer":42}\n' > tools/format_probe.py
mise run format
uv run --no-project python -c 'from pathlib import Path; assert Path("tools/format_probe.py").read_text() == "value = {\"answer\": 42}\n"'
rm tools/format_probe.py
```

Expected: `mise run format` succeeds, the assertion passes, and the probe is
removed.

- [ ] **Step 5: Validate the touched configuration and formatting checks**

Run:

```bash
mise run format-benchmark
mise run format-architecture
mise run format-ci
mise run format-schema-governance
git diff --check
```

Expected: every command exits successfully with no formatting or whitespace
errors.

- [ ] **Step 6: Review the final diff**

Run:

```bash
git diff -- mise.toml docs/superpowers/specs/2026-08-09-mise-format-design.md docs/superpowers/plans/2026-08-09-mise-format.md
git status --short
```

Expected: the intended task, design, and plan changes are present;
`tools/format_probe.py` is absent; no unrelated files were created.
