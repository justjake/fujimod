//! A share can export its own native owner or forward an existing USB owner.
use super::*;
use crate::connection::{Device, DeviceFilter};
use fujimod_usb_proxy::{Client, RemoteDevice};
use std::collections::{BTreeSet, HashMap};

type Controller = Arc<async_lock::Mutex<Option<Client>>>;
pub(super) enum Source {
    Native {
        owner: Arc<DeviceOwner>,
        exporter: Exporter,
    },
    Proxy {
        device: Box<RemoteDevice>,
        ready: Option<std::path::PathBuf>,
        clients: Mutex<HashMap<String, Controller>>,
        claimed: Mutex<BTreeSet<String>>,
    },
}
impl Source {
    pub fn native(owner: Arc<DeviceOwner>) -> Self {
        Self::Native {
            exporter: Exporter::new([owner.clone()]),
            owner,
        }
    }
    pub async fn configured_proxy(selector: Option<&str>) -> crate::Result<Option<Self>> {
        if std::env::var_os("FUJIMOD_USB_PROXY_URL").is_none() {
            return Ok(None);
        }
        let mut devices = crate::connection::discover(DeviceFilter::Fujifilm).await?;
        if let Some(selector) = selector {
            devices.retain(|device| {
                device.bus_id() == selector || device.serial_number() == Some(selector)
            });
        }
        if devices.len() != 1 {
            return Err(error("Expected one selected proxy camera"));
        }
        let Device::Proxy(device) = devices.remove(0) else {
            return Err(error("Expected proxy camera"));
        };
        Ok(Some(Self::Proxy {
            device: Box::new(device),
            ready: std::env::var_os("FUJIMOD_USB_AGENT_READY").map(Into::into),
            clients: Mutex::new(HashMap::new()),
            claimed: Mutex::new(BTreeSet::new()),
        }))
    }
    pub fn descriptor(&self) -> &fujimod_usb_proxy::Device {
        match self {
            Self::Native { owner, .. } => &owner.descriptor,
            Self::Proxy { device, .. } => &device.descriptor,
        }
    }
    pub fn present(&self) -> bool {
        match self {
            Self::Native { owner, .. } => owner.status().present,
            Self::Proxy { ready, .. } => ready.as_ref().is_none_or(|path| path.is_file()),
        }
    }
    pub fn fenced(&self) -> bool {
        match self {
            Self::Native { owner, .. } => owner.status().fenced,
            Self::Proxy { .. } => false,
        }
    }
    pub fn owns(&self, id: &str, device: &str) -> bool {
        match self {
            Self::Native { exporter, .. } => exporter.owns(id, device),
            Self::Proxy { claimed, .. } => {
                self.descriptor().id == device && claimed.lock().unwrap().contains(id)
            }
        }
    }
    pub fn abandon(&self, id: &str) -> bool {
        match self {
            Self::Native { exporter, .. } => exporter.abandon(id),
            Self::Proxy {
                clients, claimed, ..
            } => {
                // Dropping an unclosed upstream controller lets that USB owner
                // fence its own session. Never send recovery or replay here.
                clients.lock().unwrap().remove(id);
                claimed.lock().unwrap().remove(id)
            }
        }
    }
    pub async fn execute(&self, id: &str, message: Message) -> Completion {
        let result: crate::Result<Completion> = async {
            match self {
                Self::Native { exporter, .. } => {
                    let raw = exporter.handle(id, &message.encode()?).await?;
                    match Message::decode(&raw)? {
                        Message::Result { completion, .. } => Ok(completion),
                        _ => Err(error("Invalid exporter reply")),
                    }
                }
                Self::Proxy {
                    device,
                    ready,
                    clients,
                    claimed,
                } => {
                    let Message::Execute {
                        device: target,
                        op,
                        timeout_ms,
                        ..
                    } = message
                    else {
                        return Err(error("Expected USB execute message"));
                    };
                    let slot = clients
                        .lock()
                        .unwrap()
                        .entry(id.into())
                        .or_insert_with(|| Arc::new(async_lock::Mutex::new(None)))
                        .clone();
                    let mut client = slot.lock().await;
                    if client.is_none() {
                        *client = Some(if let Some(path) = ready {
                            #[derive(serde::Deserialize)]
                            struct Endpoint {
                                url: String,
                                token: String,
                            }
                            let endpoint: Endpoint =
                                serde_json::from_slice(&std::fs::read(path).map_err(error)?)
                                    .map_err(error)?;
                            Client::connect(&endpoint.url, &endpoint.token).await?
                        } else {
                            device.connect().await?
                        });
                    }
                    let opening = matches!(op, Operation::Open { .. });
                    let closing = matches!(op, Operation::Close);
                    let reply = client
                        .as_mut()
                        .unwrap()
                        .request(&target, op, Duration::from_millis(timeout_ms.into()))
                        .await?;
                    if reply.error.is_none() {
                        if opening {
                            claimed.lock().unwrap().insert(id.into());
                        }
                        if closing {
                            claimed.lock().unwrap().remove(id);
                            *client = None;
                        }
                    }
                    Ok(reply)
                }
            }
        }
        .await;
        result.unwrap_or_else(|failure| failed(failure.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_tungstenite::tungstenite::Message as Frame;

    async fn server() -> (String, Arc<Mutex<Vec<Operation>>>) {
        let listener = async_net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/usb-proxy/ws", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        std::thread::spawn(move || {
            futures_lite::future::block_on(async move {
                loop {
                    let (stream, _) = listener.accept().await.unwrap();
                    let calls = calls.clone();
                    std::thread::spawn(move || {
                        futures_lite::future::block_on(async move {
                            let mut socket = async_tungstenite::accept_async(stream).await.unwrap();
                            let mut pipeline = Pipeline::default();
                            while let Some(Ok(Frame::Binary(raw))) = socket.next().await {
                                let message = Message::decode(&raw).unwrap();
                                let reply = if matches!(message, Message::Hello { .. }) {
                                    Some(Message::Ready {
                                        version: VERSION,
                                        id: "test".into(),
                                    })
                                } else {
                                    pipeline
                                        .handle(message, |message| async {
                                            let Message::Execute { op, .. } = message else {
                                                unreachable!()
                                            };
                                            calls.lock().unwrap().push(op.clone());
                                            if matches!(op, Operation::List) {
                                                Completion {
                                                    devices: vec![fujimod_usb_proxy::Device {
                                                        id: "camera".into(),
                                                        vendor_id: 0x04cb,
                                                        product_id: 0xff80,
                                                        manufacturer: None,
                                                        product: None,
                                                        serial: None,
                                                        interfaces: vec![],
                                                        status: None,
                                                    }],
                                                    ..Default::default()
                                                }
                                            } else {
                                                Completion::default()
                                            }
                                        })
                                        .await
                                        .unwrap()
                                };
                                if let Some(reply) = reply
                                    && socket
                                        .send(Frame::Binary(reply.encode().unwrap().into()))
                                        .await
                                        .is_err()
                                {
                                    break;
                                }
                            }
                        })
                    });
                }
            })
        });
        (url, observed)
    }
    fn message(op: Operation) -> Message {
        Message::Execute {
            seq: 1,
            device: "camera".into(),
            op,
            timeout_ms: 1000,
        }
    }
    #[test]
    fn proxy_share_forwards_sessions_and_reloads_the_agent_endpoint() {
        futures_lite::future::block_on(async {
            let (url, first) = server().await;
            let device = Client::discover(&url, "test").await.unwrap().remove(0);
            let ready = std::env::temp_dir()
                .join(format!("fujimod-share-source-{}.json", std::process::id()));
            std::fs::write(
                &ready,
                serde_json::to_vec(&serde_json::json!({"url": url, "token": "test"})).unwrap(),
            )
            .unwrap();
            let source = Source::Proxy {
                device: Box::new(device),
                ready: Some(ready.clone()),
                clients: Mutex::default(),
                claimed: Mutex::default(),
            };
            let open = Operation::Open {
                configuration: 1,
                interface: 0,
                alternate: 0,
            };
            assert!(
                source
                    .execute("one", message(open.clone()))
                    .await
                    .error
                    .is_none()
            );
            assert!(source.owns("one", "camera"));
            assert!(
                source
                    .execute("one", message(Operation::Close))
                    .await
                    .error
                    .is_none()
            );
            assert!(!source.owns("one", "camera"));
            assert!(!source.abandon("one"));
            assert!(matches!(
                first.lock().unwrap().as_slice(),
                [Operation::List, Operation::Open { .. }, Operation::Close]
            ));
            let (next_url, second) = server().await;
            std::fs::write(
                &ready,
                serde_json::to_vec(&serde_json::json!({"url": next_url, "token": "test"})).unwrap(),
            )
            .unwrap();
            assert!(source.execute("two", message(open)).await.error.is_none());
            assert!(source.abandon("two"));
            assert!(!source.owns("two", "camera"));
            // An unclosed controller is dropped; no invented Close or recovery.
            assert!(matches!(
                second.lock().unwrap().as_slice(),
                [Operation::Open { .. }]
            ));
            std::fs::remove_file(&ready).unwrap();
            assert!(!source.present());
        });
    }
}
