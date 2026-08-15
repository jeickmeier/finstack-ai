//! Write the historical journal v1 corpus onto disk.

use std::process::ExitCode;

fn main() -> ExitCode {
    match finstack_ai_test::write_journal_v1_fixtures() {
        Ok(written) => {
            println!("wrote {written} journal v1 fixtures");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
