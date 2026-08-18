# Child-run compatibility fixtures

`v1/valid--accept-cancel-fanout.json` is the PR-079-A01 journal
fixture. The Rust `AgentRun` child-accept/cancel test loads the
required record kinds and asserts parent plus child `run_accepted` and
`cancellation_requested` entries. Lane-versus-child distinction stays
in the existing lane suite.
