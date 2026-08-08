//! Envelope, framing, journal, and remote protocol codecs for `finstack-ai`.
//!
//! Depends on public kernel semantic DTOs only where journal encoding requires
//! them. Kernel, runtime, and SDK must not depend on this crate for in-process
//! execution. Implementation arrives in later pull requests.
