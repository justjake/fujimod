//! `fujimod-share-owner`: create, serve and end persistent share links for a
//! camera on this computer's USB (crates/usb-proxy/SHARING.md).
use fujimod_usb::share_owner::{OwnerHooks, serve};
use fujimod_webrtc::{CandidatePolicy, HostRoom, Pacing, share::OwnerTicket};

const USAGE: &str = "usage:
  fujimod-share-owner create --signal URL [--site URL] TICKET.json
  fujimod-share-owner serve TICKET.json [--device BUS-PORTS] [--attention-command CMD ARGS...]
  fujimod-share-owner end TICKET.json";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(message) = futures_lite::future::block_on(run(args)) {
        eprintln!("fujimod-share-owner: {message}");
        std::process::exit(1);
    }
}

async fn run(args: Vec<String>) -> Result<(), String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    match command.as_str() {
        "create" => {
            let mut signal = None;
            let mut site = "https://fujimod.com".to_owned();
            let mut path = None;
            let mut items = rest.iter();
            while let Some(item) = items.next() {
                match item.as_str() {
                    "--signal" => signal = items.next().cloned(),
                    "--site" => site = items.next().cloned().ok_or(USAGE)?,
                    _ if path.is_none() => path = Some(item.clone()),
                    _ => return Err(USAGE.into()),
                }
            }
            let (signal, path) = (signal.ok_or(USAGE)?, path.ok_or(USAGE)?);
            let ticket = OwnerTicket::create(signal, site).map_err(|e| e.to_string())?;
            // Join once so the room exists, offline, before anything is shared.
            drop(
                HostRoom::join(
                    &ticket.link,
                    &ticket.owner_key,
                    CandidatePolicy::Any,
                    Pacing::Browser,
                )
                .await
                .map_err(|e| e.to_string())?,
            );
            ticket.write(&path).map_err(|e| e.to_string())?;
            println!("{}", ticket.link);
            Ok(())
        }
        "serve" => {
            let (path, rest) = rest.split_first().ok_or(USAGE)?;
            let mut device = None;
            let mut hooks = OwnerHooks::default();
            let mut items = rest.iter();
            while let Some(item) = items.next() {
                match item.as_str() {
                    "--device" => device = Some(items.next().cloned().ok_or(USAGE)?),
                    "--attention-command" => {
                        let command: Vec<String> = items.by_ref().cloned().collect();
                        if command.is_empty() {
                            return Err(USAGE.into());
                        }
                        hooks.attention = Some(command);
                    }
                    _ => return Err(USAGE.into()),
                }
            }
            let ticket = OwnerTicket::read(path).map_err(|e| e.to_string())?;
            serve(ticket, device, hooks)
                .await
                .map_err(|e| e.to_string())
        }
        "end" => {
            let path = rest.first().ok_or(USAGE)?;
            let ticket = OwnerTicket::read(path).map_err(|e| e.to_string())?;
            let room = HostRoom::join(
                &ticket.link,
                &ticket.owner_key,
                CandidatePolicy::Any,
                Pacing::Browser,
            )
            .await
            .map_err(|e| e.to_string())?;
            room.end().await.map_err(|e| e.to_string())
        }
        _ => Err(USAGE.into()),
    }
}
