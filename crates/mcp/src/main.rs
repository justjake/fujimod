//! `fujimod-mcp`: the FujiMod MCP server over standard input and output.
//!
//! ```sh
//! fujimod-mcp [--modules camera] [--output DIR]
//! ```
//!
//! `--modules` selects the compiled-in modules to serve (comma separated).
//! `--output` receives large results such as profiles; it defaults to a
//! directory under the system temporary directory.
#![forbid(unsafe_code)]

use std::{path::PathBuf, process::ExitCode, sync::Arc};

use fujimod_mcp_core::{Hub, Module};
use rmcp::ServiceExt;

const HELP: &str = "usage: fujimod-mcp [--modules camera] [--output DIR]\n";

struct Arguments {
    modules: Vec<String>,
    output: PathBuf,
}

fn arguments() -> Result<Option<Arguments>, String> {
    let mut modules = vec!["camera".to_owned()];
    let mut output = std::env::temp_dir().join(format!("fujimod-mcp-{}", std::process::id()));
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--modules" => {
                let value = args.next().ok_or("--modules needs a value")?;
                modules = value
                    .split(',')
                    .map(|name| name.trim().to_owned())
                    .collect();
            }
            "--output" => output = args.next().ok_or("--output needs a value")?.into(),
            other => return Err(format!("unknown argument {other}\n{HELP}")),
        }
    }
    Ok(Some(Arguments { modules, output }))
}

fn module(name: &str, output: &std::path::Path) -> Result<Arc<dyn Module>, String> {
    match name {
        "camera" => Ok(fujimod_mcp_camera::module(output.join("camera"))),
        other => Err(format!("unknown module {other}; available: camera")),
    }
}

async fn serve(arguments: Arguments) -> Result<(), String> {
    let modules = arguments
        .modules
        .iter()
        .map(|name| module(name, &arguments.output))
        .collect::<Result<Vec<_>, _>>()?;
    let hub = Hub::new(modules)?;
    eprintln!(
        "fujimod-mcp serving {} on stdio; output in {}",
        hub.module_names().join(", "),
        arguments.output.display()
    );
    let running = hub
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| error.to_string())?;
    running.waiting().await.map_err(|error| error.to_string())?;
    Ok(())
}

fn main() -> ExitCode {
    let arguments = match arguments() {
        Ok(Some(arguments)) => arguments,
        Ok(None) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("fujimod-mcp: {error}");
            return ExitCode::from(2);
        }
    };
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("fujimod-mcp: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve(arguments)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fujimod-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}
