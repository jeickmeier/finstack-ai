# PR-027 implementation plan

Date: 2026-08-11  
Owner: `me@jeickmeier.com`  
Branch: `codex/pr-027-python-package`  
Baseline: clean `main` at `b68d4179b8da6db081d9c6acdef76e33fa67948d`

## Execution envelope

PR-027 is the only active member of the authorized PR-027 through PR-038
range. The range uses integrated mode with target `main`. Local branch,
commit, and merge actions plus external branch/`main` pushes and hosted
CI/security workflows are authorized. Hosted pull-request creation/merge and
package publication are prohibited; release artifacts may be staged.

## Frozen scope

1. Replace the placeholder binding with one CPython-specific PyO3 extension
   module at `finstack_ai._finstack_ai` and a thin typed `finstack_ai` facade.
2. Link the current SDK facade and OpenAI-compatible provider into the one
   extension-module composition without exposing Agent or callback APIs.
3. Configure a maturin mixed Rust/Python project with editable development,
   wheel and sdist builds, checked-in `.pyi`/`py.typed`, exact build pins, and
   centralized package metadata.
4. Expose only side-effect-free build/version metadata and a minimal health
   function. Import and health checks must not initialize Tokio, create a
   network client, open a socket, or read credentials.
5. Add the approved CPython 3.11, 3.12, 3.13, 3.14, and version-specific 3.14t
   wheel matrix for manylinux x86_64/aarch64, macOS arm64, and Windows x64.
6. Validate wheel/sdist contents, clean installation without a Rust compiler,
   free-threaded concurrency, package/workspace version alignment, Rust-only
   dependency isolation, lazy imports, and the initial wheel-size budget.

The build baseline uses PyO3 `0.29.0` and maturin `1.14.1`, selected from the
current official PyO3/maturin documentation and release metadata. Classic
`abi3` is deliberately disabled.

## Acceptance map

| Acceptance | Planned proof |
| --- | --- |
| PR-027-A01 | Hosted build/install/import matrix executes every approved interpreter and platform combination, including concurrent 3.14t smoke |
| PR-027-A02 | Subprocess import and health tests inspect process state and prove no Tokio runtime or network resource is initialized |
| PR-027-A03 | Rust workspace, Python distribution, extension build, wheel metadata, and runtime version/build metadata match |
| PR-027-A04 | Rust-only minimal/default builds and dependency inventories exclude PyO3, maturin, and Python toolchain dependencies |
| PR-027-A05 | Source and wheel archives are staged twice, inspected for stubs/licenses/provider linkage, checked against the size budget, and compared reproducibly without publication |

## Security and compatibility disposition

The packaging and release-identity review trigger applies because PR-027
changes build artifacts and the Python distribution surface. The review will
cover exact dependency pins/hashes, generated-artifact ownership, credential-
free examples and imports, artifact contents, secret scanning, and hosted
matrix provenance. No new trust class, network listener, parser, privileged
host capability, kernel behavior, durable schema, protocol, or plugin ABI is
introduced, so no separate ADR trigger is identified.

Agent/control handles, Python callbacks, Pydantic adapters, semantic parity,
PyPI publication, Anthropic/local provider implementations, stable ABI,
durability, browser/WASM, and plugin isolation remain explicitly excluded.
