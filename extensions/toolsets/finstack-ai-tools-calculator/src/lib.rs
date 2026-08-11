//! Deterministic bounded calculator implementation of the public `Toolset` port.

#![warn(missing_docs)]

use std::sync::Arc;

use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ErrorCategory, Metadata, PortFuture, RawJson,
    RetrySafety, SideEffectClass, ToolCallContext, ToolError, ToolEventStream, ToolExecutionMode,
    ToolId, ToolResult, ToolSpec, ToolStreamItem, Toolset, ToolsetDescriptor, ValidatedToolCall,
};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const TOOL_ID: &str = "finstack.tools.calculator";
const TOOL_NAME: &str = "calculator";
const MAX_OPERANDS: usize = 1_024;

/// Stable invalid-calculator-argument code.
pub const CALCULATOR_INVALID_ARGUMENTS: &str = "calculator_invalid_arguments";
/// Stable arithmetic-domain failure code.
pub const CALCULATOR_ARITHMETIC_ERROR: &str = "calculator_arithmetic_error";

/// Arithmetic operation supported by [`evaluate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Sum every operand; the empty sum is zero.
    Add,
    /// Subtract every later operand from the first.
    Subtract,
    /// Multiply every operand; the empty product is one.
    Multiply,
    /// Divide the first operand by every later operand.
    Divide,
}

/// Pure calculator failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CalculatorError {
    /// The operand collection or operation arity is invalid.
    #[error("{CALCULATOR_INVALID_ARGUMENTS}: {reason}")]
    InvalidArguments {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// The operation produced division by zero or a non-finite value.
    #[error("{CALCULATOR_ARITHMETIC_ERROR}: {reason}")]
    Arithmetic {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// A checked-in public descriptor could not be constructed.
    #[error("calculator_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Evaluate one bounded arithmetic operation.
///
/// # Errors
///
/// Rejects more than 1,024 operands, invalid operation arity, division by
/// zero, non-finite inputs, and non-finite results.
pub fn evaluate(operation: Operation, operands: &[f64]) -> Result<f64, CalculatorError> {
    if operands.len() > MAX_OPERANDS || operands.iter().any(|value| !value.is_finite()) {
        return Err(CalculatorError::InvalidArguments {
            reason: "invalid_operand_collection",
        });
    }
    let result = match operation {
        Operation::Add => operands.iter().copied().sum(),
        Operation::Multiply => operands.iter().copied().product(),
        Operation::Subtract => {
            let Some((first, rest)) = operands.split_first() else {
                return Err(CalculatorError::InvalidArguments {
                    reason: "subtract_requires_an_operand",
                });
            };
            rest.iter().fold(*first, |value, operand| value - operand)
        }
        Operation::Divide => {
            let Some((first, rest)) = operands.split_first() else {
                return Err(CalculatorError::InvalidArguments {
                    reason: "divide_requires_an_operand",
                });
            };
            if rest.contains(&0.0) {
                return Err(CalculatorError::Arithmetic {
                    reason: "division_by_zero",
                });
            }
            rest.iter().fold(*first, |value, operand| value / operand)
        }
    };
    if result.is_finite() {
        Ok(result)
    } else {
        Err(CalculatorError::Arithmetic {
            reason: "non_finite_result",
        })
    }
}

/// Pure calculator toolset with one immutable, cached tool specification.
#[derive(Debug, Clone)]
pub struct CalculatorToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    tool_id: ToolId,
}

impl CalculatorToolset {
    /// Construct the calculator and its reusable public specification.
    ///
    /// # Errors
    ///
    /// Returns a configuration error only if a checked-in identity or schema
    /// constant is invalid.
    pub fn try_new() -> Result<Self, CalculatorError> {
        let tool_id = ToolId::parse(TOOL_ID).map_err(|_| CalculatorError::Configuration {
            reason: "invalid_tool_id",
        })?;
        let input_schema = RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"operands":{"items":{"type":"number"},"maxItems":1024,"type":"array"},"operation":{"enum":["add","subtract","multiply","divide"],"type":"string"}},"required":["operation","operands"],"type":"object"}"#,
        )
        .map_err(|_| CalculatorError::Configuration {
            reason: "invalid_input_schema",
        })?;
        let output_schema = RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"result":{"type":"number"}},"required":["result"],"type":"object"}"#,
        )
        .map_err(|_| CalculatorError::Configuration {
            reason: "invalid_output_schema",
        })?;
        let spec = ToolSpec {
            id: tool_id.clone(),
            model_name: Arc::from(TOOL_NAME),
            title: Arc::from("Calculator"),
            description: Arc::from("Perform bounded deterministic arithmetic over finite numbers."),
            input_schema,
            output_schema: Some(output_schema),
            execution: ToolExecutionMode::Parallel,
            side_effect: SideEffectClass::ReadOnly,
            retry_safety: RetrySafety::SafeToRetry,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
            max_result_bytes: 1_024,
            metadata: Metadata::empty(),
        };
        spec.validate()
            .map_err(|_| CalculatorError::Configuration {
                reason: "invalid_tool_spec",
            })?;
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-calculator"),
                metadata: Metadata::empty(),
            },
            tools: Arc::from([spec]),
            tool_id,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalculatorArguments {
    operation: Operation,
    operands: Vec<f64>,
}

