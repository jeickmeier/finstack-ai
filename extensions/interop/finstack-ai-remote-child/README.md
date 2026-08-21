# finstack-ai-remote-child

Native `AgentInvoker` for `RemoteChildSession` over  remote framing.

Construction takes an explicit loopback TCP or absolute Unix-socket route plus optional
Bearer credentials. The invoker never reads environment variables and never
discovers peers. Non-loopback plaintext is rejected. `start_or_attach` is
idempotent for an equal complete locator and request digest; concurrent equal
starts share one exchange. Capacity is reserved before network I/O. `cancel` sends a durable remote
`Cancel` command and does not no-op.

This crate is a T1 native adapter. It is not compiled into `wasm-host`.

```rust
use finstack_ai_remote_child::{RemoteChildInvoker, RemoteChildRoute};

let invoker = RemoteChildInvoker::try_new(RemoteChildRoute {
    endpoint: "127.0.0.1:9".into(),
    service: "finstack.remote.worker".into(),
    route: "route-1".into(),
    token: None,
})
.expect("route");
```
