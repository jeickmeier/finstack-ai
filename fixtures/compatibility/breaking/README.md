# 1.0 breaking-change fixtures

Negative corpora for PR-062-A01. These paths sit outside the
`public-rust-api/v1` and `journal/v1` discoverers so historical counts
stay stable.

| Family | Negative |
| --- | --- |
| Public Rust / Python / JS | `valid--v0.1.0-*.txt` baselines plus `invalid--renamed-*.txt` |
| Journal | `../journal/breaking/invalid--unknown-state-field.json` |
| Runtime events | `../runtime-events/v1/` |
| Remote / process | `../remote/v1/handshake/invalid--unknown-field.json` and process peer |
| WIT | `../wit/v1.0.0/` |
