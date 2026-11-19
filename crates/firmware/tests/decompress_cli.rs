use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn decodes_binary_literals_and_overlapping_backreference() {
    // Three binary literals, then six bytes copied at distance three.
    let body = [3, 0x00, 0xff, 0x80, 0x80, 0x36];
    let input: Vec<u8> = [9u32, 26, 1, 0x4000, 4]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .chain(body)
        .collect();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fujimod-decompress"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        output.stdout,
        [0x00, 0xff, 0x80, 0x00, 0xff, 0x80, 0x00, 0xff, 0x80]
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn malformed_input_fails_without_partial_stdout() {
    // Valid literals precede a backreference with an invalid zero distance.
    let malformed: Vec<u8> = [9u32, 26, 1, 0x4000, 4]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .chain([3, 0x00, 0xff, 0x80, 0x80, 0x06])
        .collect();
    for input in [&[][..], &malformed[..19], &malformed[..25], &malformed] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_fujimod-decompress"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();

        assert!(!output.status.success());
        assert!(output.stdout.is_empty(), "{:?}", output.stdout);
        assert!(!output.stderr.is_empty());
    }
}
