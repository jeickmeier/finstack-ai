//! Deterministic Codex CLI stand-in for integration tests.
//!
//! The test selects behavior with
//! `--fake-mode <success|fail|fail-noisy|hang|echo|hang-grandchild>` passed
//! through `CodexExecConfig.extra_args`, so parallel tests share no global
//! state.
//!
//! The stand-in also *validates* the argv shape the invoker must always
//! produce (`exec --json --sandbox <mode> --cd <root> ...`, with no prompt)
//! and exits nonzero before emitting anything when it does not match. Like
//! the real `codex exec`, it reads the prompt from stdin when no positional
//! prompt is given; a positional prompt after `--` is still honored so the
//! binary can double as a stand-in for argv-style callers.
//!
//! Modes beyond the original four:
//!
//! - `echo` emits the prompt as the agent message, prefixed with `stdin:` or
//!   `argv:` according to where it came from, so a test can prove the
//!   delivery channel (prompts over 1 KiB are reported as `<channel>-len:<n>`).
//! - `hang-grandchild` spawns `sleep 60` in the stand-in's own process group,
//!   emits the grandchild's pid as the agent message, then hangs; a test can
//!   then check that cancelling the leader also killed the grandchild.

use std::io::{Read, Write};

/// Longest prompt `echo` mode repeats verbatim; longer ones report length.
const ECHO_VERBATIM_BYTES: usize = 1_024;

/// Argv shape observed by the stand-in.
#[derive(Default)]
struct Invocation {
    mode: Option<String>,
    saw_exec: bool,
    saw_json: bool,
    sandbox: Option<String>,
    cd: Option<String>,
    saw_separator: bool,
    prompt: Option<String>,
}

impl Invocation {
    fn is_valid(&self) -> bool {
        let non_empty = |value: &Option<String>| value.as_ref().is_some_and(|v| !v.is_empty());
        self.saw_exec && self.saw_json && non_empty(&self.sandbox) && non_empty(&self.cd)
    }
}

fn parse() -> Invocation {
    let mut parsed = Invocation::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if parsed.saw_separator {
            // Everything after `--` is prompt text, flag-looking or not.
            if parsed.prompt.is_none() {
                parsed.prompt = Some(arg);
            }
            continue;
        }
        match arg.as_str() {
            "exec" => parsed.saw_exec = true,
            "--json" => parsed.saw_json = true,
            "--sandbox" => parsed.sandbox = args.next(),
            "--cd" => parsed.cd = args.next(),
            "--fake-mode" => parsed.mode = args.next(),
            "--" => parsed.saw_separator = true,
            _ => {}
        }
    }
    parsed
}

fn fail(message: &str) -> ! {
    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr, "codex_fake: {message}");
    let _ = stderr.flush();
    std::process::exit(2);
}

/// Where the prompt arrived from.
#[derive(Clone, Copy)]
enum PromptSource {
    Stdin,
    Argv,
}

fn main() {
    let parsed = parse();
    if !parsed.is_valid() {
        fail("unexpected argv shape");
    }
    let (prompt, source) = if let Some(prompt) = parsed.prompt {
        (prompt, PromptSource::Argv)
    } else {
        let mut prompt = String::new();
        if std::io::stdin().read_to_string(&mut prompt).is_err() {
            fail("stdin prompt is not valid utf-8");
        }
        // The prompt must never also appear in argv.
        if std::env::args().skip(1).any(|arg| arg == prompt) {
            fail("prompt was passed in argv");
        }
        (prompt, PromptSource::Stdin)
    };
    if prompt.is_empty() {
        fail("prompt is empty");
    }
    let mode = parsed.mode.unwrap_or_default();
    let mut stdout = std::io::stdout();
    emit(
        &mut stdout,
        r#"{"type":"thread.started","thread_id":"thread-fake-1"}"#,
    );
    match mode.as_str() {
        "fail" | "fail-noisy" => {
            if mode == "fail-noisy" {
                let mut stderr = std::io::stderr();
                let _ = writeln!(stderr, "codex_fake: sandbox denied write to /etc/passwd");
                let _ = stderr.flush();
            }
            emit(
                &mut stdout,
                r#"{"type":"turn.failed","error":{"message":"fake failure"}}"#,
            );
            std::process::exit(1);
        }
        "hang" => {
            std::thread::sleep(std::time::Duration::from_mins(1));
        }
        "echo" => {
            // Long prompts are reported by length: the invoker bounds each
            // JSONL line, so echoing them verbatim would never fit.
            let channel = match source {
                PromptSource::Stdin => "stdin",
                PromptSource::Argv => "argv",
            };
            let text = if prompt.len() > ECHO_VERBATIM_BYTES {
                format!("{channel}-len:{}", prompt.len())
            } else {
                format!("{channel}:{prompt}")
            };
            emit_message(&mut stdout, &text);
            emit(
                &mut stdout,
                r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5}}"#,
            );
        }
        "hang-grandchild" => {
            // Inherit this process's group (no `process_group`), which is the
            // group `configure_process_tree` created for the leader.
            let grandchild = std::process::Command::new("sleep")
                .arg("60")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            let Ok(grandchild) = grandchild else {
                fail("could not spawn grandchild");
            };
            emit_message(&mut stdout, &grandchild.id().to_string());
            std::thread::sleep(std::time::Duration::from_mins(1));
        }
        _ => {
            emit_message(&mut stdout, "fake done");
            emit(
                &mut stdout,
                r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5}}"#,
            );
        }
    }
}

fn emit_message(stdout: &mut std::io::Stdout, text: &str) {
    let line = serde_json::json!({
        "type": "item.completed",
        "item": {"id": "item_1", "type": "agent_message", "text": text},
    });
    emit(stdout, &line.to_string());
}

fn emit(stdout: &mut std::io::Stdout, line: &str) {
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}
