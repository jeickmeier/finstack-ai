# Owner walkthrough: Python rust-backed

Date: 2026-08-15
Owner: me@jeickmeier.com
Workspace: unpublished 0.0.4 on local checkout (not a clone of a public tag)

## Script

1. Repository already bootstrapped with `mise install`.
2. From repo root, run the documented construct-only path:

```bash
uv run --isolated --no-project --with-editable bindings/finstack-ai-python \
  python examples/python-minimal/rust-backed/main.py
```

## Result

- First successful offline run printed
  `starter.capability.research: Research financial statements`.
- No `--run`, so no provider request left the process.
- Friction: editable install compiles the PyO3 extension on first use.
  The no-compiler path is a staged wheel (`uv build` then `--with *.whl`),
  which is slower to produce but matches the published-user story.

This is an owner walkthrough. No external interviewee was recruited.
