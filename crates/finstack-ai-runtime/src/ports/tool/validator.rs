use core::fmt;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{RawJson, ValidationIssue, ValidationOutcome};
use jsonschema::{Draft, Resource, Retrieve, Uri};

use crate::ports::PortObject;

use super::error::ToolError;

const VALIDATION_ISSUE_MAX: usize = 64;

/// Validator-independent compiled JSON Schema adapter.
pub trait ToolValidator: PortObject {
    /// Validate canonical JSON without recompiling the schema.
    fn validate(&self, instance: &RawJson) -> ValidationOutcome;
}

/// Compile-once tool-schema abstraction.
pub trait ToolValidatorCompiler: PortObject {
    /// Compile one schema against an explicit offline resource registry.
    ///
    /// # Errors
    ///
    /// Returns a stable registration error when the schema or an explicit
    /// resource cannot be parsed, resolved, or compiled.
    fn compile(
        &self,
        schema: &RawJson,
        resources: &BTreeMap<Arc<str>, RawJson>,
    ) -> Result<Arc<dyn ToolValidator>, ToolError>;
}

/// Default offline Draft-2020-12 validator compiler.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonSchemaToolValidatorCompiler;

thread_local! {
    static COMPILE_COUNT: Cell<u64> = const { Cell::new(0) };
}

impl JsonSchemaToolValidatorCompiler {
    /// Compiles observed on this thread since the last reset.
    ///
    /// NFR-PERF-007 conformance uses this to prove construction compiles
    /// schemas once and later scripted calls do not compile again.
    #[must_use]
    pub fn thread_compile_count() -> u64 {
        COMPILE_COUNT.with(Cell::get)
    }

    /// Reset the thread-local compile counter to zero.
    pub fn reset_thread_compile_count() {
        COMPILE_COUNT.with(|count| count.set(0));
    }
}

impl ToolValidatorCompiler for JsonSchemaToolValidatorCompiler {
    fn compile(
        &self,
        schema: &RawJson,
        resources: &BTreeMap<Arc<str>, RawJson>,
    ) -> Result<Arc<dyn ToolValidator>, ToolError> {
        COMPILE_COUNT.with(|count| count.set(count.get().saturating_add(1)));
        let schema = parse_json(schema)?;
        let mut compiled_resources = Vec::with_capacity(resources.len());
        for (uri, resource) in resources {
            compiled_resources.push((
                uri.to_string(),
                Resource::from_contents(parse_json(resource)?),
            ));
        }
        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            .with_resources(compiled_resources.into_iter())
            .with_retriever(DenyRetriever)
            .build(&schema)
            .map_err(|_| ToolError::registration("tool schema compilation failed"))?;
        Ok(Arc::new(JsonSchemaToolValidator { validator }))
    }
}

pub(super) fn parse_json(value: &RawJson) -> Result<serde_json::Value, ToolError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| ToolError::registration("canonical tool schema is invalid"))
}

#[derive(Debug)]
struct DenyRetriever;

impl Retrieve for DenyRetriever {
    fn retrieve(
        &self,
        _uri: &Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(DeniedRetrieval))
    }
}

#[derive(Debug)]
struct DeniedRetrieval;

impl fmt::Display for DeniedRetrieval {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ambient schema retrieval is disabled")
    }
}

impl std::error::Error for DeniedRetrieval {}

struct JsonSchemaToolValidator {
    validator: jsonschema::Validator,
}

impl ToolValidator for JsonSchemaToolValidator {
    fn validate(&self, instance: &RawJson) -> ValidationOutcome {
        let Ok(instance) = serde_json::from_slice(instance.as_bytes()) else {
            return invalid_validator_outcome("canonical JSON is invalid");
        };
        let mut issues = self
            .validator
            .iter_errors(&instance)
            .take(VALIDATION_ISSUE_MAX)
            .filter_map(|error| {
                let schema_path = error.schema_path().as_str().to_owned();
                let keyword = schema_path
                    .rsplit('/')
                    .find(|part| !part.is_empty())
                    .map(Arc::<str>::from);
                ValidationIssue::try_new(
                    Arc::<str>::from(error.instance_path().as_str()),
                    Arc::<str>::from(schema_path),
                    keyword,
                    Arc::<str>::from("value does not satisfy the JSON Schema constraint"),
                )
                .ok()
            })
            .collect::<Vec<_>>();
        if issues.is_empty() {
            return ValidationOutcome::Valid;
        }
        issues.sort_by(|left, right| {
            (&left.instance_path, &left.schema_path, &left.keyword).cmp(&(
                &right.instance_path,
                &right.schema_path,
                &right.keyword,
            ))
        });
        issues.dedup();
        let feedback = validation_feedback(&issues);
        ValidationOutcome::try_invalid(Arc::from(issues), Arc::<str>::from(feedback))
            .unwrap_or_else(|_| invalid_validator_outcome("validation failed"))
    }
}

fn invalid_validator_outcome(message: &str) -> ValidationOutcome {
    let issue = ValidationIssue {
        instance_path: Arc::from(""),
        schema_path: Arc::from(""),
        keyword: None,
        message: Arc::from(message),
    };
    ValidationOutcome::try_invalid(
        Arc::from([issue]),
        Arc::<str>::from("JSON Schema validation failed"),
    )
    .unwrap_or(ValidationOutcome::Invalid {
        issues: Arc::from([ValidationIssue {
            instance_path: Arc::from(""),
            schema_path: Arc::from(""),
            keyword: None,
            message: Arc::from("validation failed"),
        }]),
        feedback: Arc::from("JSON Schema validation failed"),
    })
}

fn validation_feedback(issues: &[ValidationIssue]) -> String {
    let mut feedback = String::from("JSON Schema validation failed:");
    for issue in issues {
        if feedback.len() > 16_000 {
            break;
        }
        feedback.push(' ');
        feedback.push_str(&issue.instance_path);
        if let Some(keyword) = &issue.keyword {
            feedback.push_str(" (");
            feedback.push_str(keyword);
            feedback.push(')');
        }
        feedback.push(';');
    }
    feedback
}
