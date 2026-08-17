//! Shared references, diagnostics, sensitivity, and usage (TDD §5.3 / §7.2).

mod allocated_ids;
mod diagnostic;
mod error;
mod handles;
mod identity;
mod usage;

#[cfg(test)]
mod tests;

pub use allocated_ids::AllocatedIds;
pub use diagnostic::{Diagnostic, DiagnosticSeverity, Sensitivity};
pub use error::RefsError;
pub(crate) use error::{validated_label, validated_text};
pub use handles::{ArtifactRef, ExternalHandleRef};
pub use identity::{
    AssigneeHint, AuthorizationEvidence, ComponentRef, MiddlewareRef, PrincipalRef, Version,
};
pub use usage::{CostAmount, Usage};
