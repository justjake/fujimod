use fujimod_emulator_ip_dmp::gles::replay_captures;
use std::path::PathBuf;

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let Some(directory) = arguments.next() else {
        eprintln!("usage: replay_capture <capture-directory>");
        std::process::exit(2);
    };
    if arguments.next().is_some() {
        eprintln!("usage: replay_capture <capture-directory>");
        std::process::exit(2);
    }
    match replay_captures(&PathBuf::from(directory)) {
        Ok(count) => println!("replayed {count} DMP command lists with host GPU parity"),
        Err(error) => {
            eprintln!("DMP capture replay failed: {error}");
            std::process::exit(1);
        }
    }
}
