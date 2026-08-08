//! Private CI-only release smoke binary.
//!
//! Depends on the public `finstack-ai` facade so hosted CI can prove a
//! minimal release binary builds on Linux, macOS, and Windows. This is not a
//! product CLI and must not grow application APIs.

fn main() {
    println!("finstack-ai-ci-smoke {}", env!("CARGO_PKG_VERSION"));
}
