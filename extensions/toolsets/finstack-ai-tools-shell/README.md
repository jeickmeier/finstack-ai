# finstack-ai-tools-shell

Trusted native (T1) host-authority shell battery implementing the public
finstack-ai `Toolset` port. The package executes an argv vector under a
deny-by-default program allowlist, an empty host environment, an optional
authorized working-directory handle, a deadline/cancellation timeout, and a
hard output-byte cap.

The default runner is in-process `std::process` plus rustix no-follow cwd
authorization. Hosts may inject a `CommandSandbox` adapter. This crate does
not ship landlock, bubblewrap, seatbelt, or `libloading` implementations.
Redirects and network are not granted. Host secrets are never copied into
the child environment.

This crate is a T1 native adapter. It is not isolated.

```rust
use finstack_ai_tools_shell::{ShellPolicy, ShellToolset};

let policy = ShellPolicy::try_new(["echo"]).expect("policy");
let tools = ShellToolset::try_new(policy, None).expect("shell");
```
