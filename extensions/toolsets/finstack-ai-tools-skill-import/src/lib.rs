//! Composition-time `SKILL.md` importer. This is not a port and not a plugin.
//!
//! Authorized by ADR-044. Catalog default-off. Digests cover content, not
//! path. `scripts/` and `references/` fail closed. The per-run path does
//! no I/O.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::sync::Arc;

use finstack_ai_kernel::{CapabilityId, Digest};
use finstack_ai_runtime::spec::{CapabilityActivation, CapabilitySpec, InstructionSpec};
use thiserror::Error;

/// Stable parse or catalog failure.
pub const SKILL_IMPORT_INVALID: &str = "skill_import_invalid";

/// Import failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{SKILL_IMPORT_INVALID}: {reason}")]
pub struct SkillImportError {
    reason: &'static str,
}

impl SkillImportError {
    const fn new(reason: &'static str) -> Self {
        Self { reason }
    }

    /// Stable reason label.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

/// One in-memory `SKILL.md` document. The source path is never retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMarkdown {
    text: Arc<str>,
}

impl SkillMarkdown {
    /// Wrap document text. Callers that read a file must drop the path.
    #[must_use]
    pub fn from_text(text: impl Into<Arc<str>>) -> Self {
        Self { text: text.into() }
    }

    /// Borrow the document text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// One imported capability plus the content digest used in the lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSkill {
    spec: CapabilitySpec,
    content_digest: Digest,
}

impl ImportedSkill {
    /// Parsed capability. Instructions are untrusted.
    #[must_use]
    pub const fn spec(&self) -> &CapabilitySpec {
        &self.spec
    }

    /// Digest over id, description, and body. Path is not included.
    #[must_use]
    pub const fn content_digest(&self) -> Digest {
        self.content_digest
    }
}

/// Opt-in imported catalog. Disabled catalogs stay empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillCatalog {
    imported: Arc<[ImportedSkill]>,
}

impl SkillCatalog {
    /// Default-off catalog.
    #[must_use]
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Import documents only when `opt_in` is true.
    ///
    /// # Errors
    ///
    /// Returns [`SkillImportError`] when a document is malformed or uses
    /// unsupported frontmatter.
    pub fn try_import(opt_in: bool, sources: &[SkillMarkdown]) -> Result<Self, SkillImportError> {
        if !opt_in {
            return Ok(Self::disabled());
        }
        let mut imported = Vec::with_capacity(sources.len());
        for source in sources {
            imported.push(import_skill_markdown(source.text())?);
        }
        Ok(Self {
            imported: imported.into(),
        })
    }

    /// Imported capability specs in source order.
    #[must_use]
    pub fn specs(&self) -> Vec<&CapabilitySpec> {
        self.imported.iter().map(ImportedSkill::spec).collect()
    }

    /// Content digests in source order.
    #[must_use]
    pub fn content_digests(&self) -> Vec<Digest> {
        self.imported
            .iter()
            .map(ImportedSkill::content_digest)
            .collect()
    }
}

/// Parse one `SKILL.md` document into a capability spec.
///
/// # Errors
///
/// Returns [`SkillImportError`] for missing frontmatter, unsupported keys,
/// or invalid capability fields.
pub fn import_skill_markdown(source: &str) -> Result<ImportedSkill, SkillImportError> {
    let (frontmatter, body) = split_frontmatter(source)?;
    let fields = parse_frontmatter(frontmatter)?;
    if fields.scripts || fields.references {
        return Err(SkillImportError::new(
            "scripts_and_references_are_unsupported",
        ));
    }
    let id = capability_id(&fields)?;
    let description = fields
        .description
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| id.as_str().to_owned());
    let instruction = InstructionSpec::try_new(body.trim())
        .map_err(|_| SkillImportError::new("skill_body_is_not_a_valid_instruction"))?;
    let spec = CapabilitySpec {
        id: id.clone(),
        description: Arc::from(description),
        instructions: Arc::from([instruction]),
        toolsets: Arc::from([]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation: CapabilityActivation::Application,
    };
    spec.validate()
        .map_err(|_| SkillImportError::new("capability_spec_is_invalid"))?;
    let content_digest = content_digest(&spec);
    Ok(ImportedSkill {
        spec,
        content_digest,
    })
}

