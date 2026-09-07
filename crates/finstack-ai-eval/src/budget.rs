//! Unit-aware admission spending across subject and grader receipts.
use crate::{
    EVAL_BUDGET_EXHAUSTED, EVAL_COST_UNKNOWN, EvalError, EvalSpec, Reconciliation, StoreSnapshot,
};

pub(crate) fn admission_budget(spec: &EvalSpec, snapshot: &StoreSnapshot) -> Result<(), EvalError> {
    let Some(threshold) = spec.limits.budget_micros else {
        return Ok(());
    };
    let mut spent = 0_u64;
    let usages = snapshot
        .attempts
        .values()
        .flatten()
        .map(|record| (record.reconciliation, &record.usage))
        .chain(
            snapshot
                .graders
                .values()
                .filter_map(|record| record.outcome.as_ref())
                .map(|outcome| (outcome.reconciliation, &outcome.usage)),
        );
    for (reconciliation, usage) in usages {
        if reconciliation == Reconciliation::NoAdmission {
            continue;
        }
        let cost = usage
            .cost
            .as_ref()
            .filter(|cost| Some(cost.unit.as_ref()) == spec.limits.budget_unit.as_deref())
            .ok_or_else(|| {
                EvalError::new(
                    EVAL_COST_UNKNOWN,
                    "finite budget requires complete compatible reported cost",
                )
            })?;
        spent = spent.checked_add(cost.micros).ok_or_else(|| {
            EvalError::new(crate::EVAL_ARITHMETIC_OVERFLOW, "experiment cost overflow")
        })?;
    }
    if spent >= threshold {
        return Err(EvalError::new(
            EVAL_BUDGET_EXHAUSTED,
            "experiment admission spending threshold reached",
        ));
    }
    Ok(())
}

pub(crate) fn check_pricing_policy(
    spec: &EvalSpec,
    agent: &finstack_ai::Agent,
) -> Result<(), EvalError> {
    if spec.limits.budget_micros.is_some()
        && agent
            .resolved()
            .spec()
            .and_then(|spec| spec.limits.max_cost.as_ref())
            .is_none_or(|policy| Some(policy.unit()) != spec.limits.budget_unit.as_deref())
    {
        return Err(EvalError::new(
            EVAL_COST_UNKNOWN,
            "finite budget requires an accepted compatible pricing policy",
        ));
    }
    Ok(())
}
