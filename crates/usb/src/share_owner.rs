//! Native owner of a persistent share link: one local camera, served to the
//! link's controllers with the browser owner's protocol (SHARING.md).
use crate::owner::{DeviceOwner, DeviceOwners, Exporter};
use fujimod_usb_proxy::{
    CandidatePolicy, Completion, HostEvent, HostRoom, Message, Operation, Pacing, PeerSocket,
    pipeline::Pipeline,
    protocol::{DeviceStatus, VERSION},
    share::{OwnerTicket, Ticket},
};
use futures_lite::StreamExt;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn error(value: impl std::fmt::Display) -> crate::Error {
    fujimod_usb_proxy::Error(value.to_string()).into()
}

/// How the owner answers controller requests that need a person.
#[derive(Clone, Default)]
pub struct OwnerHooks {
    /// Command and arguments; the attention message is appended as the last
    /// argument. Its exit status 0 completes the controller's request.
    pub attention: Option<Vec<String>>,
}

/// `BUS-PORTS` in hex (`02-01`), the local camera position tools select with
/// `FUJIMOD_USB_DEVICE`.
pub fn device_key(info: &nusb::DeviceInfo) -> String {
    position_key(info.bus_id(), info.port_chain())
}
fn position_key(bus: &str, ports: &[u8]) -> String {
    let ports: String = ports.iter().map(|port| format!("{port:02x}")).collect();
    format!("{bus}-{ports}")
}

mod source;
use source::Source;

struct Shared {
    source: Source,
    hooks: OwnerHooks,
    link_key: String,
    battery: Mutex<Option<(String, Instant)>>,
}
impl Shared {
    fn status(&self) -> DeviceStatus {
        let reason = if !self.source.present() {
            Some("unplugged".to_owned())
        } else if self.source.fenced() {
            Some("not responding".to_owned())
        } else {
            None
        };
        let battery = self.battery.lock().unwrap().clone();
        DeviceStatus {
            online: reason.is_none(),
            reason,
            battery_age_ms: battery
                .as_ref()
                .map(|(_, at)| at.elapsed().as_millis() as u64),
            battery: battery.map(|(level, _)| level),
        }
    }
    fn device_id(&self) -> &str {
        &self.source.descriptor().id
    }
}

fn log(line: impl AsRef<str>) {
    println!("{}", line.as_ref());
}

