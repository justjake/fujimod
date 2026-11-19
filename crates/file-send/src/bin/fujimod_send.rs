//! `fujimod-send`: move files and keep a directory in sync between computers
//! over direct WebRTC connections, using a share-link room for rendezvous
//! (crates/file-send/README.md).
use fujimod_file_send::{
    Destination, DirectorySource, build_info, destination_info, path_info, prepare_sync_root,
    receive, send, serve_peer, sources, sync,
};
use fujimod_webrtc::{
    CandidatePolicy, HostEvent, HostRoom, Pacing, PeerSocket, share::OwnerTicket,
};
use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

const USAGE: &str = "usage:
  fujimod-send send [--allow-relay-costs-money] [--signal URL] FILE...
  fujimod-send receive [--allow-relay-costs-money] -o DIRECTORY|- LINK
  fujimod-send serve [--allow-relay-costs-money] [--signal URL] [--ticket FILE] DIRECTORY
  fujimod-send sync [--allow-relay-costs-money] [--force] -o DIRECTORY LINK";
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
fn reporter(label: &str) -> impl FnMut(&str, u64, u64) + '_ {
    let prefix = if label.is_empty() {
        String::new()
    } else {
        format!("[{label}] ")
    };
    let mut current: Option<(String, Instant, u64)> = None;
    let mut shown = None::<Instant>;
    move |name, done, size| {
        let now = Instant::now();
        let previous = current
            .as_ref()
            .filter(|(current_name, ..)| current_name == name)
            .map(|(_, started, start_done)| (*started, *start_done));
        let (started, start_done) = match previous {
            Some(previous) => previous,
            None => {
                current = Some((name.to_owned(), now, done));
                shown = None;
                (now, done)
            }
        };
        if done < size && shown.is_some_and(|at| now.duration_since(at).as_millis() < 500) {
            return;
        }
        shown = Some(now);
        let seconds = now.duration_since(started).as_secs_f64().max(0.001);
        let moved = done.saturating_sub(start_done);
        eprint!(
            "\r{prefix}{name}: {:.1} of {:.1} MiB, {:.1} MiB/s   ",
            done as f64 / 1048576.0,
            size as f64 / 1048576.0,
            moved as f64 / 1048576.0 / seconds
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
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let mut policy = CandidatePolicy::DirectOnly;
    let mut signal = SIGNAL.to_owned();
    let mut output = None;
    let mut ticket_path = None;
    let mut force = false;
    let mut positional = Vec::new();
    let mut items = rest.iter();
    while let Some(item) = items.next() {
        match item.as_str() {
            "--allow-relay-costs-money" => policy = CandidatePolicy::Any,
            "--signal" => signal = items.next().ok_or(USAGE)?.clone(),
            "--ticket" => ticket_path = Some(items.next().ok_or(USAGE)?.clone()),
            "--force" => force = true,
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
            if positional.is_empty() || output.is_some() || ticket_path.is_some() || force {
                return Err(USAGE.into());
            }
            eprintln!("{}", build_info());
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
                        if let Some(path) = peer.path() {
                            eprintln!("{path}");
                        }
                        let mut report = reporter("");
                        let result = send(&mut peer, &files, &mut report).await;
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
            if ticket_path.is_some() || force {
                return Err(USAGE.into());
            }
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
            eprintln!("{}", build_info());
            eprintln!("{}", destination_info(&destination));
            let mut peer = PeerSocket::connect(link, &progress, policy, Pacing::Unpaced)
                .await
                .map_err(|e| direct_hint(e, policy))?;
            if let Some(path) = peer.path() {
                eprintln!("{path}");
            }
            let mut report = reporter("");
            let stored = receive(&mut peer, &destination, &mut report).await?;
            for name in &stored {
                eprintln!("stored {name}");
            }
            Ok(())
        }
        "serve" => {
            let [directory] = positional.as_slice() else {
                return Err(USAGE.into());
            };
            if output.is_some() || force {
                return Err(USAGE.into());
            }
            let source = DirectorySource::open(directory)?;
            let ticket = match ticket_path {
                Some(path) if std::path::Path::new(&path).exists() => {
                    OwnerTicket::read(&path).map_err(|e| e.to_string())?
                }
                Some(path) => {
                    let ticket = OwnerTicket::create(signal, SITE).map_err(|e| e.to_string())?;
                    ticket.write(&path).map_err(|e| e.to_string())?;
                    ticket
                }
                None => OwnerTicket::create(signal, SITE).map_err(|e| e.to_string())?,
            };
            let room = HostRoom::join(&ticket.link, &ticket.owner_key, policy, Pacing::Unpaced)
                .await
                .map_err(|e| e.to_string())?;
            let (_, snapshot) = source.snapshot();
            eprintln!(
                "Serving {} ({} path(s)); press Ctrl-C to stop",
                source.root().display(),
                snapshot.len()
            );
            eprintln!("On the receiving computer, run:");
            println!("fujimod-send sync -o . '{}'", ticket.link);
            let _ = std::io::stdout().flush();
            loop {
                match room.next_event().await.map_err(|e| e.to_string())? {
                    HostEvent::Controller(id, mut peer) => {
                        eprintln!("{id}: connected");
                        if let Some(path) = peer.path() {
                            eprintln!("{id}: {path}");
                        }
                        let source = source.clone();
                        let label = id.clone();
                        std::thread::Builder::new()
                            .name("fujimod-file-server-client".into())
                            .spawn(move || {
                                futures_lite::future::block_on(async {
                                    let mut report = reporter(&label);
                                    match serve_peer(&mut peer, &source, &mut report).await {
                                        Ok(()) => eprintln!("{label}: disconnected"),
                                        Err(error) => eprintln!("{label}: {error}"),
                                    }
                                    peer.goodbye(Vec::new());
                                });
                            })
                            .map_err(|e| e.to_string())?;
                    }
                    HostEvent::Left(id) => eprintln!("{id}: left"),
                }
            }
        }
        "sync" => {
            if ticket_path.is_some() {
                return Err(USAGE.into());
            }
            let [link] = positional.as_slice() else {
                return Err(USAGE.into());
            };
            let directory = output.ok_or(USAGE)?;
            if directory == "-" {
                return Err("sync needs a directory, not standard output".into());
            }
            if fujimod_webrtc::share::Ticket::from_url(link)
                .map_err(|e| e.to_string())?
                .is_none()
            {
                return Err("not a share link".into());
            }
            let directory = PathBuf::from(directory);
            prepare_sync_root(&directory, force)?;
            eprintln!("{}", build_info());
            eprintln!("{}", path_info(&directory));
            loop {
                let mut peer =
                    match PeerSocket::connect(link, &progress, policy, Pacing::Unpaced).await {
                        Ok(peer) => peer,
                        Err(error) => {
                            eprintln!("{}", direct_hint(error, policy));
                            std::thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                    };
                eprintln!("connected to sender");
                if let Some(path) = peer.path() {
                    eprintln!("{path}");
                }
                let mut report = reporter("");
                match sync(&mut peer, &directory, &mut report).await {
                    Ok(()) => {
                        peer.goodbye(Vec::new());
                        eprintln!("sender closed the connection");
                        return Ok(());
                    }
                    Err(error) => {
                        peer.goodbye(Vec::new());
                        eprintln!("sync: {error}; reconnecting");
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        _ => Err(USAGE.into()),
    }
}
