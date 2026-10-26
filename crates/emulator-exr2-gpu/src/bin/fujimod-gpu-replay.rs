use std::path::PathBuf;

fn main() {
    let mut arguments = std::env::args_os();
    let program = arguments.next().unwrap_or_default();
    let Some(path) = arguments.next() else {
        eprintln!("usage: {} RECORDING", PathBuf::from(program).display());
        std::process::exit(2);
    };
    if arguments.next().is_some() {
        eprintln!("usage: {} RECORDING", PathBuf::from(program).display());
        std::process::exit(2);
    }
    if let Err(error) = fujimod_emulator_exr2_gpu::replay::run(PathBuf::from(path)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
