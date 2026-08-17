# finstack-ai-tools-calculator

Deterministic, bounded arithmetic battery implementing the public finstack-ai
`Toolset` port. It exposes one cached `calculator` specification and performs no
I/O.

This crate is a T1 native adapter. It is not isolated.

```rust
use finstack_ai_tools_calculator::CalculatorToolset;

let tools = CalculatorToolset::try_new().expect("calculator");
```
