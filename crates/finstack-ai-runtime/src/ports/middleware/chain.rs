use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{ComponentId, Digest};

use super::error::MiddlewareError;
use super::port::Middleware;
use super::types::{MiddlewareDescriptor, MiddlewareRole, Stage, canonical_bytes};
use super::{MIDDLEWARE_ORDER_CYCLE, MIDDLEWARE_RESOLUTION_INVALID};

/// One middleware component supplied to the resolver.
pub struct MiddlewareRegistration {
    /// Ready direct handle.
    pub middleware: Arc<dyn Middleware>,
}

/// Resolved direct middleware handle and immutable descriptor.
#[derive(Clone)]
pub struct ResolvedMiddleware {
    /// Ready direct handle.
    pub middleware: Arc<dyn Middleware>,
    /// Frozen descriptor.
    pub descriptor: MiddlewareDescriptor,
    /// Registration-order tiebreaker.
    pub registration_index: u32,
}

/// Deterministically resolved middleware chain.
pub struct ResolvedMiddlewareChain {
    chain_digest: Digest,
    stages: BTreeMap<Stage, Arc<[ResolvedMiddleware]>>,
}

impl ResolvedMiddlewareChain {
    /// Resolve descriptors once and reject duplicates, missing requirements, cycles, and invalid
    /// compaction ownership before any run starts.
    ///
    /// # Errors
    ///
    /// Returns a stable construction diagnostic on any invalid graph.
    pub fn try_new(registrations: Vec<MiddlewareRegistration>) -> Result<Self, MiddlewareError> {
        if registrations.len() > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_RESOLUTION_INVALID,
                "middleware chain exceeds the semantic component bound",
            ));
        }
        let mut resolved = Vec::with_capacity(registrations.len());
        let mut ids = BTreeSet::new();
        let mut compactors = 0_usize;
        for (index, registration) in registrations.into_iter().enumerate() {
            let descriptor = registration.middleware.descriptor();
            descriptor.validate()?;
            if descriptor.invocation.recovery
                != finstack_ai_kernel::InvocationRecovery::RecomputeSafe
            {
                return Err(MiddlewareError::stable(
                    MIDDLEWARE_RESOLUTION_INVALID,
                    "middleware must be recompute-safe until a reconcile port exists",
                ));
            }
            if !ids.insert(descriptor.invocation.component.clone()) {
                return Err(MiddlewareError::stable(
                    MIDDLEWARE_RESOLUTION_INVALID,
                    "middleware component is registered more than once",
                ));
            }
            if matches!(descriptor.role, MiddlewareRole::ContextCompactor { .. }) {
                compactors += 1;
            }
            resolved.push(ResolvedMiddleware {
                middleware: registration.middleware,
                descriptor,
                registration_index: u32::try_from(index).map_err(|_| {
                    MiddlewareError::stable(
                        MIDDLEWARE_RESOLUTION_INVALID,
                        "middleware registration index overflowed",
                    )
                })?,
            });
        }
        if compactors > 1 {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_RESOLUTION_INVALID,
                "more than one context-compaction owner is active",
            ));
        }
        for item in &resolved {
            for dependency in item
                .descriptor
                .order
                .before
                .iter()
                .chain(item.descriptor.order.after.iter())
            {
                if !ids.contains(dependency) {
                    return Err(MiddlewareError::stable(
                        MIDDLEWARE_RESOLUTION_INVALID,
                        "middleware ordering references a missing component",
                    ));
                }
            }
        }
        let descriptor_bytes = canonical_bytes(
            &resolved
                .iter()
                .map(|item| &item.descriptor)
                .collect::<Vec<_>>(),
        )?;
        let chain_digest = Digest::middleware_chain(&descriptor_bytes);
        let mut stages = BTreeMap::new();
        for stage in [
            Stage::BeforeRun,
            Stage::PrepareContext,
            Stage::BeforeModel,
            Stage::AfterModel,
            Stage::BeforeToolBatch,
            Stage::AfterToolBatch,
            Stage::BeforeFinalize,
        ] {
            let ordered = resolve_stage(stage, &resolved)?;
            stages.insert(stage, ordered.into());
        }
        Ok(Self {
            chain_digest,
            stages,
        })
    }

    /// Frozen chain digest.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.chain_digest
    }

    /// Resolved direct handles for one stage.
    #[must_use]
    pub fn stage(&self, stage: Stage) -> &[ResolvedMiddleware] {
        self.stages.get(&stage).map_or(&[], AsRef::as_ref)
    }
}

fn resolve_stage(
    stage: Stage,
    all: &[ResolvedMiddleware],
) -> Result<Vec<ResolvedMiddleware>, MiddlewareError> {
    let nodes = all
        .iter()
        .filter(|item| item.descriptor.stages.contains(stage))
        .cloned()
        .collect::<Vec<_>>();
    let node_ids = nodes
        .iter()
        .map(|item| item.descriptor.invocation.component.clone())
        .collect::<BTreeSet<_>>();
    let mut incoming = node_ids
        .iter()
        .cloned()
        .map(|id| (id, 0_usize))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = BTreeMap::<ComponentId, BTreeSet<ComponentId>>::new();
    let mut add_edge = |from: &ComponentId, to: &ComponentId| {
        if from != to
            && node_ids.contains(from)
            && node_ids.contains(to)
            && outgoing.entry(from.clone()).or_default().insert(to.clone())
        {
            *incoming.entry(to.clone()).or_default() += 1;
        }
    };
    for left in &nodes {
        for right in &nodes {
            if left.descriptor.order.tier < right.descriptor.order.tier {
                add_edge(
                    &left.descriptor.invocation.component,
                    &right.descriptor.invocation.component,
                );
            }
        }
        for after in left.descriptor.order.before.iter() {
            add_edge(&left.descriptor.invocation.component, after);
        }
        for before in left.descriptor.order.after.iter() {
            add_edge(before, &left.descriptor.invocation.component);
        }
    }
    let by_id = nodes
        .iter()
        .cloned()
        .map(|item| (item.descriptor.invocation.component.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let mut result = Vec::with_capacity(nodes.len());
    let mut remaining = node_ids;
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .filter(|id| incoming.get(*id).copied().unwrap_or_default() == 0)
            .filter_map(|id| {
                let item = by_id.get(id)?;
                Some((
                    (
                        item.descriptor.order.tier,
                        item.descriptor.order.priority,
                        item.registration_index,
                        id.clone(),
                    ),
                    id.clone(),
                ))
            })
            .min_by(|left, right| left.0.cmp(&right.0))
            .map(|(_, id)| id)
            .ok_or_else(|| {
                MiddlewareError::stable(
                    MIDDLEWARE_ORDER_CYCLE,
                    "middleware ordering contains a cycle",
                )
            })?;
        remaining.remove(&next);
        let Some(item) = by_id.get(&next) else {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_RESOLUTION_INVALID,
                "middleware graph is missing a resolved node",
            ));
        };
        result.push(item.clone());
        if let Some(targets) = outgoing.get(&next) {
            for target in targets {
                if let Some(value) = incoming.get_mut(target) {
                    *value = value.saturating_sub(1);
                }
            }
        }
    }
    Ok(result)
}
