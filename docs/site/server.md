# Server

`finstack-ai-server` is a reference remote-session host. It is not part of
the default SDK graph and does not add a seventh port.

Default listen is a Unix socket or `127.0.0.1`. Non-loopback TCP requires
explicit enablement, TLS 1.3, and configured authentication. Bearer
credentials are never accepted over plaintext TCP.

`SecurityAuditGate::enable` is required before accept. Remote clients are
[T4](security-trust-levels.md). Process-family handshake is a distinct
vocabulary; it is not a completed isolation story.

See [crates/finstack-ai-server](../../crates/finstack-ai-server/README.md)
and [compatibility governance](../implementation/compatibility-governance.md)
for the remote and process families.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
