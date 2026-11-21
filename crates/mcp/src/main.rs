//! `fujimod-mcp`: the FujiMod MCP server over standard input and output.
//!
//! ```sh
//! fujimod-mcp [--modules camera] [--output DIR]
//! ```
//!
//! All compiled-in modules are enabled by default. `--modules` limits the
//! modules to serve (comma separated).
//! `--output` receives large results such as profiles; it defaults to a
//! directory under the system temporary directory.
#![forbid(unsafe_code)]

use std::{path::PathBuf, process::ExitCode, sync::Arc};

use fujimod_mcp_core::{Hub, Module};
use rmcp::ServiceExt;

const HELP: &str = "usage: fujimod-mcp [--modules camera] [--output DIR]\n";

struct Arguments {
    modules: Option<Vec<String>>,
    output: PathBuf,
}

fn arguments() -> Result<Option<Arguments>, String> {
    let mut modules = None;
    let mut output = std::env::temp_dir().join(format!("fujimod-mcp-{}", std::process::id()));
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--modules" => {
                let value = args.next().ok_or("--modules needs a value")?;
                modules = Some(
                    value
                        .split(',')
                        .map(|name| name.trim().to_owned())
                        .collect(),
                );
            }
            "--output" => output = args.next().ok_or("--output needs a value")?.into(),
            other => return Err(format!("unknown argument {other}\n{HELP}")),
        }
    }
    Ok(Some(Arguments { modules, output }))
}

type ModuleFactory = fn(std::path::PathBuf) -> Arc<dyn Module>;

// Register each compiled-in module once; every registered module is enabled
// unless the caller explicitly limits the selection with --modules.
const MODULES: &[(&str, ModuleFactory)] = &[("camera", fujimod_mcp_camera::module)];

fn modules(
    selected: Option<&[String]>,
    output: &std::path::Path,
) -> Result<Vec<Arc<dyn Module>>, String> {
    let Some(names) = selected else {
        return Ok(MODULES
            .iter()
            .map(|(name, build)| build(output.join(name)))
            .collect());
    };
    names
        .iter()
        .map(|name| {
            let (_, build) = MODULES
                .iter()
                .find(|(registered, _)| registered == name)
                .ok_or_else(|| {
                    format!(
                        "unknown module {name}; available: {}",
                        MODULES
                            .iter()
                            .map(|(name, _)| *name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
            Ok(build(output.join(name)))
        })
        .collect()
}

async fn serve(arguments: Arguments) -> Result<(), String> {
    let modules = modules(arguments.modules.as_deref(), &arguments.output)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn defaults_enable_every_registered_module() {
        let active = modules(None, Path::new("unused")).unwrap();
        assert_eq!(
            active
                .iter()
                .map(|module| module.name())
                .collect::<Vec<_>>(),
            MODULES.iter().map(|(name, _)| *name).collect::<Vec<_>>()
        );
        assert!(active.iter().any(|module| module.name() == "camera"));
    }

    #[test]
    fn explicit_selection_limits_modules_and_refuses_unknown_names() {
        let active = modules(Some(&["camera".to_owned()]), Path::new("unused")).unwrap();
        assert_eq!(
            active
                .iter()
                .map(|module| module.name())
                .collect::<Vec<_>>(),
            vec!["camera"]
        );
        assert!(modules(Some(&[]), Path::new("unused")).unwrap().is_empty());
        let error = modules(Some(&["missing".to_owned()]), Path::new("unused"))
            .err()
            .unwrap();
        assert!(error.contains("unknown module missing; available: camera"));
    }
}
