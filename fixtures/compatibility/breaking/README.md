# 1.0 breaking-change fixtures

Negative corpora for source and wire compatibility changes. These paths sit outside the
`public-rust-api/v1` and `journal/v1` discoverers so historical counts
stay stable.

| Family | Negative |
| --- | --- |
| Public Rust / Python / JS | `valid--v0.1.0-*.txt` baselines plus `invalid--renamed-*.txt`; Rust also has `invalid--added-field.txt` and `invalid--return-type-change.txt` |
| Journal | `../journal/breaking/invalid--unknown-state-field.json` |
| Runtime events | `../runtime-events/v1/` |
| Remote / process | `../remote/v1/handshake/invalid--unknown-field.json` and process peer |
| WIT | `../wit/v1.0.0/` |
