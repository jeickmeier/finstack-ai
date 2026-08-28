# finstack-know CLI

The operator surface over the shared knowledge definition.

```text
finstack-know ask <question> [--session <id>] [--json] [--model <name>] [--data-dir <path>]
finstack-know ingest <path>  [--session <id>] [--json]
finstack-know sessions list | show <id> [--json] | name <id> <name>
finstack-know repl [--session <id>]
finstack-know docs [<topic>]
```

- `ask` without `--session` creates a session and prints its id; with
  `--session` it continues that session on lane `main`.
- Output modes: human text (streamed deltas, tool lines, final result)
  or `--json` — one NDJSON object per `RunEvent`, kind names verbatim.
  Both renderers consume the same event stream.
- `sessions list|show|name` inspect and label the shared sqlite journal;
  the same sessions open from the Python notebooks.
- `repl` is a readline loop on one session: Ctrl-C cancels the current
  run, elicitation requests are shown and answered inline, `:q` quits.
- `docs` prints these bundled topics — no provider or data dir needed.
- Credentials: pass `--api-key-env <VAR>`; the binary reads that
  variable and hands the value to the library, which never reads the
  environment itself.

Exit codes: 0 success, 1 run failure, 2 configuration or usage error.
