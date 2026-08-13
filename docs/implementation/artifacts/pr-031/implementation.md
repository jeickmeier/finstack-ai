# PR-031 implementation review

Date: 2026-08-12
Branch: `codex/pr-031-pydantic-adapters`

## Delivered boundary

- Pydantic is an optional Python extra. Importing `finstack_ai` imports only the
  standard-library adapter module and the native extension; Pydantic is loaded
  lazily when `@tool` or `output_type=` is used.
- One binding-owned normalizer accepts a bounded portable Draft 2020-12 subset,
  injects `additionalProperties: false` for object schemas, sorts required
  fields, permits local `$defs`/`$ref`, and rejects unsupported keywords,
  optional provider properties, external references, or non-object provider
  roots with an exact JSON pointer.
- `@tool` accepts fully annotated sync or async functions. Registration builds
  and caches one validation-mode input TypeAdapter, one optional
  serialization-mode output TypeAdapter, and their normalized schemas.
  `refresh_schema()` is the sole explicit regeneration path before creating a
  new toolset.
- `pydantic_toolset()` converts already Rust-validated raw arguments into Python
  once and serializes the result once. Invalid input never enters Python;
  invalid annotated output is returned to the existing Rust output validator,
  which preserves native error codes and retry settlement.
- `Agent.from_python(..., output_type=...)` accepts a Pydantic model,
  dataclass, TypedDict, arbitrary TypeAdapter-compatible target, or an existing
  TypeAdapter. It generates the validation schema once, compiles the canonical
  Rust validator once, configures `OutputSpec::JsonSchema` before `BeforeRun`,
  submits `OutputValidated` after model settlement, and drives retryable
  validation failures through the existing durable retry timer.
- A successful `RunResult.output` is the cached TypeAdapter's typed object;
  `RunResult.retry_attempts` exposes the kernel-owned durable count used by
  trace/parity tests.

## Invariants retained

- The synchronous kernel remains deterministic and I/O-free.
- Rust remains the semantic schema validator and retry owner for native,
  Python, and future WASM bindings. Pydantic validation is limited to Python
  object crossings.
- Schema compilation, output configuration, validation outcomes, retry
  scheduling, and timer completion follow commit-before-effect ordering.
- No network or filesystem schema retrieval is available.
- The fixed six ports, seven middleware stages, trusted in-process callback
  boundary, and one Rust-owned semantic engine remain unchanged.

## Explicit limits

- The portable subset intentionally rejects provider-incompatible constraints
  such as `minLength`, numeric bounds, defaults, `oneOf`, external references,
  open objects, optional object properties, and non-object tool-input or model
  output roots.
- Annotated tool parameters with defaults are rejected because the initial
  strict provider subset requires every argument. Optional values remain
  expressible as required nullable fields.
- Output-type selection is supported for trusted Python callback agents in this
  PR. Provider-specific translation and browser/WASM binding ergonomics remain
  later logical PRs.
- No publication, hosted PR/merge, independent review, browser binding, or G4
  decision is claimed by source implementation.
