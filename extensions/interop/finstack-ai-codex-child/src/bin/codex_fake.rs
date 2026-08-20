//! Deterministic Codex CLI stand-in for integration tests.
//!
//! The test selects behavior with `--fake-mode <success|fail|fail-noisy|hang>`
//! passed through `CodexExecConfig.extra_args`, so parallel tests share no
//! global state.
//!
//! The stand-in also *validates* the argv shape the invoker must always
//! produce (`exec --json --sandbox <mode> --cd <root> ... -- <prompt>`) and
//! exits nonzero before emitting anything when it does not match. Every
//! integration test therefore doubles as a check on NFR-4: the prompt is
//! passed after a literal `--` so it can never be read as a flag.

use std::io::Write;

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
        self.saw_exec
            && self.saw_json
            && non_empty(&self.sandbox)
            && non_empty(&self.cd)
            && self.saw_separator
            && non_empty(&self.prompt)
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

fn main() {
    let parsed = parse();
    if !parsed.is_valid() {
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "codex_fake: unexpected argv shape");
        let _ = stderr.flush();
        std::process::exit(2);
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
        _ => {
            emit(
                &mut stdout,
                r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"fake done"}}"#,
            );
            emit(
                &mut stdout,
                r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5}}"#,
            );
        }
    }
}

fn emit(stdout: &mut std::io::Stdout, line: &str) {
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}