fn content_digest(spec: &CapabilitySpec) -> Digest {
    let instructions = spec
        .instructions
        .iter()
        .map(InstructionSpec::text)
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "id": spec.id.as_str(),
        "description": spec.description.as_ref(),
        "instructions": instructions,
    });
    let bytes = serde_json_canonicalizer::to_vec(&payload).unwrap_or_else(|_| Vec::from(b"{}"));
    Digest::raw_json(&bytes)
}

struct Frontmatter {
    name: Option<String>,
    id: Option<String>,
    description: Option<String>,
    scripts: bool,
    references: bool,
}

fn split_frontmatter(source: &str) -> Result<(&str, &str), SkillImportError> {
    let rest = source
        .strip_prefix("---\n")
        .ok_or(SkillImportError::new("skill_frontmatter_is_required"))?;
    let (front, body) = rest
        .split_once("\n---\n")
        .ok_or(SkillImportError::new("skill_frontmatter_is_unterminated"))?;
    Ok((front, body))
}

fn parse_frontmatter(front: &str) -> Result<Frontmatter, SkillImportError> {
    let mut fields = Frontmatter {
        name: None,
        id: None,
        description: None,
        scripts: false,
        references: false,
    };
    for raw in front.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(SkillImportError::new("skill_frontmatter_line_is_invalid"));
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        match key {
            "name" => fields.name = Some(value.to_owned()),
            "id" => fields.id = Some(value.to_owned()),
            "description" => fields.description = Some(value.to_owned()),
            "scripts" => fields.scripts = true,
            "references" => fields.references = true,
            _ => return Err(SkillImportError::new("skill_frontmatter_key_is_unknown")),
        }
    }
    Ok(fields)
}

fn capability_id(fields: &Frontmatter) -> Result<CapabilityId, SkillImportError> {
    let raw = fields
        .id
        .as_deref()
        .or(fields.name.as_deref())
        .ok_or(SkillImportError::new("skill_name_or_id_is_required"))?;
    let id = if raw.contains('.') {
        raw.to_owned()
    } else {
        format!("skill.{raw}")
    };
    CapabilityId::parse(id).map_err(|_| SkillImportError::new("skill_capability_id_is_invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REVIEW: &str =
        "---\nname: reviewer\ndescription: Review notes\n---\nCite primary sources.\n";

    #[test]
    fn parse_maps_frontmatter_and_body() {
        let imported = import_skill_markdown(REVIEW).expect("parse");
        assert_eq!(imported.spec().id.as_str(), "skill.reviewer");
        assert_eq!(imported.spec().description.as_ref(), "Review notes");
        assert_eq!(
            imported.spec().instructions[0].text(),
            "Cite primary sources."
        );
        assert_eq!(
            imported.spec().activation,
            CapabilityActivation::Application
        );
        assert!(imported.spec().context_providers.is_empty());
    }

    #[test]
    fn digest_covers_content_not_a_path() {
        let left = import_skill_markdown(REVIEW).expect("left");
        let right = import_skill_markdown(REVIEW).expect("right");
        assert_eq!(left.content_digest(), right.content_digest());
        let changed = import_skill_markdown(
            "---\nname: reviewer\ndescription: Review notes\n---\nCite other sources.\n",
        )
        .expect("changed");
        assert_ne!(left.content_digest(), changed.content_digest());
    }

    #[test]
    fn catalog_is_default_off() {
        let source = SkillMarkdown::from_text(REVIEW);
        let disabled = SkillCatalog::try_import(false, std::slice::from_ref(&source)).expect("off");
        assert!(disabled.specs().is_empty());
        let imported = SkillCatalog::try_import(true, &[source]).expect("on");
        assert_eq!(imported.specs().len(), 1);
    }

    #[test]
    fn scripts_and_references_fail_closed() {
        let error = import_skill_markdown(
            "---\nname: reviewer\nscripts: ./scripts\n---\nCite primary sources.\n",
        )
        .expect_err("scripts");
        assert_eq!(error.reason(), "scripts_and_references_are_unsupported");
    }
}
