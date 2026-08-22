# error-codes

Owner: `me@jeickmeier.com`
Compatibility profile: pre-1.0 classified breakage
Fixtures: `fixtures/compatibility/error-codes/`

Freezes the **values** of the stable error-code constants, which
`public-rust-api` cannot see: `cargo-public-api` records that
`AGENT_RUN_TIMEOUT: &str` exists, not that it holds `"agent_run_timeout"`.
Changing the string is a cross-language break, because the Python and
JavaScript bindings match on the value.

Generated. Do not hand-edit.

- Check (runs inside `mise run ci-rust`): `mise run check-public-api`
- Regenerate after an intended change: `mise run write-public-api`

Scope is constants declared as `pub const NAME: &str = "...";` whose value
satisfies the kernel's error-code grammar. Dotted identifiers -- tool ids,
digest domains, compaction strategy names -- are not error codes. Codes
returned inline from a `code()` arm without a named constant are not covered;
the fixture grows when those are named.
