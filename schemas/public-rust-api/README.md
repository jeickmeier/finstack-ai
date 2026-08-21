# public-rust-api

Owner: `me@jeickmeier.com`  
Compatibility profile: pre-1.0 classified breakage  
Fixtures: `fixtures/compatibility/public-rust-api/`

Notes root for the public Rust API family. Authoritative source is `crates/`
(see `schemas/schema-families.toml` `source_root`). Versioned compatibility
fixtures cover typed IDs, raw JSON and metadata, digests, timestamps and
durations, source-free errors, blob and message values, run/effect/record/event
surfaces, strict reducer inputs, committed-batch bounds, and state-hash known
answers.

Public item and feature-name changes use the schema-change template and must
update fixtures under `fixtures/compatibility/public-rust-api/` when behavior
or serialization contracts change.
