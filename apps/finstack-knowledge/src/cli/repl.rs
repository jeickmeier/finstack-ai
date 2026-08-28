//! The `repl` command: a readline loop over one session.
//!
//! Driven as a function over injected input/output so tests need no PTY.
//! Each turn is one run on lane `main`; `ask_user` interactions raised by
//! the elicitation toolset are rendered, answered from the same input, and
//! resolved; an interrupt cancels the in-flight run and returns to the
//! prompt. `:q` quits, `:session` prints the session id.

use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use finstack_ai::{Agent, AgentRunRequest, Lane};
use finstack_ai_kernel::{
    AuthorizationEvidence, ContentBlock, InteractionResolution, RawJson, RunEventBody,
    RunSecurityContext, SessionId,
};

use super::render::render_markup_plain;
use crate::{
    KnowledgeConfig, KnowledgeError, build_agent_with_journal, model_name, open_journal, security,
};

/// Run the repl until `:q` or end of input.
///
/// `interrupt` cancels the in-flight run when set mid-turn (the binary
/// wires Ctrl-C to it); the loop survives and returns to the prompt.
///
/// # Errors
///
/// Configuration and composition failures; per-turn run failures are
/// reported to `output` and do not end the loop.
pub fn run_repl<'a>(
    config: &'a KnowledgeConfig,
    session: Option<&'a str>,
    os_user: &'a str,
    input: &'a mut dyn BufRead,
    output: &'a mut dyn Write,
    interrupt: Arc<AtomicBool>,
) -> std::pin::Pin<Box<dyn Future<Output = Result<(), KnowledgeError>> + 'a>> {
    Box::pin(run_repl_inner(config, session, os_user, input, output, interrupt))
}

async fn run_repl_inner(
    config: &KnowledgeConfig,
    session: Option<&str>,
    os_user: &str,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    interrupt: Arc<AtomicBool>,
) -> Result<(), KnowledgeError> {
    let journal = open_journal(config)?;
    let agent = build_agent_with_journal(config, journal.clone()).await?;
    let security = security(os_user)?;

    let (session, lane) = match session {
        None => {
            let session = finstack_ai::Session::create(journal, "local")
                .await
                .map_err(compose)?;
            let lane = session.lane("main").await.map_err(compose)?;
            (session, lane)
        }
        Some(id) => {
            let id = SessionId::parse(id).map_err(|_| KnowledgeError::Config {
                reason: "session_id_invalid",
            })?;
            let session = finstack_ai::Session::open(journal, id, "local")
                .await
                .map_err(compose)?;
            let lane = session.lane("main").await.map_err(compose)?;
            (session, lane)
        }
    };
    let session_id = session.session_id().to_string();
    write_line(output, &format!("session: {session_id}"));

    loop {
        write_prompt(output);
        let Some(raw) = read_line(input) else { break };
        let trimmed = raw.trim().to_owned();
        match trimmed.as_str() {
            "" => {}
            ":q" => break,
            ":session" => write_line(output, &session_id),
            question => {
                run_turn(
                    &agent, &lane, &security, config, question, input, output, &interrupt,
                )
                .await?;
            }
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "one internal call site; grouping would only rename the arguments"
)]
async fn run_turn(
    agent: &Agent,
    lane: &Lane,
    security: &RunSecurityContext,
    config: &KnowledgeConfig,
    question: &str,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    interrupt: &AtomicBool,
) -> Result<(), KnowledgeError> {
    let request = AgentRunRequest::try_new(model_name(config)?, question, security.clone())
        .map_err(compose)?;
    let run = match lane.run(agent, request) {
        Ok(run) => run,
        Err(error) => {
            write_line(output, &format!("run failed: {error}"));
            return Ok(());
        }
    };

    let mut cancelled = false;
    if interrupt.swap(false, Ordering::SeqCst) {
        let _ = run.cancel().await;
        cancelled = true;
    }
    let mut answer = String::new();
    loop {
        let batch = match run.next_event_batch().await {
            Ok(Some(batch)) => batch,
            Ok(None) => break,
            Err(error) => {
                write_line(output, &format!("stream ended: {error}"));
                break;
            }
        };
        for event in batch.events() {
            match event.body() {
                RunEventBody::ModelTextDelta(delta) => answer.push_str(delta.text()),
                RunEventBody::InteractionRequested(request) => {
                    let prompt: String = request
                        .prompt()
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text(text) => Some(text.text()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    write_line(output, &format!("[interaction] {prompt}"));
                    write_prompt(output);
                    let reply = read_line(input).unwrap_or_default();
                    let payload = serde_json::json!({ "answer": reply.trim() });
                    let resolution = InteractionResolution::try_new(
                        request.interaction_id(),
                        format!("know-repl-{}", request.interaction_id()),
                        security.principal().clone(),
                        AuthorizationEvidence::try_new(
                            security.authorization_policy_version(),
                            security.authorization_decision_id(),
                        )
                        .map_err(compose)?,
                        RawJson::parse(payload.to_string().as_bytes()).map_err(compose)?,
                        None::<&str>,
                    )
                    .map_err(compose)?;
                    if let Err(error) = run.resolve_interaction(resolution).await {
                        write_line(output, &format!("interaction failed: {error}"));
                    }
                }
                RunEventBody::RunCancelled { .. } => cancelled = true,
                _ => {}
            }
        }
        if interrupt.swap(false, Ordering::SeqCst) {
            let _ = run.cancel().await;
            cancelled = true;
        }
    }
    match run.result().await {
        Ok(result) => {
            let text = if answer.is_empty() { result.text() } else { answer };
            write_line(output, &render_markup_plain(&text));
        }
        Err(error) if cancelled => {
            let _ = error;
            write_line(output, "run cancelled");
        }
        Err(error) => write_line(output, &format!("run failed: {error}")),
    }
    Ok(())
}

fn write_prompt(output: &mut dyn Write) {
    let _ = write!(output, "know> ");
    let _ = output.flush();
}

fn write_line(output: &mut dyn Write, line: &str) {
    let _ = writeln!(output, "{line}");
    let _ = output.flush();
}

fn read_line(input: &mut dyn BufRead) -> Option<String> {
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(count) if count > 0 => Some(line),
        Ok(_) | Err(_) => None,
    }
}

fn compose(error: impl std::fmt::Display) -> KnowledgeError {
    KnowledgeError::Compose {
        reason: error.to_string(),
    }
}
