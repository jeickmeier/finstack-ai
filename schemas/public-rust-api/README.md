# public-rust-api

Owner: `me@jeickmeier.com`  
Compatibility profile: pre-1.0 classified breakage  
Fixtures: `fixtures/compatibility/public-rust-api/`

Notes root for the public Rust API family. Authoritative source is `crates/`
(see `schemas/schema-families.toml` `source_root`). PR-006 activates the family
with versioned compatibility fixtures for typed IDs, RawJson/Metadata, digests,
timestamps/durations, and source-free error descriptors. PR-007 extends the
family with blob-ref, content-block, and message fixtures. PR-008 adds
run/effect/record/event surfaces. PR-009 adds strict reducer inputs and phases,
committed-batch bounds, state-hash known answers, and reducer-owned record/event
derivation fixtures.

Public item and feature-name changes use the schema-change template and must
update fixtures under `fixtures/compatibility/public-rust-api/` when behavior
or serialization contracts change.
