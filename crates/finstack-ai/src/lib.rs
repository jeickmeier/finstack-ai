//! Public SDK/facade for the `finstack-ai` agent engine.
//!
//! Owns composition and ergonomic adapters over the runtime. The default
//! `native-tokio` feature selects the runtime Tokio driver; browser WASM
//! consumers disable defaults and enable `wasm-host`. Implementation arrives
//! in later Phase 3 pull requests.
