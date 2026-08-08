# finstack-ai

Project documentation is routed through [docs/README.md](docs/README.md).

- The [planning baseline](docs/planning/README.md) defines product scope, architecture, engineering rules, technical design, security obligations, sequencing, and acceptance criteria.
- The [implementation control set](docs/implementation/README.md) records delivery status, ownership, actual work, decisions, evidence, and exceptions while implementation is under way.

## Developer bootstrap

This repository uses [mise](https://mise.jdx.dev/) for pinned tool installs and checked-in tasks.

1. Install mise if needed: <https://mise.jdx.dev/getting-started.html>
2. From the repo root, install pinned tools: `mise install`
3. Verify the toolchain: `mise run doctor`

Tool versions and tasks live in [`mise.toml`](mise.toml). Prefer `mise run <task>` over ad-hoc wrappers. Build, test, lint, and architecture tasks are added as the corresponding Phase 0 work lands.
