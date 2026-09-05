use super::error::{TOOL_POLICY_DENIED, ToolError};
use super::types::ToolCallContext;

/// Reject a tool call whose principal tenant scope disagrees with the locator.
///
/// A principal with no tenant scope is accepted. A present scope must equal
/// `ctx.run.locator.tenant_scope`. This is the shared
/// predicate used by shell and filesystem today.
///
/// # Errors
///
/// Returns `tool_policy_denied` when the scopes differ.
///
/// # Examples
///
/// ```
/// use finstack_ai_kernel::{
///     EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RunId, SessionId, ToolBatchId,
///     ToolCallId,
/// };
/// use finstack_ai_runtime::ports::model::{CancellationSignal, RunCallContext};
/// use finstack_ai_runtime::ports::tool::{ToolCallContext, verify_authority};
/// use finstack_ai_runtime::ports::model::AuthorizationContext;
///
/// let locator = OperationLocator::try_new(
///     "tenant-a",
///     SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session"),
///     LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane"),
///     RunId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("run"),
/// )
/// .expect("locator");
/// let ctx = ToolCallContext {
///     run: RunCallContext {
///         locator,
///         authorization: AuthorizationContext {
///             principal: PrincipalRef::try_new("oidc", "user-1", Some("tenant-a")).expect("principal"),
///             authentication_method: "fixture".into(),
///             assurance_level: "high".into(),
///             roles: [].into(),
///             permitted_scopes: ["tenant-a".into()].into(),
///             safe_claims: Metadata::empty(),
///             policy_version: "v1".into(),
///             decision_id: "decision-1".into(),
///         },
///         effect_id: EffectId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("effect"),
///         attempt: 1,
///         deadline: None,
///         budget_scope_id: None,
///         cancellation: CancellationSignal::new(),
///         relation_depth: 0,
///     },
///     tool_batch_id: ToolBatchId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("batch"),
///     tool_call_id: ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("call"),
/// };
/// assert!(verify_authority(&ctx).is_ok());
/// ```
pub fn verify_authority(ctx: &ToolCallContext) -> Result<(), ToolError> {
    if ctx
        .run
        .authorization
        .principal
        .tenant_scope()
        .is_some_and(|scope| scope != ctx.run.locator.tenant_scope.as_ref())
    {
        return Err(ToolError::stable(
            TOOL_POLICY_DENIED,
            "principal scope does not match the committed effect",
        ));
    }
    Ok(())
}
