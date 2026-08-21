# Established Codebase Conventions

Reference of patterns observed across the finstack workspace. When reviewing for consistency, deviations from these patterns are candidates for findings.

## Rust Conventions

### Error Handling

| Convention | Example | Crates Using |
|-----------|---------|--------------|
| `thiserror::Error` derive | `#[derive(Error, Debug)]` | Kernel, runtime, protocol, server |
| Stable error codes | `ErrorCode` lowercase `snake_case` | Kernel public errors |
| Helper constructors on errors | `ErrorCode::new(...)` | Kernel |
| Typed module errors | `error.rs` next to the owning module | Kernel, runtime |

**Deviation to watch:** Not every crate uses a single crate-root `Result` alias. Follow the local crate's established pattern.

### Builder Pattern

| Convention | Dominant Pattern | Deviation |
|-----------|-----------------|-----------|
| Entry point | `Type::builder(...)` or `Type::new(...)` | Follow the local type family |
| Setter prefix | Bare name | `set_` prefix only on mutators (not builders) |
| Terminal method | `.build()` | Overloads that take extra parameters |
| Return type | `Result<T>` from `.build()` | `T` directly for infallible construction |

### Trait Bounds

| Trait Category | Expected Bounds | Notes |
|---------------|----------------|-------|
| Public port traits | `Send + Sync` when used on native runtimes | WASM hosts may differ |
| Extension traits | None required | Keep local |
| Provider/toolset traits | `Send + Sync` on native | Follow the port contract |

### Module Organization

| Crate Size | Pattern | Example |
|-----------|---------|---------|
| Large module (>3 submodules) | `mod.rs` with submodules | `kernel/src/records/` |
| Small module (<3 submodules) | Single file | `protocol/src/error.rs` |

### Re-exports

| Pattern | Where Used | Notes |
|---------|-----------|-------|
| Root `pub use` | SDK / kernel `lib.rs` | Public types users construct or match on |
| No re-exports | Some internal crates | Access via full module path |

## Python Binding Conventions

| Convention | Pattern | Example |
|-----------|---------|---------|
| Package surface | Re-export compiled types from `__init__.py` | `finstack_ai.Agent` |
| Stub file | `_finstack_ai.pyi` | IDE-facing signatures |
| Error mapping | Centralized exception types | `FinstackError`, `ConfigurationError` |
| Inner field | `pub(crate) inner: RustType` when wrappers exist | Consistent across wrappers |

## WASM Binding Conventions

| Convention | Pattern | Example |
|-----------|---------|---------|
| JS function naming | `camelCase` via `js_name` | `createAgent` |
| JS type naming | `PascalCase` via `js_name` | `Agent`, `Session` |
| Inner field | `pub(crate) inner: RustType` | Matches Python pattern |
| Facade | Hand-authored `js/src/` over generated glue | Do not hand-edit `js/generated/` |

## Naming Patterns

### Domain Terms

When the same concept appears in multiple places, use the same term from the shared semantic vocabulary:

| Concept | Canonical Term | Avoid |
|---------|---------------|-------|
| Semantic run | `Run` | `Job`, `Execution` (unless a distinct type) |
| Durable session | `Session` | `Conversation` as a public type |
| Effect intent | `Effect` | `Action`, `SideEffect` |
| Machine-readable error | `code` | `error_type`, `kind` for the same field |

### Constants

| Scope | Style | Example |
|-------|-------|---------|
| `pub const` | `SCREAMING_SNAKE_CASE` | `LABEL_MAX_BYTES` |
| Module-level private | `SCREAMING_SNAKE_CASE` (preferred) | Mixed in practice |
| Function-level | `snake_case` | Local computation constants |

## Known Intentional Deviations

Document any places where divergence from the dominant pattern is intentional:

### Error & Module Structure
- Kernel splits error types by subsystem (`effects/error.rs`, `records/error.rs`) because each owns a distinct durable surface.
- Runtime keeps port-specific errors next to the port module.

### Generated Artifacts
- `bindings/finstack-ai-wasm/js/generated/` is produced by `mise run build-wasm -- release`. Hand edits are never a consistency fix.

### Documentation
