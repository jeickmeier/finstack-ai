# Owner walkthrough: Rust minimal

Date: 2026-08-15
Owner: me@jeickmeier.com
Workspace: unpublished 0.0.4 on local checkout (not a clone of a public tag)

## Script

1. Repository already bootstrapped with `mise install`.
2. From repo root, run the documented quick start:

```bash
cargo run -p finstack-ai-native-examples --bin minimal --offline --locked
```

## Result

- First successful offline run printed `minimal ready`.
- No credential prompt. No network. Loopback provider only.
- Friction: first compile of the example crate takes several seconds; later
  runs are sub-second. The README now states `--offline --locked`.

This is an owner walkthrough. No external interviewee was recruited.
