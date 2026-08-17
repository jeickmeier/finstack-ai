# C1 jsonschema graph spike

Measured 2026-08-17 against workspace `HEAD` `9781190f70e72bca1ea7e68351a346465104432a`
with `cargo tree --locked -e normal --prefix none --format '{p}'`, unique names
after stripping the ` v<version>` suffix. Isolated candidate graphs used
throwaway crates under `/tmp` and did not change workspace `Cargo.toml`.

## Baseline unique package counts

| Graph | Unique names | List |
| --- | ---: | --- |
| `finstack-ai-kernel` host | 26 | [kernel-host-packages.txt](kernel-host-packages.txt) |
| `finstack-ai-runtime` host | 97 | [runtime-host-packages.txt](runtime-host-packages.txt) |
| `finstack-ai-wasm` host | 119 | [wasm-host-packages.txt](wasm-host-packages.txt) |
| `finstack-ai-wasm` `wasm32-unknown-unknown` | 117 | [wasm-wasm32-packages.txt](wasm-wasm32-packages.txt) |
| workspace `jsonschema` 0.40.2 subtree | 82 | [jsonschema-040-workspace-packages.txt](jsonschema-040-workspace-packages.txt) |

Runtime packages that are neither kernel nor the `jsonschema` subtree:
`finstack-ai-runtime`, `futures-core`, `uuid`.

## Isolated validator subtrees

| Candidate | Host unique names | `wasm32` unique names | List |
| --- | ---: | ---: | --- |
| Control: `jsonschema` 0.40.2, `default-features = false` | 81 | 85 | [jsonschema-040-isolated-packages.txt](jsonschema-040-isolated-packages.txt) |
| `jsonschema` 0.49.9, `default-features = false` | 87 | 91 | [jsonschema-049-isolated-packages.txt](jsonschema-049-isolated-packages.txt) |
| `boon` 0.6.1 | 55 | 54 | [boon-061-isolated-packages.txt](boon-061-isolated-packages.txt) |

Estimated workspace unique names after a swap, keeping kernel overlap
(`(graph - jsonschema-subtree) ∪ (kernel ∩ graph) ∪ replacement`):

| Graph | Baseline | `jsonschema` 0.49.9 | `boon` 0.6.1 |
| --- | ---: | ---: | ---: |
| runtime host | 97 | 103 | 72 |
| wasm host | 119 | 124 | 93 |
| wasm `wasm32` | 117 | 123 | 92 |

`jsonschema` 0.49.9 adds `heck`, `jsonschema-regex`, `jsonschema-value`,
`micromap`, `strum`, and `strum_macros` versus 0.40.2. It does not drop
`idna`, ICU, `email_address`, or `fancy-regex`.

`boon` 0.6.1 still depends on `idna = "1.0"` and therefore the same ICU
cluster. It drops `email_address`, `fancy-regex`, `fraction`/`num-*`,
`referencing`, and `foldhash` from the validator subtree.

## Size budget snapshot

Checked-in `bindings/finstack-ai-wasm/js/generated/finstack_ai_wasm_bg.wasm`
is 8,465,252 bytes against [perf-size-budgets.json](../../perf-size-budgets.json)
`wasm-bg` max 8,650,752 (185,500 bytes headroom). No candidate crate drops
ICU data, so a C2 swap is not expected to create material wheel/WASM
headroom. `boon` embeds draft metaschemas with `include_str!`, which can
grow the artifact.

## License impact versus `deny.toml`

`deny.toml` allowlists `Unicode-3.0`, `MIT-0`, and `Zlib` for this graph.

| License | Current `jsonschema` 0.40 sources | After `jsonschema` 0.49 | After `boon` 0.6.1 | Workspace after a runtime-only swap |
| --- | --- | --- | --- | --- |
| `Unicode-3.0` | ICU cluster plus `unicode-ident` | same | same | stays (`unicode-ident` via `syn`; ICU via `idna`) |
| `MIT-0` | `borrow-or-share` (`fluent-uri`) | same | same | stays |
| `Zlib` | `foldhash` | same | absent from isolated `boon` tree | stays (`foldhash` via Wasmtime/`hashbrown`) |

No candidate lets `deny.toml` drop `Unicode-3.0` or `MIT-0`. `Zlib` cannot
be removed from the workspace allowlist after a runtime-only swap.

## Capability probes (not a production swap)

- `jsonschema` 0.40.2: compile-once Draft 2020-12, `with_resources`,
  `DenyRetriever`, native and workspace `wasm32` already evidenced by PR-016.
- `jsonschema` 0.49.9: `with_retriever` still compiles; `with_resources` is
  gone in favor of `Registry::new().add(...).prepare()` plus `with_registry`.
  Default features add `resolve-http`, `resolve-file`, and `tls-aws-lc-rs`
  and must stay off. Isolated `cargo check` passed native and
  `wasm32-unknown-unknown`.
- `boon` 0.6.1: `Draft::V2020_12`, `add_resource`, and a `UrlLoader` that
  denies ambient retrieve passed the portable fixture cases and the
  `urn:finstack:positive` offline `$ref` case. Default native loader
  registers `file://` `FileLoader`; a swap must install a deny loader.
  Isolated `wasm32` check requires an explicit `getrandom` `wasm_js` pin
  plus `--cfg getrandom_backend="wasm_js"` because today's pin lives on
  `jsonschema` 0.40 / `ahash`.
