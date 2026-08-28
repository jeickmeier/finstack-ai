//! `finstack-know` binary entry point.
//!
//! Exit codes: 0 success, 1 run failure, 2 configuration/usage error.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser as _;
use finstack_ai_knowledge::cli::args::{Cli, Command};
use finstack_ai_knowledge::cli::docs_command;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(cli: Cli) -> Result<ExitCode, finstack_ai_knowledge::KnowledgeError> {
    match cli.command {
        Command::Docs { topic } => {
            let rendered = docs_command(topic.as_deref())?;
            println!("{rendered}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Ask(_) | Command::Ingest(_) | Command::Sessions { .. } | Command::Repl { .. } => {
            Err(finstack_ai_knowledge::KnowledgeError::Config {
                reason: "command_not_implemented_yet",
            })
        }
    }
}
