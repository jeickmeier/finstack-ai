//! Native calculator loop plus capability-scoped filesystem discovery.

use std::path::PathBuf;

use finstack_ai::runtime::Toolset;
use finstack_ai_native_examples::{BoxError, run_tool_loop};
use finstack_ai_tools_filesystem::FileSystemToolset;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let root = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    match FileSystemToolset::try_new(&root) {
        Ok(filesystem) => println!("filesystem tools: {}", filesystem.tools().len()),
        Err(error) => println!("filesystem unavailable (fail closed): {error}"),
    }
    println!("calculator result: {}", Box::pin(run_tool_loop()).await?);
    Ok(())
}
