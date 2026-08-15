# finstack-ai-server

Reference remote session server. Default listen is a Unix socket or
`127.0.0.1`. Non-loopback TCP requires explicit enablement, TLS 1.3, and
configured authentication. Bearer credentials are never accepted over
plaintext TCP; use a Unix socket or TLS.

`SecurityAuditGate::enable` is required before accept. This crate is not
part of the default SDK graph and does not add a seventh port.

Remote clients are [T4](../../docs/site/security-trust-levels.md).

## Quick start

See [docs/site/server.md](../../docs/site/server.md). Kernel / runtime /
protocol consumers should start from the
[Rust SDK guide](../../docs/site/rust.md):

```bash
cargo run -p finstack-ai-native-examples --bin minimal --offline --locked
```

This crate has no live-server quick start in CI.

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
[ADRs](../../docs/implementation/adr-register.md).
[RFCs](../../docs/rfcs/README.md).
