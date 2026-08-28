# finstack-ai

Deterministic agent microkernel, runtime, SDK, and Python/WASM bindings. Rust owns
semantic state, records, events, effects, continuation, recovery, lineage, and
error semantics. Providers, tools, stores, observers, bindings, and isolated
plugins remain outside the kernel.

Workspace manifests are staged at **2.0.0**. No `v2.0.0` tag exists; the latest
local release tag is `v1.0.0`. Build from this repository until a 2.0 release is
published.

## Quick start

All starters are deterministic and run without external credentials.

### Rust

```bash
mise run install-all
cargo run -p finstack-ai-native-examples --bin minimal --offline --locked
```

The public Rust facade is `finstack-ai`. Its default feature enables the native
Tokio runtime; provider and tool batteries remain opt-in. See the
[`finstack-ai` crate guide](crates/finstack-ai/README.md) and
[`examples/rust-minimal`](examples/rust-minimal/).

### Python

```bash
mise run install-all
uv run --isolated --no-project --with-editable bindings/finstack-ai-python \
  python examples/python-minimal/python-callback/main.py
```

The wheel is a typed PyO3 facade over the same Rust engine. Async APIs are the
primary interface. See the [Python guide](bindings/finstack-ai-python/README.md),
[API reference](bindings/finstack-ai-python/docs/api-reference.md), and shipped
[`py.typed` stub](bindings/finstack-ai-python/python/finstack_ai/_finstack_ai.pyi).

### Browser/WASM

```bash
mise run install-all
mise run build-wasm -- release
```

The hand-authored `@finstack/ai` TypeScript facade drives the Rust engine in the
browser. The production topology uses a Dedicated Worker; host adapters are
trusted page code, not a sandbox. See the
[WASM package guide](bindings/finstack-ai-wasm/js/README.md),
[browser security notes](bindings/finstack-ai-wasm/js/docs/browser-security.md),
and [`examples/browser-minimal`](examples/browser-minimal/).

## Architecture and invariants

| Layer | Responsibility |
| --- | --- |
| `finstack-ai-kernel` | Synchronous, deterministic, I/O-free semantic state; normalized records, events, and effects. |
| `finstack-ai-runtime` | Six port contracts, effect execution, commit coordination, cancellation, deadlines, and recovery. |
| `finstack-ai` | Public Rust composition API and ergonomic agent/session/run handles. |
| `finstack-ai-protocol` | Canonical remote codecs and pre-beta shape normalization. |
| `extensions/` | Trusted native providers, tools, stores, context sources, middleware, observers, workflow adapters, and interop. |
| `bindings/` | Typed Python and browser/WASM facades over Rust-owned behavior. |
| `plugins/` | WIT contracts, guest SDK, and isolated Wasmtime host. |

The load-bearing lifecycle rules are:

- recoverable effect intent is committed before external dispatch;
- duplicate identities are idempotent and conflicting reuse fails closed;
- cancellation, deadlines, uncertainty, and terminal state remain distinct;
- observers cannot change semantic behavior or terminal state;
- child runs preserve parent/effect lineage and explicit placement;
- the kernel performs no network, storage, clock, randomness, or host callbacks;
- Python and JavaScript convert host values but do not reimplement Rust semantics.

See [`AGENTS.md`](AGENTS.md) for package boundaries and repository rules.

## Public surfaces

| Audience | Entry point | Documentation |
| --- | --- | --- |
| Rust application authors | `finstack-ai` | [crate guide](crates/finstack-ai/README.md) |
| Python application authors | `finstack_ai` | [binding guide](bindings/finstack-ai-python/README.md), [API reference](bindings/finstack-ai-python/docs/api-reference.md) |
| Browser application authors | `@finstack/ai` | [package guide](bindings/finstack-ai-wasm/js/README.md) |
| Plugin guest authors | `finstack-ai-guest-sdk` | [guest SDK guide](plugins/finstack-ai-guest-sdk/README.md) |
| Plugin host operators | `finstack-ai-plugin-host` | [host guide](plugins/finstack-ai-plugin-host/README.md) |
| Protocol implementers | `finstack-ai-wit`, `finstack-ai-protocol` | [WIT guide](plugins/finstack-ai-wit/README.md), [compatibility policy](plugins/finstack-ai-wit/COMPATIBILITY.md) |

