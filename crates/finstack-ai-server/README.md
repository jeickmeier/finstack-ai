# finstack-ai-server

Reference remote session server. Default listen is a Unix socket or
`127.0.0.1`. Non-loopback TCP requires explicit enablement, TLS 1.3, and
configured authentication. Bearer credentials are never accepted over
plaintext TCP; use a Unix socket or TLS.

`SecurityAuditGate::enable` is required before accept. This crate is not
part of the default SDK graph and does not add a seventh port.
