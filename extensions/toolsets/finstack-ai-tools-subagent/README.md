# finstack-ai-tools-subagent

Model-facing subagent battery over a host-bound `ChildRunStarter`. It exposes
`subagent_start`, `subagent_status`, and `subagent_cancel` and holds no
invocation authority of its own.

`ChildRunPolicy` is enforced by the runtime accept path, not this crate. Policy,
depth, and budget failures are returned as tool results. This crate is a T1
native adapter. It is not isolated. Remote child cancel is not asserted here.

```rust
use std::sync::Arc;

use finstack_ai_runtime::{AgentRef, ChildRunStarter};
use finstack_ai_tools_subagent::SubagentToolset;

# fn demo(starter: Arc<ChildRunStarter>, allow: AgentRef) {
let tools = SubagentToolset::try_new(starter, Arc::from([allow])).expect("toolset");
# let _ = tools;
# }
```
