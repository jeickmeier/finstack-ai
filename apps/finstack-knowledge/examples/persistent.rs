//! Persistent knowledge assistant: real SQLite/artifacts, typed results,
//! event streaming, reopening the same lane, and explicit cancellation.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai::runtime::artifact::{ArtifactMetadata, ArtifactScope, stage_required_artifact};
use finstack_ai::{
    AgentRun, AgentRunError, AgentRunOutput, AgentRunRequest, AttachmentInput, Session,
};
use finstack_ai_kernel::{Metadata, RawJson, Sensitivity, SessionId};
use finstack_ai_knowledge::{
    KnowledgeAgent, KnowledgeConfig, ProviderChoice, build_agent, model_name, open_artifact_store,
    open_journal, security,
};
use finstack_ai_native_examples::{BoxError, controlled};
use serde::Deserialize;

const SOURCE: &str = "Policy retention is seven years.";
const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"properties":{"answer":{"type":"string"},"sources":{"type":"array","items":{"type":"string"}}},"required":["answer","sources"]}"#;

#[derive(Debug, Deserialize)]
struct Answer {
    answer: String,
    sources: Vec<String>,
}

fn config(path: &std::path::Path, endpoint: String) -> KnowledgeConfig {
    KnowledgeConfig::new(
        path.to_owned(),
        ProviderChoice::Ollama {
            base_url: endpoint,
            model: "knowledge-offline".to_owned(),
        },
    )
}

async fn agent(config: &KnowledgeConfig) -> Result<KnowledgeAgent, BoxError> {
    let mut composition = build_agent(config).await?;
    composition.agent = composition
        .agent
        .try_with_output_schema(&RawJson::parse(SCHEMA)?)?;
    Ok(composition)
}

async fn collect(run: AgentRun) -> Result<BTreeSet<&'static str>, BoxError> {
    let mut kinds = BTreeSet::new();
    while let Some(batch) = run.next_event_batch().await? {
        for event in batch.events() {
            kinds.insert(event.kind().kind_name());
        }
    }
    Ok(kinds)
}

fn answer(output: &AgentRunOutput) -> Result<Answer, BoxError> {
    Ok(serde_json::from_slice(
        output
            .structured_json()
            .ok_or("missing validated JSON")?
            .as_bytes(),
    )?)
}

async fn first_turn(path: &std::path::Path) -> Result<SessionId, BoxError> {
    let server = controlled::serve().await?;
    let config = config(path, server.endpoint);
    let session = Session::create(open_journal(&config)?, "local").await?;
    let mut composition = agent(&config).await?;
    let lane = session.lane("main").await?;
    let artifacts = open_artifact_store(&config)?;
    let artifact = stage_required_artifact(
        artifacts.as_ref(),
        ArtifactScope {
            tenant_scope: Arc::from("local"),
            session_id: SessionId::from_bytes([0; 16]),
            run_id: None,
            sensitivity: Sensitivity::Internal,
        },
        format!("policy\n{SOURCE}\n").into_bytes().into(),
        ArtifactMetadata {
            kind: Arc::from("attachment"),
            media_type: Arc::from("text/csv"),
            name: Some(Arc::from("policy.csv")),
            attributes: Metadata::empty(),
        },
    )
    .await?;
    let mut request = AgentRunRequest::try_new(
        model_name(&config)?,
        "Read the attached policy.",
        security("recipe")?,
    )?;
    request.attachments = Arc::from([AttachmentInput { artifact }]);
    let run = lane.run(&composition.agent, request)?;
    let stream = tokio::spawn(collect(run.clone()));
    let received = server.request.await?;
    assert!(
        received.contains(SOURCE),
        "document bytes must reach the real provider request"
    );
    server
        .reply
        .send(r#"{"answer":"seven years","sources":["policy.csv"]}"#.to_owned())
        .map_err(|_| "reply closed")?;
    let output = run.result().await?;
    let result = answer(&output)?;
    assert_eq!(result.answer, "seven years");
    assert_eq!(result.sources, ["policy.csv"]);
    assert!(stream.await??.contains("run_completed"));
    assert!(output.structured_json().is_some());
    assert!(composition.search.maintain(256).await?.failures.is_empty());
    server.task.await??;
    Ok(session.session_id())
}

async fn reopened_turn(path: &std::path::Path, id: SessionId) -> Result<(), BoxError> {
    let server = controlled::serve().await?;
    let config = config(path, server.endpoint);
    let mut composition = agent(&config).await?;
    let session = Session::open(open_journal(&config)?, id, "local").await?;
    let lane = session.lane("main").await?;
    let run = lane.run(
        &composition.agent,
        AgentRunRequest::try_new(
            model_name(&config)?,
            "What was the retention policy?",
            security("recipe")?,
        )?,
    )?;
    let stream = tokio::spawn(collect(run.clone()));
    let received = server.request.await?;
    assert!(
        received.contains(SOURCE),
        "reopened journal attachments must resolve from disk"
    );
    assert!(
        received.contains("Read the attached policy."),
        "original session history must survive"
    );
    server
        .reply
        .send(r#"{"answer":"seven years","sources":["policy.csv"]}"#.to_owned())
        .map_err(|_| "reply closed")?;
    assert_eq!(answer(&run.result().await?)?.answer, "seven years");
    assert!(stream.await??.contains("run_completed"));
    assert!(composition.search.maintain(256).await?.failures.is_empty());
    server.task.await??;
    Ok(())
}

async fn cancellation(path: &std::path::Path, id: SessionId) -> Result<(), BoxError> {
    let server = controlled::serve().await?;
    let config = config(path, server.endpoint);
    let mut composition = agent(&config).await?;
    let session = Session::open(open_journal(&config)?, id, "local").await?;
    let lane = session.lane("main").await?;
    let run = lane.run(
        &composition.agent,
        AgentRunRequest::try_new(
            model_name(&config)?,
            "Wait for an explicit cancellation.",
            security("recipe")?,
        )?,
    )?;
    let stream = tokio::spawn(collect(run.clone()));
    let _received = server.request.await?;
    run.cancel().await?;
    assert!(matches!(run.result().await, Err(AgentRunError::Cancelled)));
    assert!(stream.await??.contains("run_cancelled"));
    assert!(composition.search.maintain(256).await?.failures.is_empty());
    server.task.abort();
    assert!(server.task.await.is_err_and(|error| error.is_cancelled()));
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let data = tempfile::tempdir()?;
    let session = Box::pin(first_turn(data.path())).await?;
    reopened_turn(data.path(), session).await?;
    cancellation(data.path(), session).await?;
    println!("persistent knowledge: disk reopen, structured output, events, cancellation verified");
    Ok(())
}
