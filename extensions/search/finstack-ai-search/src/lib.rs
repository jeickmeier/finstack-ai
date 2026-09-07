//! Native bounded search federation over replaceable [`SearchSource`] backends.
//! The facade implements existing `Toolset` and `ContextProvider` ports only.
//! Bind authenticated scope at construction; never accept authority from a tool
//! query. Global recall replaces memory recall in federated compositions.
#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![doc(test(attr(allow(clippy::expect_used))))]

mod engine;
mod expansion;
mod provider;
mod schema;
mod toolset;

pub use engine::{SearchConfig, SearchEngine, SearchRequest};
pub use expansion::GraphExpansion;
pub use finstack_ai_search_core::*;
pub use provider::SearchContextProvider;
pub use toolset::{SEARCH_TOOL_ID, SearchToolset};
