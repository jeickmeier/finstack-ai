# finstack-ai-server

Reference remote session server. Default listen is a Unix socket or
`127.0.0.1`. Non-loopback TCP requires explicit enablement, TLS 1.3, and
configured authentication. Bearer credentials are never accepted over
plaintext TCP; use a Unix socket or TLS.

`SecurityAuditGate::enable` is required before accept. This crate is not
part of the default SDK graph and does not add a seventh port.

Remote clients are T4.

## Quick start

Kernel, runtime, and protocol consumers should start from the facade API below:

```bash
cargo run -p finstack-ai-native-examples --bin minimal --offline --locked
```

This crate has no live-server quick start in CI.

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
