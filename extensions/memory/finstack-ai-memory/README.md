# finstack-ai-memory

Memory extension composition (store, recall provider, toolset, observer) for
finstack-ai. This crate defines the bounded, validated memory record model
and will grow, across later tasks, the store trait, recall provider, tool
surface, and observer that compose on top of it.

```rust
use finstack_ai_memory::{MemoryId, MemoryScope};

let id = MemoryId::parse("mem-1").expect("id");
let scope = MemoryScope::try_new("tenant-a").expect("scope");
assert_eq!(scope.tenant(), "tenant-a");
let _ = id;
```
