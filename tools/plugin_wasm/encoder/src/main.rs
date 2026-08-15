//! Encode a core wasm module with a component-type section into a component.

use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!("usage: encode-plugin-wasm <core.wasm> <component.wasm>");
        return ExitCode::FAILURE;
    };
    let Some(output) = args.next() else {
        eprintln!("usage: encode-plugin-wasm <core.wasm> <component.wasm>");
        return ExitCode::FAILURE;
    };
    let bytes = match fs::read(&input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("read {input}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let encoded = match encode_component(&bytes) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("encode {input}: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = fs::write(&output, encoded) {
        eprintln!("write {output}: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn encode_component(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = wit_component::ComponentEncoder::default().validate(true);
    encoder = encoder.module(bytes).map_err(|error| error.to_string())?;
    encoder.encode().map_err(|error| error.to_string())
}
