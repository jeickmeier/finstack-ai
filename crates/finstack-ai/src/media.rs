//! Typed media toolsets composed independently of the model provider.
//!
//! Construct toolsets with one shared artifact store, then pass their versioned
//! registrations through `NativeAgentBuilder::toolset` or `LinkedAgentPorts::toolsets`.
//! Pipeline configurations take the same media and composition handles so their
//! ownership, limits, approval policy, and result schemas remain authoritative.

/// `OpenAI` media configuration and toolset.
#[cfg(feature = "tool-openai-media")]
pub use finstack_ai_tools_openai_media as openai;
/// `OpenRouter` media configuration and toolset.
#[cfg(feature = "tool-openrouter-media")]
pub use finstack_ai_tools_openrouter_media as openrouter;
/// Local ffmpeg configuration and toolset.
#[cfg(feature = "tool-video-compose")]
pub use finstack_ai_tools_video_compose as video;
/// `MoviePlan` driver, toolset, limits, and durable render-state stores.
#[cfg(feature = "workflow-media-pipeline")]
pub use finstack_ai_workflow_media_pipeline as pipeline;

#[cfg(all(test, feature = "linked-providers", feature = "linked-tools"))]
mod tests;
