//! Python binding package placeholder for `finstack-ai`.
//!
//! `PyO3`/`cdylib` wiring lands with the Python binding pull requests. This
//! crate currently depends on the facade with default features disabled so
//! the workspace graph stays free of host-language binding dependencies.
//!
//! PR-005 registers a deferred Python conformance adapter in `finstack-ai-test`.
//! That adapter reports unavailable and is not binding parity evidence.
