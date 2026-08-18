# finstack-ai-tools-subagent

Model-facing subagent battery over the host `AgentInvoker`. It exposes
`subagent_start`, `subagent_await`, and `subagent_cancel` and holds no
invocation authority of its own.

`ChildRunPolicy` is enforced by the runtime accept path, not this crate. Policy,
depth, and budget failures are returned as tool results. This crate is a T1
native adapter. It is not isolated. Remote child cancel is not asserted here.

```rust
use std::sync::Arc;

use finstack_ai_runtime::{AgentId, AgentRef, AgentInvoker, Digest};
use finstack_ai_tools_subagent::SubagentToolset;

# fn demo(invoker: Arc<dyn AgentInvoker>, allow: AgentRef) {
let tools = SubagentToolset::try_new(invoker, Arc::from([allow])).expect("toolset");
# let _ = tools;
# }
```
