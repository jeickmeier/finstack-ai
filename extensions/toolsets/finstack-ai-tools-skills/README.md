# finstack-ai-tools-skills

Model-facing capability catalog tools. `capability_list` renders the compact
id-and-description catalog. `capability_activate` is additions-only: the host
computes `current ∪ named` and submits that complete set.

This crate is a T1 native adapter. It is not isolated. It does not scan the
filesystem and does not import `SKILL.md` (that is FR-10). Mid-run activation
invalidates Anthropic/OpenAI prompt-cache prefixes; that cost is recorded, not
solved here.

```rust
use std::sync::Arc;

use finstack_ai_tools_skills::{SkillsHost, SkillsHostError, SkillsToolset};

# fn demo() {
let host = SkillsHost {
    catalog: "demo.research: Research notes".to_owned(),
    active: Arc::new(|_run| Ok(Vec::new())),
    activate: Arc::new(|_run, _complete| Ok(())),
};
let _ = SkillsToolset::try_new(host);
let _ = SkillsHostError::Bound;
# }
```
