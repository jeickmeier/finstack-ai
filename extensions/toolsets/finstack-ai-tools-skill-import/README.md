# finstack-ai-tools-skill-import

Composition-time `SKILL.md` importer authorized by ADR-044. Catalog
default-off. The per-run path does no I/O. Imported capabilities are
untrusted and cannot set `trusted_application_instructions`.

```rust
use finstack_ai_tools_skill_import::{SkillCatalog, SkillMarkdown};

# fn demo() -> Result<(), finstack_ai_tools_skill_import::SkillImportError> {
let source = SkillMarkdown::from_text(
    "---\nname: reviewer\ndescription: Review notes\n---\nCite primary sources.\n",
);
let catalog = SkillCatalog::try_import(true, &[source])?;
assert_eq!(catalog.specs().len(), 1);
let empty = SkillCatalog::try_import(false, &[source])?;
assert!(empty.specs().is_empty());
# Ok(())
# }
```
