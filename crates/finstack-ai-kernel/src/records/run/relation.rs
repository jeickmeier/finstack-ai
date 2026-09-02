//! Run relation kinds and lineage edges.

use serde::de;
use serde::{Deserialize, Serialize};

use crate::primitives::{EffectId, RunId};

use super::error::RunError;
use super::validate::validate_relation_shape;
use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::primitives::BudgetScopeId;
use crate::primitives::validated_label;
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
    /// # Arguments
    ///
    /// * `root_run_id` - Root of the lineage tree.
    /// * `parent_run_id` - Immediate parent run, or `None` for a root.
    /// * `parent_effect_id` - Parent invocation effect, required for child kinds.
    /// * `kind` - Lineage kind. [`RunRelationKind::Root`] must have no parent.
    /// * `depth` - Distance from the root; roots are `0` and must stay ≤
    ///   [`crate::MAX_RUN_RELATION_DEPTH`].
    /// * `budget_scope_id` - Optional shared-budget scope inherited by the child.
    /// * `external_work_ref` - Optional external work label; `None` omits it.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] for invalid root/parent shape, depth, or labels.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{RunId, RunRelation, RunRelationKind};
    ///
    /// let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// let relation = RunRelation::try_new(
    ///     run,
    ///     None,
    ///     None,
    ///     RunRelationKind::Root,
    ///     0,
    ///     None,
    ///     None::<&str>,
    /// )
    /// .expect("relation");
    /// assert_eq!(relation.kind(), RunRelationKind::Root);
    /// ```
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
        let external_work_ref = external_work_ref
            .map(|value| validated_label(value.as_ref(), "external_work_ref"))
            .transpose()?;
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
    /// # Arguments
    ///
    /// * `run_id` - Identity of the root run. It is stored as both `run_id` and
    ///   `root_run_id`.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] only if label validation fails (not expected for roots).
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{RunId, RunRelation, RunRelationKind};
    ///
    /// let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// let relation = RunRelation::root(run).expect("root");
    /// assert_eq!(relation.kind(), RunRelationKind::Root);
    /// assert_eq!(relation.depth(), 0);
    /// ```
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