The complete runnable catalog is under [`examples/`](examples/README.md).

## Trust and persistence

Native Rust extensions, Python callbacks, and JavaScript host adapters execute
inside the application process and inherit its authority. Register only trusted
code. Use the plugin host for untrusted WIT components and grant only explicit,
bounded capabilities.

Persistence guarantees depend on the selected store:

- the Rust in-memory store is process-local;
- SQLite and PostgreSQL batteries provide native persistent journals;
- browser IndexedDB adapters are experimental and support inspection, not
  crash-durable continuation;
- dropping a `Run` handle only detaches observation—call `cancel` explicitly.

Never embed provider credentials in browser bundles. Browser provider traffic
must terminate credentials at a trusted same-origin proxy.

## Workspace layout

- `crates/` — kernel, protocol, runtime, SDK, server, and conformance support.
- `extensions/` — trusted native batteries grouped by capability family.
- `bindings/` — Python/PyO3 and browser/WASM packages.
- `plugins/` — WIT contracts, guest helpers, Wasmtime host, templates, and
  reference components.
- `examples/` — Rust, Python, browser, notebook, and durable-interaction
  starters.
- `fixtures/` — compatibility and feature-boundary fixtures.
- `scripts/` — checked-in generators and CI validation.

A directory family is organizational, not a contract. Read a crate's trait
implementations to determine which runtime port it provides.

## Developer bootstrap

This repository uses [mise](https://mise.jdx.dev/) for pinned tools and
checked-in tasks.

1. Install mise: <https://mise.jdx.dev/getting-started.html>
2. Run `mise run install-all` from the repository root.
3. Use `mise run ci-all` before handoff.

Common tasks:

- `mise run ci-all` — sequential equivalent of hosted Rust, Python, and WASM CI.
- `mise run ci-rust`, `ci-python`, `ci-wasm` — required per-language checks.
- `mise run build-all`, `build-rust`, `build-python`, `build-wasm` — builds; pass
  an optional profile after `--` where supported.
- `mise run check-all`, `check-rust`, `check-python`, `check-wasm` — formatting,
  lint, type, architecture, and generated-artifact checks.
- `mise run test-all`, `test-rust`, `test-python`, `test-wasm` — language test
  suites.
- `mise run test-fast` — workspace nextest, virtual-environment pytest, and the
  Chromium WASM path.
- `mise run coverage-all` and `mise run bench-all` — diagnostic coverage and
  benchmark entry points.

Prefer `mise run <task>` over ad-hoc wrappers. The task definitions live in
[`mise.toml`](mise.toml); hosted CI details live in
[`.github/ci/README.md`](.github/ci/README.md).

`uv sync` at the repository root creates `.venv` with editable
`finstack-ai[pydantic]` and notebook dependencies. Notebook setup is documented
in [`examples/python-notebooks`](examples/python-notebooks/README.md).

## Compatibility and generated artifacts

Public Rust inventories, Python stubs, TypeScript declarations, WIT packages,
protocol fixtures, and generated WASM glue are compatibility-controlled.
Regenerate them only through their checked-in `mise` or `scripts/` commands;
CI rejects drift.

Permanent WIT worlds use `@1.0.0`. Experimental guest packages use `@0.0.4` and
follow the [WIT compatibility policy](plugins/finstack-ai-wit/COMPATIBILITY.md).
Release-facing changes belong in [`CHANGELOG.md`](CHANGELOG.md).

## License and governance

- Dual-licensed: [MIT](licenses/LICENSE-MIT) OR [Apache-2.0](licenses/LICENSE-APACHE)
- Contributions: [CONTRIBUTING.md](CONTRIBUTING.md) (DCO sign-off; no CLA)
- Code of Conduct: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
- Maintainers: [GOVERNANCE.md](GOVERNANCE.md)
- Vulnerability reports: [SECURITY.md](SECURITY.md)
