//! Leaf-crate compile proof for the public target-correct `Toolset` ABI.

use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::Arc;

use finstack_ai_runtime::{
    Metadata, PortFuture, ToolCallContext, ToolError, ToolEventStream, ToolSpec, ToolStreamItem,
    Toolset, ToolsetDescriptor, ValidatedToolCall,
};
use futures_core::Stream;

/// Minimal external Toolset leaf using only the public runtime contract.
pub struct LeafToolset;

impl Toolset for LeafToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        ToolsetDescriptor {
            name: Arc::from("leaf-toolset"),
            metadata: Metadata::empty(),
        }
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::from([])
    }

    fn call(
        &self,
        _ctx: ToolCallContext,
        _call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        Box::pin(async { Ok(Box::pin(EmptyStream) as ToolEventStream) })
    }
}

struct EmptyStream;

impl Stream for EmptyStream {
    type Item = Result<ToolStreamItem, ToolError>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}
