# Filesystem sandbox reference

Read-only list/read fixture for a granted WASI preopen. This is an
isolated (T3) component, not `finstack-ai-tools-filesystem`.

Vendored WASI WIT is `wasi:filesystem@0.2.12` plus the `wasi:io` and
`wasi:clocks` packages that filesystem types import. Source:
`wasmtime-wasi` 47.0.3 `src/p2/wit/deps/{filesystem,io,clocks}.wit`.
Do not vendor sockets, HTTP, or CLI.

SHA-256 of the vendored copies:

```text
e675f261017bf9b4fa7df5b9701023fe0249ede71021ac1bf978748e62edcda8  filesystem.wit
96e206d00076fa0480df32c5bcf255a3fa4862805ac2f6b8537a781cce54f433  io.wit
6ed8aa65bb8cbe224a0b2cbac9fc1b3bd25bdb17eda5ae0d23c983ed31c447cc  clocks.wit
```

Manifest worlds stay `toolset-plugin`. Extra WASI imports require a
host `filesystem` grant and a concrete preopen.
