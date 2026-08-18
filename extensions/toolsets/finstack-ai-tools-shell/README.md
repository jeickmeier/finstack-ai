# finstack-ai-tools-shell

Trusted native (T1) host-authority shell battery implementing the public
finstack-ai `Toolset` port. The package executes an argv vector under a
deny-by-default program allowlist, an empty host environment, an optional
authorized working-directory handle, a deadline/cancellation timeout, and a
hard output-byte cap.

The default runner is the labeled unconfined in-process `std::process`
path plus rustix no-follow cwd authorization. Hosts may inject a
`CommandSandbox` adapter or call `ShellToolset::try_with_confinement` to
consume the runtime `ProcessConfinement` service (Linux Landlock +
`no_new_privs`, macOS Seatbelt, Windows restricted token + Job Object).
Missing primitives fail closed and never spawn unconfined. This crate
does not ship a remote/E2B sandbox or `libloading`. Redirects and
network are not granted. Host secrets are never copied into the child
environment.

This crate is a T1 native adapter. Confinement does not upgrade that
label. It is not isolated.

```rust
use finstack_ai_tools_shell::{ShellPolicy, ShellToolset};

let policy = ShellPolicy::try_new(["echo"]).expect("policy");
let tools = ShellToolset::try_new(policy, None).expect("shell");
```
