# Calculator reference component

Isolated `toolset-plugin` guest with identity
`finstack.plugin.calculator`. It copies native `evaluate` semantics
(max 1,024 finite operands; empty add = 0; empty multiply = 1) and is
**not** `finstack-ai-tools-calculator`.

Rebuild with `mise run gen-plugin-wasm`. Host conformance lives in
`finstack-ai-plugin-host` (`calculator_matches_native_evaluate_and_conformance`).
