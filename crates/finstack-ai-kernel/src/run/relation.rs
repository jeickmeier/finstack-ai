//! Run relation kinds and lineage edges.

use serde::de;
use serde::{Deserialize, Serialize};

use crate::ids::{EffectId, RunId};

use super::error::RunError;
use super::validate::validate_relation_shape;
use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::ids::BudgetScopeId;
use crate::refs::validated_label;
use std::sync::Arc;

/// How a run relates to its root/parent lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunRelationKind {
    /// Root run.
    Root,
    /// Child agent invocation.
    ChildAgent,
    /// Delegated agent invocation.
    DelegatedAgent,
    /// Workflow step.
    WorkflowStep,
}

/// Immutable run lineage relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunRelation {
    root_run_id: RunId,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_effect_id: Option<EffectId>,
    kind: RunRelationKind,
    depth: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_scope_id: Option<BudgetScopeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_work_ref: Option<Arc<str>>,
}

impl RunRelation {
    /// Construct a validated run relation.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] for invalid root/parent shape, depth, or labels.
    pub fn try_new(
        root_run_id: RunId,
        parent_run_id: Option<RunId>,
        parent_effect_id: Option<EffectId>,
        kind: RunRelationKind,
        depth: u16,
        budget_scope_id: Option<BudgetScopeId>,
        external_work_ref: Option<impl AsRef<str>>,
    ) -> Result<Self, RunError> {
        validate_relation_shape(kind, parent_run_id, parent_effect_id, depth)?;
        let external_work_ref = match external_work_ref {
            Some(value) => Some(validated_label(value.as_ref(), "external_work_ref")?),
            None => None,
        };
        Ok(Self {
            root_run_id,
            parent_run_id,
            parent_effect_id,
            kind,
            depth,
            budget_scope_id,
            external_work_ref,
        })
    }

    /// Construct a root relation for `run_id` at depth 0.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] only if label validation fails (not expected for roots).
    pub fn root(run_id: RunId) -> Result<Self, RunError> {
        Self::try_new(
            run_id,
            None,
            None,
            RunRelationKind::Root,
            0,
            None,
            None::<&str>,
        )
    }

    /// Borrow root run id.
    #[must_use]
    pub fn root_run_id(&self) -> RunId {
        self.root_run_id
    }

    /// Borrow parent run id.
    #[must_use]
    pub fn parent_run_id(&self) -> Option<RunId> {
        self.parent_run_id
    }

    /// Borrow parent effect id.
    #[must_use]
    pub fn parent_effect_id(&self) -> Option<EffectId> {
        self.parent_effect_id
    }

    /// Relation kind.
    #[must_use]
    pub fn kind(&self) -> RunRelationKind {
        self.kind
    }

    /// Depth.
    #[must_use]
    pub fn depth(&self) -> u16 {
        self.depth
    }

    /// Budget scope.
    #[must_use]
    pub fn budget_scope_id(&self) -> Option<BudgetScopeId> {
        self.budget_scope_id
    }

    /// External work ref.
    #[must_use]
    pub fn external_work_ref(&self) -> Option<&str> {
        self.external_work_ref.as_deref()
    }
}

impl<'de> Deserialize<'de> for RunRelation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            root_run_id: RunId,
            #[serde(default)]
            parent_run_id: Option<RunId>,
            #[serde(default)]
            parent_effect_id: Option<EffectId>,
            kind: RunRelationKind,
            depth: u16,
            #[serde(default)]
            budget_scope_id: Option<BudgetScopeId>,
            #[serde(default)]
            external_work_ref: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.root_run_id,
            wire.parent_run_id,
            wire.parent_effect_id,
            wire.kind,
            wire.depth,
            wire.budget_scope_id,
            wire.external_work_ref.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}
