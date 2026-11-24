use fujimod_catalog_format::{check_tree, write_tree};
use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let mut check = false;
    let mut root = None;
    for argument in env::args().skip(1) {
        match argument.as_str() {
            "--check" => check = true,
            "-h" | "--help" => {
                usage();
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("unknown option {other}");
                usage();
                std::process::exit(2);
            }
            other => root = Some(PathBuf::from(other)),
        }
    }
    let root =
        root.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog"));
    let report = if check {
        check_tree(&root)
    } else {
        write_tree(&root)
    };
    let report = match report {
        Ok(report) => report,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };
    for path in &report.changed {
        println!("{}", shown(&root, path));
    }
    for (path, problem) in &report.problems {
        eprintln!("error: {}: {problem}", shown(&root, path));
    }
    let total = report.changed.len() + report.unchanged + report.problems.len();
    if check {
        eprintln!(
            "{} of {total} records differ from the canonical form",
            report.changed.len()
        );
    } else {
        eprintln!("wrote {} of {total} records", report.changed.len());
    }
    if !report.problems.is_empty() || (check && !report.changed.is_empty()) {
        std::process::exit(1);
    }
}

fn shown(root: &Path, path: &Path) -> String {
    root.parent()
        .and_then(|base| path.strip_prefix(base).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn usage() {
    eprintln!("usage: catalog-format [--check] [catalog-root]");
}
