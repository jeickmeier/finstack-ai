//! Published reference calculator component. Not the trusted native battery.

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

finstack_ai_guest_sdk::toolset_plugin!();

use exports::finstack::ai_toolset::toolset::{Guest, ToolCatalog, ToolResult, ToolSpec};
use finstack::ai_types::types::{CallContext, PluginError};
use finstack_ai_guest_sdk::{
    ToolSpecParts, catalog_digest, encode_json_result, parse_args, plugin_error,
    require_sanitized_context,
};
use serde::Deserialize;

const TOOL_ID: &str = "finstack.plugin.calculator";
const MAX_OPERANDS: usize = 1_024;
const CALCULATOR_INVALID_ARGUMENTS: &str = "calculator_invalid_arguments";
const CALCULATOR_ARITHMETIC_ERROR: &str = "calculator_arithmetic_error";

struct Calculator;

impl Guest for Calculator {
    fn list_tools() -> Result<ToolCatalog, PluginError> {
        let spec = calculator_spec();
        let digest = catalog_digest(&[to_parts(&spec)]).map_err(map_err)?;
        Ok(ToolCatalog {
            digest,
            tools: vec![spec],
        })
    }

    fn call(
        context: CallContext,
        tool_id: String,
        args_json: Vec<u8>,
    ) -> Result<ToolResult, PluginError> {
        require_sanitized_context(&context.tenant_scope, &context.authorization_decision_id)
            .map_err(map_err)?;
        if tool_id != TOOL_ID {
            return Err(map_err(plugin_error(
                "plugin_unknown_tool",
                "unknown tool id",
            )));
        }
        let args: CalculatorArgs = parse_args(&args_json).map_err(|_| {
            map_err(plugin_error(
                CALCULATOR_INVALID_ARGUMENTS,
                "calculator arguments are invalid",
            ))
        })?;
        let result = evaluate(args.operation, &args.operands)?;
        let content_json =
            encode_json_result(&serde_json::json!({ "result": result })).map_err(map_err)?;
        Ok(ToolResult {
            content_json,
            is_error: false,
        })
    }
}

export!(Calculator);

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalculatorArgs {
    operation: Operation,
    operands: Vec<f64>,
}

fn evaluate(operation: Operation, operands: &[f64]) -> Result<f64, PluginError> {
    if operands.len() > MAX_OPERANDS || operands.iter().any(|value| !value.is_finite()) {
        return Err(map_err(plugin_error(
            CALCULATOR_INVALID_ARGUMENTS,
            "invalid_operand_collection",
        )));
    }
    let result = match operation {
        Operation::Add => operands.iter().copied().sum(),
        Operation::Multiply => operands.iter().copied().product(),
        Operation::Subtract => {
            let Some((first, rest)) = operands.split_first() else {
                return Err(map_err(plugin_error(
                    CALCULATOR_INVALID_ARGUMENTS,
                    "subtract_requires_an_operand",
                )));
            };
            rest.iter().fold(*first, |value, operand| value - operand)
        }
        Operation::Divide => {
            let Some((first, rest)) = operands.split_first() else {
                return Err(map_err(plugin_error(
                    CALCULATOR_INVALID_ARGUMENTS,
                    "divide_requires_an_operand",
                )));
            };
            if rest.contains(&0.0) {
                return Err(map_err(plugin_error(
                    CALCULATOR_ARITHMETIC_ERROR,
                    "division_by_zero",
                )));
            }
            rest.iter().fold(*first, |value, operand| value / operand)
        }
    };
    if result.is_finite() {
        Ok(result)
    } else {
        Err(map_err(plugin_error(
            CALCULATOR_ARITHMETIC_ERROR,
            "non_finite_result",
        )))
    }
}

fn calculator_spec() -> ToolSpec {
    ToolSpec {
        id: TOOL_ID.to_owned(),
        model_name: "calculator".to_owned(),
        title: "Calculator".to_owned(),
        description: "Perform bounded deterministic arithmetic over finite numbers.".to_owned(),
        input_schema_json: br#"{"additionalProperties":false,"properties":{"operands":{"items":{"type":"number"},"maxItems":1024,"type":"array"},"operation":{"enum":["add","subtract","multiply","divide"],"type":"string"}},"required":["operation","operands"],"type":"object"}"#.to_vec(),
        output_schema_json: Some(
            br#"{"additionalProperties":false,"properties":{"result":{"type":"number"}},"required":["result"],"type":"object"}"#.to_vec(),
        ),
        execution_mode: "parallel".to_owned(),
        side_effect: "read_only".to_owned(),
        retry_safety: "safe_to_retry".to_owned(),
        approval_policy_json: br#"{"requirement":"not_required"}"#.to_vec(),
        max_result_bytes: 1024,
        metadata_json: b"{}".to_vec(),
    }
}

fn to_parts(spec: &ToolSpec) -> ToolSpecParts {
    ToolSpecParts {
        id: spec.id.clone(),
        model_name: spec.model_name.clone(),
        title: spec.title.clone(),
        description: spec.description.clone(),
        input_schema_json: spec.input_schema_json.clone(),
        output_schema_json: spec.output_schema_json.clone(),
        execution_mode: spec.execution_mode.clone(),
        side_effect: spec.side_effect.clone(),
        retry_safety: spec.retry_safety.clone(),
        approval_policy_json: spec.approval_policy_json.clone(),
        max_result_bytes: spec.max_result_bytes,
        metadata_json: spec.metadata_json.clone(),
    }
}

fn map_err(error: finstack_ai_guest_sdk::GuestError) -> PluginError {
    PluginError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    }
}