/// Wait for the selected local camera, then serve it on `ticket`'s room until
/// the process is stopped. Signaling loss rejoins; camera loss keeps the room.
pub async fn serve(
    ticket: OwnerTicket,
    selector: Option<String>,
    hooks: OwnerHooks,
) -> crate::Result<()> {
    let link = Ticket::from_url(&ticket.link)?.ok_or_else(|| error("invalid share link"))?;
    let selector = selector.or_else(|| std::env::var("FUJIMOD_USB_DEVICE").ok());
    if let Some(source) = Source::configured_proxy(selector.as_deref()).await? {
        return serve_room(
            ticket,
            Arc::new(Shared {
                source,
                hooks,
                link_key: link.key,
                battery: Mutex::new(None),
            }),
        )
        .await;
    }
    let owners = Arc::new(DeviceOwners::default());
    let mut watch = nusb::watch_devices()?;
    let registry = owners.clone();
    std::thread::Builder::new()
        .name("fujimod-share-hotplug".into())
        .spawn(move || {
            futures_lite::future::block_on(async move {
                while let Some(event) = watch.next().await {
                    match event {
                        nusb::hotplug::HotplugEvent::Connected(info) => {
                            if info.vendor_id() == 0x04cb {
                                log(format!("camera connected at {}", device_key(&info)));
                            }
                            registry.connected(info).await
                        }
                        nusb::hotplug::HotplugEvent::Disconnected(id) => {
                            registry.disconnected(id).await
                        }
                    }
                }
            })
        })
        .map_err(error)?;
    let info = loop {
        let cameras: Vec<_> = nusb::list_devices()
            .await?
            .filter(|info| info.vendor_id() == 0x04cb)
            .filter(|info| {
                selector
                    .as_deref()
                    .is_none_or(|key| device_key(info) == key)
            })
            .collect();
        match cameras.len() {
            1 => break cameras.into_iter().next().unwrap(),
            0 => futures_timer::Delay::new(Duration::from_secs(1)).await,
            _ => {
                return Err(error(format!(
                    "{} Fujifilm cameras connected; select one with --device BUS-PORTS ({})",
                    cameras.len(),
                    cameras
                        .iter()
                        .map(device_key)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    };
    log(format!(
        "sharing {} at {}",
        info.product_string().unwrap_or("camera"),
        device_key(&info)
    ));
    let owner = owners.get(info).await?;
    owner.share()?;
    let shared = Arc::new(Shared {
        source: Source::native(owner),
        hooks,
        link_key: link.key,
        battery: Mutex::new(None),
    });
    serve_room(ticket, shared).await
}

async fn serve_room(ticket: OwnerTicket, shared: Arc<Shared>) -> crate::Result<()> {
    loop {
        let room = match HostRoom::join(
            &ticket.link,
            &ticket.owner_key,
            CandidatePolicy::Any,
            Pacing::Browser,
        )
        .await
        {
            Ok(room) => room,
            Err(failure) => {
                log(format!(
                    "share room unavailable: {failure}; retrying in 5 s"
                ));
                futures_timer::Delay::new(Duration::from_secs(5)).await;
                continue;
            }
        };
        log(format!("sharing on {}", ticket.link));
        loop {
            match room.next_event().await {
                Ok(HostEvent::Controller(id, peer)) => {
                    let shared = shared.clone();
                    std::thread::Builder::new()
                        .name("fujimod-share-controller".into())
                        .spawn(move || futures_lite::future::block_on(session(shared, id, peer)))
                        .map_err(error)?;
                }
                Ok(HostEvent::Left(_)) => (),
                Err(failure) => {
                    log(format!("share signaling lost: {failure}; rejoining"));
                    break;
                }
            }
        }
        drop(room);
        futures_timer::Delay::new(Duration::from_secs(1)).await;
    }
}

/// One controller connection, as `web/peer.ts` ShareHost serves it.
async fn session(shared: Arc<Shared>, id: String, mut peer: PeerSocket) {
    let mut goodbye = None;
    let result = run_session(&shared, &id, &mut peer, &mut goodbye).await;
    let reason = match (&result, goodbye) {
        (_, Some(reason)) => reason,
        (Err(failure), None) => failure.to_string(),
        (Ok(()), None) => "connection lost without goodbye".into(),
    };
    log(format!("controller {id} disconnected: {reason}"));
    if shared.source.abandon(&id) {
        log(format!(
            "controller {id} disconnected while USB was claimed; completion unknown"
        ));
    }
}

async fn run_session(
    shared: &Shared,
    id: &str,
    peer: &mut PeerSocket,
    goodbye: &mut Option<String>,
) -> crate::Result<()> {
    let hello = Message::decode(&peer.receive().await?)?;
    let Message::Hello {
        version,
        role,
        token,
        reason,
    } = hello
    else {
        return Err(error("invalid peer authentication"));
    };
    if version != VERSION || role != "controller" || token != shared.link_key {
        return Err(error("invalid peer authentication"));
    }
    log(format!(
        "controller {id} connected: {}",
        reason.as_deref().unwrap_or("")
    ));
    peer.send(
        Message::Ready {
            version: VERSION,
            id: id.to_owned(),
        }
        .encode()?,
    )
    .await?;
    let mut pipeline = Pipeline::default();
    loop {
        let message = Message::decode(&peer.receive().await?)?;
        match message {
            Message::Goodbye { reason } => {
                *goodbye = Some(if reason.is_empty() {
                    "done".into()
                } else {
                    reason
                });
                return Ok(());
            }
            Message::Status { battery } => {
                log(format!("controller {id} battery: {battery}"));
                *shared.battery.lock().unwrap() = Some((battery, Instant::now()));
            }
            Message::Attention { message } => log(format!("controller {id} attention: {message}")),
            Message::Activity { title } => {
                if !title.is_empty() {
                    log(format!("controller {id}: {title}"))
                }
            }
            Message::ProgressStart { label, total } => {
                log(format!("controller {id} progress: {label} (0/{total})"))
            }
            Message::ProgressUpdate { .. } | Message::ProgressDone => (),
            message if Pipeline::carries(&message) => {
                if let Message::Execute { timeout_ms, .. } = &message
                    && !(1..=120_000).contains(timeout_ms)
                {
                    return Err(error("invalid USB request"));
                }
                let reply = pipeline
                    .handle(message, |message| execute(shared, id, message))
                    .await?;
                let failed = matches!(
                    &reply,
                    Some(Message::Result { completion, .. }) if completion.error.is_some()
                );
                if let Some(reply) = reply {
                    peer.send(reply.encode()?).await?;
                }
                // A USB failure ends the connection; nothing queued behind it runs.
                if failed {
                    return Err(error("USB operation failed"));
                }
            }
            _ => return Err(error("invalid USB request")),
        }
    }
}

fn failed(message: impl Into<String>) -> Completion {
    Completion {
        error: Some(message.into()),
        ..Default::default()
    }
}

async fn execute(shared: &Shared, id: &str, message: Message) -> Completion {
    let Message::Execute { device, op, .. } = &message else {
        return failed("expected an execute message");
    };
    match op {
        Operation::Probe => return Completion::default(),
        Operation::List => {
            let mut descriptor = shared.source.descriptor().clone();
            descriptor.status = Some(shared.status());
            return Completion {
                devices: vec![descriptor],
                ..Default::default()
            };
        }
        _ => (),
    }
    if device != shared.device_id() {
        return failed("Unknown shared device");
    }
    if !shared.source.present() {
        return failed("Shared camera offline: unplugged");
    }
    match op {
        Operation::OwnerStatus { message } | Operation::OwnerAttention { message } => {
            if !shared.source.owns(id, device) {
                return failed("Shared camera is not claimed");
            }
            if message.is_empty() || message.len() > 512 {
                return failed("Invalid owner message");
            }
            if matches!(op, Operation::OwnerStatus { .. }) {
                log(format!("controller {id} status: {message}"));
                return Completion::default();
            }
            log(format!("controller {id} attention countdown: {message}"));
            let Some(command) = shared.hooks.attention.clone() else {
                return failed("Owner countdown unavailable");
            };
            let message = message.clone();
            let (done, wait) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let status = std::process::Command::new(&command[0])
                    .args(&command[1..])
                    .arg(message)
                    .status();
                let _ = done.send_blocking(status.map(|s| s.success()).unwrap_or(false));
            });
            return match wait.recv().await {
                Ok(true) => Completion::default(),
                _ => failed("Owner countdown failed"),
            };
        }
        _ => (),
    }
    shared.source.execute(id, message).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn position_keys_match_the_camera_queue_device_key() {
        assert_eq!(position_key("02", &[1]), "02-01");
        assert_eq!(position_key("01", &[1, 0x0a]), "01-010a");
    }
}
