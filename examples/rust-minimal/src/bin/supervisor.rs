//! Offline supervisor: exactly two specialists, committed lineage and cancellation.
use std::sync::Arc;
use std::time::Duration;

use finstack_ai::runtime::ports::journal::{JournalStore, LoadRequest};
use finstack_ai::{Agent, AgentRun, AgentRunError, AgentRunRequest, ChildRunPolicy};
use finstack_ai_kernel::{
    AgentId, BundleId, ChildPlacement, ComponentId, ComponentRef, RecordBody,
};
use finstack_ai_native_examples::{BoxError, PREVIEW_VERSION, controlled, security};
use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

fn component(id: &str) -> Result<ComponentRef, BoxError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id)?,
        Some(PREVIEW_VERSION),
    ))
}

async fn definition(
    endpoint: &str,
    id: &str,
    store: Arc<dyn JournalStore>,
    supervisor: bool,
) -> Result<Agent, BoxError> {
    let model = Arc::new(OllamaProvider::try_new(
        OllamaConfig::try_new(endpoint)?,
        vec![OllamaModelConfig::try_new(
            "specialist",
            1_048_576,
            1_048_576,
            512,
            512,
            64,
        )?],
    )?);
    Ok(Agent::builder(
        AgentId::parse(id)?,
        BundleId::parse("recipe.supervisor")?,
        (component("recipe.model.ollama")?, model),
        (component("recipe.store.memory")?, store),
    )
    .policy(finstack_ai::RunPolicy {
        child_runs: if supervisor {
            ChildRunPolicy::Allow { max_depth: 1 }
        } else {
            ChildRunPolicy::Deny
        },
        ..finstack_ai::RunPolicy::default()
    })
    .build()
    .await?)
}

fn request(text: &str) -> Result<AgentRunRequest, BoxError> {
    let mut request = AgentRunRequest::try_new(
        finstack_ai::runtime::ports::model::ModelName::try_new("specialist")?,
        text,
        security()?,
    )?;
    request.timeout = Duration::from_secs(10);
    request.max_cycles = 1;
    Ok(request)
}

async fn drain(run: AgentRun) -> Result<(), BoxError> {
    while run.next_event_batch().await?.is_some() {}
    Ok(())
}

async fn check_lineage(
    store: &dyn JournalStore,
    parent: &AgentRun,
    child: &AgentRun,
) -> Result<(), BoxError> {
    let loaded = store
        .load(LoadRequest {
            session_id: child.locator().session_id,
        })
        .await?;
    let accepted = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .find_map(|record| match record.body() {
            RecordBody::RunAccepted(accepted) => Some(accepted),
            _ => None,
        })
        .ok_or("child admission missing")?;
    assert_eq!(
        accepted.relation().parent_run_id(),
        Some(parent.locator().run_id)
    );
    assert_eq!(accepted.relation().root_run_id(), parent.locator().run_id);
    assert_eq!(accepted.relation().depth(), 1);
    assert!(accepted.relation().parent_effect_id().is_some());
    println!(
        "lineage: {} -> {}",
        parent.locator().run_id,
        child.locator().run_id
    );
    Ok(())
}

async fn check_prepared(store: &dyn JournalStore, parent: &AgentRun) -> Result<(), BoxError> {
    let history = store
        .load(LoadRequest {
            session_id: parent.locator().session_id,
        })
        .await?;
    assert_eq!(
        history
            .committed_batches
            .iter()
            .flat_map(|batch| batch.records.iter())
            .filter(|record| matches!(record.body(), RecordBody::ChildRunPrepared(_)))
            .count(),
        2
    );
    Ok(())
}

async fn exercise(cancel: bool) -> Result<(), BoxError> {
    let store: Arc<dyn JournalStore> = Arc::new(MemoryJournalStore::try_new(MemoryStoreLimits {
        sessions: 8,
        batches_per_session: 128,
        records_per_session: 1024,
        snapshot_bytes: 16_384,
    })?);
    let parent_server = controlled::serve().await?;
    let parent_agent = definition(
        &parent_server.endpoint,
        "recipe.supervisor",
        Arc::clone(&store),
        true,
    )
    .await?;
    let parent = parent_agent.start(request("Collect two specialist results.")?)?;
    let parent_stream = tokio::spawn(drain(parent.clone()));
    parent_server.request.await?;
    let mut children = Vec::new();
    let mut servers = Vec::new();
    let mut streams = Vec::new();
    for (id, task) in [
        ("recipe.specialist.policy", "Extract the retention policy."),
        ("recipe.specialist.risk", "Identify the review risk."),
    ] {
        let server = controlled::serve().await?;
        let specialist = definition(&server.endpoint, id, Arc::clone(&store), false).await?;
        let child = Box::pin(parent.start_child(
            &specialist,
            request(task)?,
            ChildPlacement::IsolatedChildSession,
            None,
        ))
        .await?;
        streams.push(tokio::spawn(drain(child.clone())));
        server.request.await?;
        check_lineage(store.as_ref(), &parent, &child).await?;
        children.push(child);
        servers.push((server.reply, server.task));
    }
    if cancel {
        parent.cancel().await?;
        assert!(matches!(
            parent.result().await,
            Err(AgentRunError::Cancelled)
        ));
        for child in children {
            assert!(matches!(
                child.result().await,
                Err(AgentRunError::Cancelled)
            ));
        }
        for (_, task) in servers {
            task.abort();
            let _ = task.await;
        }
        parent_server.task.abort();
        let _ = parent_server.task.await;
    } else {
        let mut results = Vec::new();
        for ((reply, server), (child, answer)) in servers.into_iter().zip(
            children
                .into_iter()
                .zip(["retention: seven years", "risk: annual review"]),
        ) {
            reply
                .send(answer.to_owned())
                .map_err(|_| "specialist closed")?;
            let result = child.result().await?;
            assert_eq!(result.text(), answer);
            results.push(result.text());
            server.await??;
        }
        let summary = results.join("; ");
        parent_server
            .reply
            .send(summary.clone())
            .map_err(|_| "parent closed")?;
        assert_eq!(parent.result().await?.text(), summary);
        parent_server.task.await??;
    }
    parent_stream.await??;
    for stream in streams {
        stream.await??;
    }
    check_prepared(store.as_ref(), &parent).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tokio::time::timeout(Duration::from_secs(30), async {
        exercise(false).await?;
        exercise(true).await
    })
    .await??;
    println!("supervisor: two results, committed lineage, bounded children, cancellation verified");
    Ok(())
}
