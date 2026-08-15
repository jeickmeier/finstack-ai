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