impl Toolset for CalculatorToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let expected_id = self.tool_id.clone();
        Box::pin(async move {
            validate_call_context(&ctx, &call, &expected_id, TOOL_NAME)?;
            let arguments: CalculatorArguments =
                serde_json::from_slice(call.call.arguments().as_bytes()).map_err(|_| {
                    tool_error(
                        CALCULATOR_INVALID_ARGUMENTS,
                        ErrorCategory::Validation,
                        "calculator arguments are invalid",
                    )
                })?;
            let result = evaluate(arguments.operation, &arguments.operands).map_err(|error| {
                let (code, category) = match error {
                    CalculatorError::InvalidArguments { .. } => {
                        (CALCULATOR_INVALID_ARGUMENTS, ErrorCategory::Validation)
                    }
                    CalculatorError::Arithmetic { .. } => {
                        (CALCULATOR_ARITHMETIC_ERROR, ErrorCategory::Tool)
                    }
                    CalculatorError::Configuration { .. } => {
                        (CALCULATOR_ARITHMETIC_ERROR, ErrorCategory::Internal)
                    }
                };
                tool_error(code, category, "calculator evaluation failed")
            })?;
            let output =
                serde_json::to_vec(&serde_json::json!({ "result": result })).map_err(|_| {
                    tool_error(
                        CALCULATOR_ARITHMETIC_ERROR,
                        ErrorCategory::Internal,
                        "calculator result serialization failed",
                    )
                })?;
            let result = ToolResult {
                output: RawJson::parse(output).map_err(|_| {
                    tool_error(
                        CALCULATOR_ARITHMETIC_ERROR,
                        ErrorCategory::Internal,
                        "calculator result normalization failed",
                    )
                })?,
                is_error: false,
            };
            Ok(Box::pin(stream::once(async move {
                Ok(ToolStreamItem::Completed(result))
            })) as ToolEventStream)
        })
    }
}

fn validate_call_context(
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
    expected_id: &ToolId,
    expected_name: &str,
) -> Result<(), ToolError> {
    if call.tool_id != *expected_id || call.call.tool_name() != expected_name {
        return Err(tool_error(
            CALCULATOR_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "calculator call identity is invalid",
        ));
    }
    let locator_scope = ctx.run.locator.tenant_scope.as_ref();
    if ctx
        .run
        .authorization
        .principal
        .tenant_scope()
        .is_some_and(|scope| scope != locator_scope)
    {
        return Err(tool_error(
            CALCULATOR_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "calculator principal scope does not match the committed effect",
        ));
    }
    Ok(())
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty())
        .unwrap_or_else(|error| error)
}

#[cfg(test)]
mod tests;
