//! Line-oriented bridge for emulator checks.
//!
//! Input lines are M followed by a hex-encoded data-channel message. Output
//! lines are OK followed by zero or more hex-encoded replies, or ERR.

use std::io::{self, BufRead, BufWriter, Write};
use std::path::PathBuf;

use fujimod_sync_server::{FolderDestination, SyncReceiver};

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    text
}

fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    if text.len() % 2 != 0 {
        return Err("odd hex length".to_string());
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    let raw = text.as_bytes();
    for index in (0..raw.len()).step_by(2) {
        let high = (raw[index] as char)
            .to_digit(16)
            .ok_or_else(|| "invalid hex digit".to_string())?;
        let low = (raw[index + 1] as char)
            .to_digit(16)
            .ok_or_else(|| "invalid hex digit".to_string())?;
        bytes.push(((high << 4) | low) as u8);
    }
    Ok(bytes)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .ok_or("usage: sync-receiver-bridge ROOT")?;
    let mut receiver = SyncReceiver::new(FolderDestination::new(PathBuf::from(root)));
    let stdin = io::stdin();
    let mut stdout = BufWriter::new(io::stdout());
    writeln!(stdout, "READY")?;
    stdout.flush()?;
    for line in stdin.lock().lines() {
        let line = line?;
        if line == "Q" {
            writeln!(stdout, "BYE")?;
            stdout.flush()?;
            break;
        }
        let Some(encoded) = line.strip_prefix("M ") else {
            writeln!(stdout, "ERR expected M <hex>")?;
            stdout.flush()?;
            continue;
        };
        let message = match hex_decode(encoded) {
            Ok(message) => message,
            Err(error) => {
                writeln!(stdout, "ERR {error}")?;
                stdout.flush()?;
                continue;
            }
        };
        match receiver.handle(&message) {
            Ok(replies) if replies.is_empty() => writeln!(stdout, "OK")?,
            Ok(replies) => {
                let replies = replies
                    .iter()
                    .map(|reply| hex_encode(reply))
                    .collect::<Vec<_>>()
                    .join(" ");
                writeln!(stdout, "OK {replies}")?;
            }
            Err(error) => writeln!(stdout, "ERR {error}")?,
        }
        stdout.flush()?;
    }
    Ok(())
}
