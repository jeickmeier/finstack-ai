# finstack-ai-tools-filesystem

Trusted native filesystem battery implementing the public finstack-ai `Toolset`
port. The package opens an explicit root directory capability and exposes
bounded read, write, edit, list, glob, and literal content-search tools.

On Unix, every operation walks relative directory handles with no-follow flags
and authorizes the opened object. Targets without the required safe primitives
fail construction closed. The package does not execute commands, inherit an
ambient root, or create untracked spill files.
