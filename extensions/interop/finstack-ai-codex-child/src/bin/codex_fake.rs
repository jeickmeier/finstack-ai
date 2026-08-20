//! Deterministic Codex CLI stand-in for integration tests.
//!
//! Ignores real Codex flags; the test selects behavior with `--fake-mode
//! <success|fail|hang>` passed through `CodexExecConfig.extra_args`, so
//! parallel tests share no global state.

use std::io::Write;

fn main() {
    let mut mode = String::from("success");
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--fake-mode"
            && let Some(value) = args.next()
        {
            mode = value;
        }
    }
    let mut stdout = std::io::stdout();
    emit(
        &mut stdout,
        r#"{"type":"thread.started","thread_id":"thread-fake-1"}"#,
    );
    match mode.as_str() {
        "fail" => {
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
