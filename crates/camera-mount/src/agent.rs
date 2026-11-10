//! Local arbitration through the same owner and wire protocol as camera shares.
use async_tungstenite::tungstenite::Message as Frame;
use fujimod_usb::owner::{DeviceOwner, Exporter};
use fujimod_usb_proxy::{Message, Operation, pipeline::Pipeline, protocol::VERSION};
use futures_util::StreamExt;
use std::{path::PathBuf, sync::Arc};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Socket = async_tungstenite::WebSocketStream<async_net::TcpStream>;
async fn receive(socket: &mut Socket) -> Result<Message> {
    loop {
        match socket.next().await.ok_or("Controller disconnected")?? {
            Frame::Binary(bytes) => return Ok(Message::decode(&bytes)?),
            Frame::Ping(bytes) => socket.send(Frame::Pong(bytes)).await?,
            Frame::Pong(_) => (),
            _ => return Err("Expected binary USB proxy message".into()),
        }
    }
}
async fn send(socket: &mut Socket, message: Message) -> Result<()> {
    socket.send(Frame::Binary(message.encode()?.into())).await?;
    Ok(())
}
trait Backend {
    async fn execute(&self, id: &str, message: Message) -> fujimod_usb_proxy::Completion;
    fn abandon(&self, id: &str);
}
impl Backend for Exporter {
    async fn execute(&self, id: &str, message: Message) -> fujimod_usb_proxy::Completion {
        let result = async {
            let raw = self
                .handle(id, &message.encode()?)
                .await
                .map_err(|e| fujimod_usb_proxy::Error(e.to_string()))?;
            let Message::Result { completion, .. } = Message::decode(&raw)? else {
                return Err(fujimod_usb_proxy::Error("Invalid exporter result".into()));
            };
            Ok::<_, fujimod_usb_proxy::Error>(completion)
        }
        .await;
        result.unwrap_or_else(|e| fujimod_usb_proxy::Completion {
            error: Some(e.to_string()),
            ..Default::default()
        })
    }
    fn abandon(&self, id: &str) {
        Exporter::abandon(self, id);
    }
}
async fn controller<B: Backend>(
    mut socket: Socket,
    exporter: Arc<B>,
    gate: Arc<async_lock::Mutex<()>>,
    token: String,
    id: String,
) -> Result<()> {
    let Message::Hello {
        version: VERSION,
        role,
        token: offered,
        ..
    } = receive(&mut socket).await?
    else {
        return Err("Expected USB proxy handshake".into());
    };
    if role != "controller" || offered != token {
        return Err("Invalid credentials".into());
    }
    send(
        &mut socket,
        Message::Ready {
            version: VERSION,
            id: id.clone(),
        },
    )
    .await?;
    let mut pipeline = Pipeline::default();
    let mut lease = None;
    let result: Result<()> = async {
        loop {
            let message = receive(&mut socket).await?;
            if matches!(message, Message::Goodbye { .. }) { return Ok(()); }
            if !Pipeline::carries(&message) { continue; }
            if matches!(&message, Message::Execute { op: Operation::Open { .. }, .. }) {
                if lease.is_some() { return Err("Controller already holds USB".into()); }
                // Wait before Open; never retry USB transfers. The owner keeps
                // uncertain-completion fences after a controller disconnects.
                lease = Some(gate.lock_arc().await);
            }
            let closing = matches!(&message, Message::Execute { op: Operation::Close, .. });
            let reply = pipeline.handle(message, |message| exporter.execute(&id, message)).await?;
            let failed = matches!(&reply, Some(Message::Result { completion, .. }) if completion.error.is_some());
            if closing && !failed { lease = None; }
            if let Some(reply) = reply { send(&mut socket, reply).await?; }
            if failed { return Err("USB operation failed".into()); }
        }
    }.await;
    exporter.abandon(&id);
    result
}
async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let selector = args
        .next()
        .ok_or("Usage: fujimod-usb-agent BUS-PORTS READY-JSON")?;
    let ready = PathBuf::from(args.next().ok_or("Missing ready file")?);
    let token = std::env::var("FUJIMOD_USB_PROXY_TOKEN")?;
    let mut watch = nusb::watch_devices()?;
    let devices: Vec<_> = nusb::list_devices()
        .await?
        .filter(|d| fujimod_usb::share_owner::device_key(d) == selector)
        .collect();
    if devices.len() != 1 {
        return Err("Expected one selected USB camera".into());
    }
    let info = devices.into_iter().next().unwrap();
    let device_id = info.id();
    let owner = DeviceOwner::open(info, selector).await?;
    owner.share()?;
    let listener = async_net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}/usb-proxy/ws", listener.local_addr()?);
    let temporary = ready.with_extension("tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec(&serde_json::json!({"url": url, "token": token}))?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, &ready)?;
    // Each USB enumeration gets a new agent and helper. Never release RAM
    // from a previous boot, including a restart shorter than a discovery poll.
    std::thread::spawn(move || {
        futures_lite::future::block_on(async move {
            while let Some(event) = watch.next().await {
                if matches!(event, nusb::hotplug::HotplugEvent::Disconnected(id) if id == device_id)
                {
                    let _ = std::fs::remove_file(ready);
                    std::process::exit(0);
                }
            }
        })
    });
    let exporter = Arc::new(Exporter::new([owner]));
    let gate = Arc::new(async_lock::Mutex::new(()));
    let mut counter = 0u64;
    loop {
        let (stream, _) = listener.accept().await?;
        counter += 1;
        let (exporter, gate, token) = (exporter.clone(), gate.clone(), token.clone());
        std::thread::spawn(move || {
            futures_lite::future::block_on(async move {
                if let Ok(socket) = async_tungstenite::accept_async(stream).await
                    && let Err(error) =
                        controller(socket, exporter, gate, token, format!("local-{counter}")).await
                {
                    eprintln!("USB agent: {error}");
                }
            })
        });
    }
}
fn main() {
    if let Err(error) = futures_lite::future::block_on(run()) {
        eprintln!("fujimod-usb-agent: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_usb_proxy::{Client, Completion};
    use std::{collections::BTreeSet, sync::Mutex, time::Duration};

    #[derive(Default)]
    struct Camera(Mutex<BTreeSet<String>>);
    impl Backend for Camera {
        async fn execute(&self, id: &str, message: Message) -> Completion {
            let mut owners = self.0.lock().unwrap();
            match message {
                Message::Execute {
                    op: Operation::Open { .. },
                    ..
                } => {
                    assert!(owners.is_empty(), "two controllers interleaved");
                    owners.insert(id.into());
                }
                Message::Execute {
                    op: Operation::Close,
                    ..
                } => {
                    assert!(owners.remove(id));
                }
                _ => (),
            }
            Completion::default()
        }
        fn abandon(&self, id: &str) {
            self.0.lock().unwrap().remove(id);
        }
    }
    #[test]
    fn proxy_sessions_wait_for_close_and_reuse_one_camera() {
        futures_lite::future::block_on(async {
            let listener = async_net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}/usb-proxy/ws", listener.local_addr().unwrap());
            let camera = Arc::new(Camera::default());
            let observed = camera.clone();
            let gate = Arc::new(async_lock::Mutex::new(()));
            std::thread::spawn(move || {
                futures_lite::future::block_on(async move {
                    for n in 0..2 {
                        let (stream, _) = listener.accept().await.unwrap();
                        let (camera, gate) = (camera.clone(), gate.clone());
                        std::thread::spawn(move || {
                            futures_lite::future::block_on(async {
                                let socket = async_tungstenite::accept_async(stream).await.unwrap();
                                let _ = controller(
                                    socket,
                                    camera,
                                    gate,
                                    "secret".into(),
                                    format!("{n}"),
                                )
                                .await;
                            })
                        });
                    }
                })
            });
            let mut first = Client::connect(&url, "secret").await.unwrap();
            let mut second = Client::connect(&url, "secret").await.unwrap();
            let timeout = Duration::from_secs(5);
            let open = Operation::Open {
                configuration: 1,
                interface: 0,
                alternate: 0,
            };
            assert!(
                first
                    .request("camera", open.clone(), timeout)
                    .await
                    .unwrap()
                    .error
                    .is_none()
            );
            {
                let pending = second.request("camera", open, timeout);
                futures_lite::pin!(pending);
                assert!(
                    futures_lite::future::poll_once(&mut pending)
                        .await
                        .is_none()
                );
                assert_eq!(observed.0.lock().unwrap().len(), 1);
                first
                    .request("camera", Operation::Close, timeout)
                    .await
                    .unwrap();
                assert!(pending.await.unwrap().error.is_none());
            }
            second
                .request("camera", Operation::Close, timeout)
                .await
                .unwrap();
            assert!(observed.0.lock().unwrap().is_empty());
        });
    }
}
