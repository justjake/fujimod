use std::io::{self, Read, Write};
use std::process::ExitCode;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut source = Vec::new();
    io::stdin().lock().read_to_end(&mut source)?;
    let (decoded, _) = fujimod_firmware::decompress(&source)?;
    let mut stdout = io::stdout().lock();
    stdout.write_all(&decoded)?;
    stdout.flush()?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fujimod-decompress: {error}");
            ExitCode::FAILURE
        }
    }
}
