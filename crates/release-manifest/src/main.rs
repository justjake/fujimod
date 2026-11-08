use fujimod_release_manifest::Release;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = concat!(
    "Usage: fujimod-release-manifest --firmware ID --version VERSION [--output DIR] PATH=FILE...\n",
    "Writes latest.json and the content-addressed store files for one release.\n",
    "Use -- before a path whose host file starts with '-'."
);

struct Options {
    firmware: String,
    version: String,
    output: PathBuf,
    files: Vec<(String, PathBuf)>,
    help: bool,
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut args = args.peekable();
    let mut firmware = None;
    let mut version = None;
    let mut output = PathBuf::from(".");
    let mut files = Vec::new();
    let mut help = false;
    let mut options = true;
    while let Some(argument) = args.next() {
        if options {
            match argument.as_str() {
                "--" => {
                    options = false;
                    continue;
                }
                "--help" | "-h" => {
                    help = true;
                    continue;
                }
                "--firmware" => {
                    if firmware.is_some() {
                        return Err(USAGE.into());
                    }
                    firmware = Some(args.next().ok_or(USAGE)?);
                    continue;
                }
                "--version" => {
                    if version.is_some() {
                        return Err(USAGE.into());
                    }
                    version = Some(args.next().ok_or(USAGE)?);
                    continue;
                }
                "--output" => {
                    output = PathBuf::from(args.next().ok_or(USAGE)?);
                    continue;
                }
                _ => {}
            }
            if argument.starts_with('-') {
                return Err(USAGE.into());
            }
        }
        let (path, file) = argument.split_once('=').ok_or(USAGE)?;
        files.push((path.to_owned(), PathBuf::from(file)));
    }
    Ok(Options {
        firmware: firmware.unwrap_or_default(),
        version: version.unwrap_or_default(),
        output,
        files,
        help,
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fujimod-release-manifest: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let options = parse_options(std::env::args().skip(1))?;
    if options.help {
        println!("{USAGE}");
        return Ok(());
    }
    if options.firmware.is_empty() || options.version.is_empty() || options.files.is_empty() {
        return Err(USAGE.into());
    }
    let release = Release::new(options.firmware, options.version, options.files)
        .map_err(|error| error.to_string())?;
    release
        .write(&options.output)
        .map_err(|error| error.to_string())?;
    println!(
        "wrote {} store file(s) and latest.json to {}",
        release
            .files()
            .iter()
            .map(|file| file.store_name())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        options.output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        parse_options(args.iter().map(|argument| argument.to_string()))
    }

    #[test]
    fn parses_firmware_version_output_and_pairs() {
        let options = parse(&[
            "--firmware",
            "example-cam",
            "--version",
            "1.2.3",
            "--output",
            "release",
            "/firmware.patch=patch.bin",
            "/etc/ssl/cert.pem=cert.pem",
        ])
        .unwrap();
        assert_eq!(options.firmware, "example-cam");
        assert_eq!(options.version, "1.2.3");
        assert_eq!(options.output, PathBuf::from("release"));
        assert_eq!(
            options.files,
            vec![
                ("/firmware.patch".to_owned(), PathBuf::from("patch.bin")),
                ("/etc/ssl/cert.pem".to_owned(), PathBuf::from("cert.pem")),
            ]
        );
    }

    #[test]
    fn splits_paths_at_the_first_equals_sign() {
        let options = parse(&["/a=b=c"]).unwrap();
        assert_eq!(options.files, vec![("/a".to_owned(), PathBuf::from("b=c"))]);
    }

    #[test]
    fn requires_flags_and_pair_syntax() {
        assert!(parse(&[]).unwrap().files.is_empty());
        assert!(parse(&["--firmware"]).is_err());
        assert!(parse(&["--version"]).is_err());
        assert!(parse(&["--output"]).is_err());
        assert!(parse(&["/a"]).is_err());
        assert!(parse(&["--unknown"]).is_err());
    }

    #[test]
    fn double_dash_allows_dashed_file_names() {
        let options = parse(&["--", "/a=-b"]).unwrap();
        assert_eq!(options.files, vec![("/a".to_owned(), PathBuf::from("-b"))]);
    }
}
