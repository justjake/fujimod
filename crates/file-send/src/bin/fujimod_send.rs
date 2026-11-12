//! `fujimod-send`: send files to another computer over a direct WebRTC
//! connection, using a share-link room for rendezvous (crates/file-send/README.md).
use fujimod_file_send::{Destination, receive, send, sources};
use fujimod_webrtc::{
    CandidatePolicy, HostEvent, HostRoom, Pacing, PeerSocket, share::OwnerTicket,
};
use std::{io::Write, path::PathBuf, time::Instant};

const USAGE: &str = "usage:
  fujimod-send send [--allow-relay-costs-money] [--signal URL] FILE...
  fujimod-send receive [--allow-relay-costs-money] -o DIRECTORY|- LINK";
const SIGNAL: &str = "wss://share.fujimod.com/usb-proxy/signal";
const SITE: &str = "https://fujimod.com";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(message) = futures_lite::future::block_on(run(args)) {
        eprintln!("fujimod-send: {message}");
        std::process::exit(1);
    }
}

/// Prints each file's progress to standard error at most every half second.
fn reporter() -> impl FnMut(&str, u64, u64) {
    let started = Instant::now();
    let mut shown = None::<Instant>;
    move |name, done, size| {
        let now = Instant::now();
        if done < size && shown.is_some_and(|at| now.duration_since(at).as_millis() < 500) {
            return;
        }
        shown = Some(now);
        let seconds = started.elapsed().as_secs_f64().max(0.001);
        eprint!(
            "\r{name}: {:.1} of {:.1} MiB, {:.1} MiB/s   ",
            done as f64 / 1048576.0,
            size as f64 / 1048576.0,
            done as f64 / 1048576.0 / seconds
        );
        if done == size {
            eprintln!();
        }
        let _ = std::io::stderr().flush();
    }
}

fn direct_hint(error: impl std::fmt::Display, policy: CandidatePolicy) -> String {
    if policy == CandidatePolicy::DirectOnly {
        format!(
            "{error}\nNo direct connection was found. Both sides can pass \
             --allow-relay-costs-money to relay through Cloudflare TURN, which is billed per byte."
        )
    } else {
        error.to_string()
    }
}

async fn run(args: Vec<String>) -> Result<(), String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let mut policy = CandidatePolicy::DirectOnly;
    let mut signal = SIGNAL.to_owned();
    let mut output = None;
    let mut positional = Vec::new();
    let mut items = rest.iter();
    while let Some(item) = items.next() {
        match item.as_str() {
            "--allow-relay-costs-money" => policy = CandidatePolicy::Any,
            "--signal" => signal = items.next().ok_or(USAGE)?.clone(),
            "-o" | "--output" => output = Some(items.next().ok_or(USAGE)?.clone()),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ if item.starts_with('-') && item != "-" => return Err(USAGE.into()),
            _ => positional.push(item.clone()),
        }
    }
    let progress: fujimod_webrtc::Progress = std::sync::Arc::new(|step: &str| {
        eprintln!("{step}");
    });
    match command.as_str() {
        "send" => {
            if positional.is_empty() || output.is_some() {
                return Err(USAGE.into());
            }
            let paths: Vec<PathBuf> = positional.iter().map(PathBuf::from).collect();
            let files = sources(&paths)?;
            let ticket = OwnerTicket::create(signal, SITE).map_err(|e| e.to_string())?;
            let room = HostRoom::join(&ticket.link, &ticket.owner_key, policy, Pacing::Unpaced)
                .await
                .map_err(|e| e.to_string())?;
            eprintln!("On the receiving computer, run:");
            println!("fujimod-send receive -o . '{}'", ticket.link);
            let _ = std::io::stdout().flush();
            let result = loop {
                match room.next_event().await.map_err(|e| e.to_string())? {
                    HostEvent::Controller(_, mut peer) => {
                        eprintln!("receiver connected");
                        let result = send(&mut peer, &files, &mut reporter()).await;
                        peer.goodbye(Vec::new());
                        break result;
                    }
                    HostEvent::Left(_) => (),
                }
            };
            // Revoke the link whether or not the transfer worked.
            let ended = room.end().await.map_err(|e| e.to_string());
            result?;
            ended?;
            eprintln!("sent {} file(s); link revoked", files.len());
            Ok(())
        }
        "receive" => {
            let [link] = positional.as_slice() else {
                return Err(USAGE.into());
            };
            let destination = match output.as_deref().ok_or(USAGE)? {
                "-" => Destination::Stdout,
                directory => {
                    std::fs::create_dir_all(directory).map_err(|e| format!("{directory}: {e}"))?;
                    Destination::Directory(directory.into())
                }
            };
            let mut peer = PeerSocket::connect(link, &progress, policy, Pacing::Unpaced)
                .await
                .map_err(|e| direct_hint(e, policy))?;
            let stored = receive(&mut peer, &destination, &mut reporter()).await?;
            for name in &stored {
                eprintln!("stored {name}");
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}
