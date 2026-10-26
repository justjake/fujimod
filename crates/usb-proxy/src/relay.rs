//! Optional relay service. It routes opaque transfers and owns device leases.
//! Each controller is one stream to its exporter: the relay checks leases and
//! passes the pipeline through without waiting, so the exporter alone orders,
//! times and discards operations (see PIPELINE.md).
use crate::{Error, Result, protocol::*};
use axum::{
    Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message as Ws, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, mpsc};

/// One controller stream's message for an exporter socket.
type Work = (u64, Message);
struct Exporter {
    devices: Vec<Device>,
    sender: mpsc::Sender<Work>,
    /// Controller streams bound to this exporter, by stream number.
    streams: HashMap<u64, mpsc::UnboundedSender<Message>>,
}
struct Inner {
    exporters: HashMap<String, Exporter>,
    leases: HashMap<String, String>,
}
#[derive(Clone)]
struct Relay {
    origins: Arc<Vec<String>>,
    token: Arc<String>,
    inner: Arc<Mutex<Inner>>,
    counter: Arc<AtomicU64>,
}
fn failure(seq: u64, error: impl Into<String>) -> Message {
    Message::Result {
        seq,
        completion: Completion {
            error: Some(error.into()),
            ..Completion::default()
        },
    }
}
fn equal(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}
/// Mount on loopback behind HTTPS/WSS (for example Tailscale Serve). The token
/// is required for both roles and is never placed in a URL query or server log.
pub fn router(token: String, origins: Vec<String>) -> Result<Router> {
    if token.len() < 32 {
        return Err(Error("relay token must have at least 32 characters".into()));
    }
    let relay = Relay {
        origins: Arc::new(origins),
        token: Arc::new(token),
        inner: Arc::new(Mutex::new(Inner {
            exporters: HashMap::new(),
            leases: HashMap::new(),
        })),
        counter: Arc::new(AtomicU64::new(1)),
    };
    Ok(Router::new()
        .route(
            "/usb-proxy/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/usb-proxy/exporter.js",
            get(|| async {
                (
                    [("content-type", "text/javascript")],
                    include_str!("../web/build/exporter.js"),
                )
            }),
        )
        .route("/usb-proxy/bindings/{*path}", get(binding_asset))
        .route("/usb-proxy/ws", get(upgrade))
        .with_state(relay)
        .merge(crate::signaling::router()))
}
// Generated bindings are served beside the relay bundle. Keeping them as build
// artifacts avoids a dependency cycle between the Wasm SDK and this crate.
async fn binding_asset(Path(path): Path<String>) -> axum::response::Response {
    let allowed = matches!(
        path.as_str(),
        "dist/index.js" | "pkg/fujimod.js" | "pkg/service-values.js" | "pkg/fujimod_bg.wasm"
    ) || (path.starts_with("pkg/snippets/") && path.ends_with(".js"));
    if !allowed
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.contains('\\')
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let root = std::env::var_os("FUJIMOD_USB_PROXY_BINDINGS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bindings/javascript")
        });
    match std::fs::read(root.join(&path)) {
        Ok(bytes) => (
            [(
                "content-type",
                if path.ends_with(".wasm") {
                    "application/wasm"
                } else {
                    "text/javascript"
                },
            )],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn upgrade(
    State(relay): State<Relay>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    // Native clients omit Origin; browser clients need an allowed origin.
    if let Some(origin) = headers.get("origin") {
        let origin = origin.to_str().unwrap_or("");
        let host = headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if origin != format!("https://{host}")
            && origin != format!("http://{host}")
            && !relay.origins.iter().any(|allowed| allowed == origin)
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    ws.max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(|socket| session(relay, socket))
}
async fn receive(socket: &mut WebSocket) -> Option<Message> {
    loop {
        match socket.recv().await? {
            Ok(Ws::Binary(bytes)) => return Message::decode(&bytes).ok(),
            Ok(Ws::Ping(_)) | Ok(Ws::Pong(_)) => continue,
            _ => return None,
        }
    }
}
async fn send(socket: &mut WebSocket, message: &Message) -> bool {
    let Ok(bytes) = message.encode() else {
        return false;
    };
    socket.send(Ws::Binary(bytes.into())).await.is_ok()
}
async fn session(relay: Relay, mut socket: WebSocket) {
    let Ok(Some(Message::Hello {
        version: VERSION,
        role,
        token,
        ..
    })) = tokio::time::timeout(Duration::from_secs(10), receive(&mut socket)).await
    else {
        return;
    };
    if !equal(&token, &relay.token) || !["exporter", "controller"].contains(&role.as_str()) {
        return;
    }
    let id = relay.counter.fetch_add(1, Ordering::Relaxed).to_string();
    if !send(
        &mut socket,
        &Message::Ready {
            version: VERSION,
            id: id.clone(),
        },
    )
    .await
    {
        return;
    }
    if role == "exporter" {
        exporter(relay, socket, id).await;
    } else {
        controller(relay, socket, id).await;
    }
}
async fn exporter(relay: Relay, socket: WebSocket, id: String) {
    let (tx, mut rx) = mpsc::channel::<Work>(1024);
    relay.inner.lock().await.exporters.insert(
        id.clone(),
        Exporter {
            devices: vec![],
            sender: tx,
            streams: HashMap::new(),
        },
    );
    let (mut output, mut input) = socket.split();
    loop {
        tokio::select! {
            work = rx.recv() => {
                let Some((stream, message)) = work else { break };
                let Ok(bytes) = (Message::Routed { stream, message: Box::new(message) }).encode() else { break };
                if output.send(Ws::Binary(bytes.into())).await.is_err() { break; }
            }
            frame = input.next() => {
                match frame {
                    Some(Ok(Ws::Binary(bytes))) => match Message::decode(&bytes) {
                        Ok(Message::Inventory { mut devices }) if devices.len() <= 128 => {
                            // Never allow a browser to overwrite another exporter's namespace.
                            if devices.iter().any(|d| d.id.is_empty() || d.id.contains('/') || d.id.len() > 128) { break; }
                            for device in &mut devices { device.id = format!("{id}/{}", device.id); }
                            if let Some(peer) = relay.inner.lock().await.exporters.get_mut(&id) { peer.devices = devices; }
                        }
                        Ok(Message::Routed { stream, message }) if matches!(
                            *message,
                            Message::Result { .. } | Message::ExpectFailed { .. } | Message::Synced { .. }
                        ) => {
                            if let Some(stream) = relay.inner.lock().await.exporters.get(&id).and_then(|p| p.streams.get(&stream)) {
                                let _ = stream.send(*message);
                            }
                        }
                        _ => break,
                    },
                    Some(Ok(Ws::Ping(bytes))) => { if output.send(Ws::Pong(bytes)).await.is_err() { break; } }
                    Some(Ok(Ws::Pong(_))) => (),
                    _ => break,
                }
            }
        }
    }
    let mut inner = relay.inner.lock().await;
    // Dropping stream senders ends their controllers: completion unknown,
    // never fabricated USB success.
    inner.exporters.remove(&id);
    inner
        .leases
        .retain(|device, _| !device.starts_with(&format!("{id}/")));
}
/// A controller's stream, once its first operation names an exporter.
struct Bound {
    exporter: String,
    sender: mpsc::Sender<Work>,
}
async fn controller(relay: Relay, mut socket: WebSocket, owner: String) {
    let stream = relay.counter.fetch_add(1, Ordering::Relaxed);
    let (results, mut inbound) = mpsc::unbounded_channel::<Message>();
    let mut bound: Option<Bound> = None;
    // Close operations in flight, by sequence: a successful one ends the lease.
    let mut closing: HashMap<u64, String> = HashMap::new();
    loop {
        tokio::select! {
            message = receive(&mut socket) => {
                let Some(message) = message else { break };
                if message.validate().is_err() { break; }
                match message {
                    Message::Execute { seq, device, timeout_ms, op } => {
                        if matches!(op, Operation::List) {
                            let devices = relay.inner.lock().await.exporters.values().flat_map(|p| p.devices.clone()).collect();
                            if !send(&mut socket, &Message::Result { seq, completion: Completion { devices, ..Completion::default() } }).await { break; }
                            continue;
                        }
                        let routed = admit(&relay, &owner, stream, &results, &mut bound, &device, &op).await;
                        let Ok(local) = routed else {
                            let _ = send(&mut socket, &failure(seq, routed.unwrap_err())).await;
                            break;
                        };
                        if matches!(op, Operation::Close) { closing.insert(seq, device); }
                        let sender = &bound.as_ref().unwrap().sender;
                        if sender.send((stream, Message::Execute { seq, device: local, timeout_ms, op })).await.is_err() { break; }
                    }
                    Message::Sync { seq } if bound.is_none() => {
                        if !send(&mut socket, &Message::Synced { seq, discarded: 0 }).await { break; }
                    }
                    message @ (Message::Sync { .. } | Message::Expect { .. }) => {
                        let Some(b) = &bound else { break };
                        if b.sender.send((stream, message)).await.is_err() { break; }
                    }
                    message if message.is_notice() => {
                        if let Some(b) = &bound && b.sender.send((stream, message)).await.is_err() { break; }
                    }
                    _ => break,
                }
            }
            result = inbound.recv() => {
                let Some(result) = result else { break };
                let mut failed = false;
                if let Message::Result { seq, completion } = &result {
                    failed = completion.error.is_some();
                    if let Some(device) = closing.remove(seq) && !failed {
                        relay.inner.lock().await.leases.remove(&device);
                    }
                }
                // A USB failure ends the controller, as at a WebRTC exporter.
                if !send(&mut socket, &result).await || failed { break; }
            }
        }
    }
    let Some(bound) = bound else { return };
    // Release host claims only, never send camera protocol cleanup. Retain the
    // lease after an uncertain close until the browser exporter disconnects.
    let owned: Vec<_> = relay
        .inner
        .lock()
        .await
        .leases
        .iter()
        .filter(|(_, v)| *v == &owner)
        .map(|(k, _)| k.clone())
        .collect();
    let mut seq = u64::MAX / 2;
    for device in owned {
        let Some((_, local)) = device.split_once('/') else {
            continue;
        };
        // A sync first ends any failed pipeline's discarding.
        seq += 2;
        let close = Message::Execute {
            seq,
            device: local.into(),
            timeout_ms: 5000,
            op: Operation::Close,
        };
        if bound
            .sender
            .send((stream, Message::Sync { seq: seq - 1 }))
            .await
            .is_err()
            || bound.sender.send((stream, close)).await.is_err()
        {
            break;
        }
        let closed = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(message) = inbound.recv().await {
                if let Message::Result {
                    seq: done,
                    completion,
                } = message
                    && done == seq
                {
                    return completion.error.is_none();
                }
            }
            false
        })
        .await;
        if closed == Ok(true) {
            relay.inner.lock().await.leases.remove(&device);
        }
    }
    let _ = bound
        .sender
        .send((
            stream,
            Message::Goodbye {
                reason: "controller ended".into(),
            },
        ))
        .await;
    if let Some(peer) = relay.inner.lock().await.exporters.get_mut(&bound.exporter) {
        peer.streams.remove(&stream);
    }
}
/// Check the route and lease of one operation, binding the stream to its
/// exporter on first use. Returns the exporter-local device ID.
async fn admit(
    relay: &Relay,
    owner: &str,
    stream: u64,
    results: &mpsc::UnboundedSender<Message>,
    bound: &mut Option<Bound>,
    device: &str,
    op: &Operation,
) -> std::result::Result<String, String> {
    let Some((exporter, local)) = device.split_once('/') else {
        return Err("invalid device route".into());
    };
    let mut inner = relay.inner.lock().await;
    let Some(peer) = inner.exporters.get_mut(exporter) else {
        return Err("exporter disconnected; completion unknown".into());
    };
    match bound {
        Some(b) if b.exporter != exporter => return Err("one controller uses one exporter".into()),
        Some(_) => (),
        None => {
            peer.streams.insert(stream, results.clone());
            *bound = Some(Bound {
                exporter: exporter.into(),
                sender: peer.sender.clone(),
            });
        }
    }
    // Echo through the exporter, without requiring a USB lease.
    if matches!(op, Operation::Probe) {
        return Ok(local.into());
    }
    let exists = peer.devices.iter().any(|d| d.id == device);
    let allowed = if matches!(op, Operation::Open { .. }) {
        exists
            && inner
                .leases
                .insert(device.into(), owner.into())
                .is_none_or(|previous| {
                    // Keep another controller's lease.
                    let mine = previous == owner;
                    if !mine {
                        inner.leases.insert(device.into(), previous);
                    }
                    mine
                })
    } else {
        inner.leases.get(device).map(String::as_str) == Some(owner)
    };
    if allowed {
        Ok(local.into())
    } else {
        Err("device unavailable or leased by another controller".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Client, Outcome};
    use async_tungstenite::{smol::connect_async, tungstenite::Message as Wire};
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    async fn server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/usb-proxy/ws", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router(TOKEN.into(), vec![]).unwrap())
                .await
                .unwrap();
        });
        (url, server)
    }
    struct AbortOnDrop(tokio::task::JoinHandle<()>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    fn wire(message: Message) -> Wire {
        Wire::Binary(message.encode().unwrap().into())
    }
    /// A pipelined exporter: in order, discarding after a failure until Sync.
    /// IN transfers return `length` bytes counting up from the sequence number.
    async fn fixture(
        url: &str,
        rtt: Duration,
    ) -> (
        String,
        tokio::task::JoinHandle<()>,
        mpsc::UnboundedReceiver<Operation>,
    ) {
        let (mut socket, _) = connect_async(url).await.unwrap();
        socket
            .send(wire(Message::Hello {
                reason: None,
                version: VERSION,
                role: "exporter".into(),
                token: TOKEN.into(),
            }))
            .await
            .unwrap();
        let ready = Message::decode(&socket.next().await.unwrap().unwrap().into_data()).unwrap();
        let Message::Ready { id, .. } = ready else {
            panic!()
        };
        socket
            .send(wire(Message::Inventory {
                devices: vec![Device {
                    id: "camera".into(),
                    vendor_id: 0x04cb,
                    product_id: 0xff80,
                    manufacturer: None,
                    product: None,
                    serial: None,
                    interfaces: vec![],
                    status: None,
                }],
            }))
            .await
            .unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        let (mut output, mut input) = socket.split();
        // Pipe latency: each message arrives rtt/2 after it was sent, however
        // many are in flight.
        let (arrive, mut arrivals) = mpsc::unbounded_channel::<(tokio::time::Instant, Message)>();
        let (depart, mut departures) = mpsc::unbounded_channel::<(tokio::time::Instant, Message)>();
        let reader = tokio::spawn(async move {
            while let Some(Ok(frame)) = input.next().await {
                if let Ok(message) = Message::decode(&frame.into_data()) {
                    let _ = arrive.send((tokio::time::Instant::now() + rtt / 2, message));
                }
            }
        });
        let writer = tokio::spawn(async move {
            while let Some((at, message)) = departures.recv().await {
                tokio::time::sleep_until(at).await;
                if output.send(wire(message)).await.is_err() {
                    break;
                }
            }
        });
        let task = tokio::spawn(async move {
            let _tasks = (AbortOnDrop(reader), AbortOnDrop(writer));
            let (mut failed, mut discarded) =
                (HashMap::<u64, bool>::new(), HashMap::<u64, u64>::new());
            let mut results: HashMap<(u64, u64), Vec<u8>> = HashMap::new();
            while let Some((at, message)) = arrivals.recv().await {
                tokio::time::sleep_until(at).await;
                let Message::Routed { stream, message } = message else {
                    continue;
                };
                let reply = match *message {
                    Message::Execute { .. } | Message::Expect { .. }
                        if failed.get(&stream) == Some(&true) =>
                    {
                        *discarded.entry(stream).or_default() += 1;
                        None
                    }
                    Message::Execute {
                        seq,
                        op,
                        timeout_ms,
                        ..
                    } => {
                        if matches!(op, Operation::TransferIn { length: 10, .. }) {
                            // The USB budget must not be inflated with network time.
                            assert_eq!(timeout_ms, 10);
                        }
                        if !matches!(op, Operation::Probe) {
                            tx.send(op.clone()).unwrap();
                        }
                        let completion = match op {
                            Operation::TransferIn { length: 7, .. } => Completion {
                                data: vec![1, 2, 3],
                                transferred: 3,
                                error: Some("stall".into()),
                                ..Completion::default()
                            },
                            // A transfer the device never answers: the exporter
                            // reports its own USB deadline, as browsers do.
                            Operation::TransferIn { length: 9, .. } => {
                                tokio::time::sleep(Duration::from_millis(timeout_ms.into())).await;
                                Completion {
                                    error: Some("USB deadline expired; completion unknown".into()),
                                    ..Completion::default()
                                }
                            }
                            Operation::TransferIn { length, .. } => {
                                let data: Vec<u8> =
                                    (0..length).map(|i| (seq as usize + i) as u8).collect();
                                results.insert((stream, seq), data.clone());
                                Completion {
                                    transferred: data.len(),
                                    data,
                                    ..Completion::default()
                                }
                            }
                            Operation::TransferOut { data, .. } => Completion {
                                transferred: data.len(),
                                ..Completion::default()
                            },
                            _ => Completion::default(),
                        };
                        if completion.error.is_some() {
                            failed.insert(stream, true);
                        }
                        Some(Message::Result { seq, completion })
                    }
                    Message::Expect {
                        seq,
                        of,
                        offset,
                        bytes,
                        mask,
                        length,
                        alternatives,
                    } => {
                        let data = results.get(&(stream, of)).cloned().unwrap_or_default();
                        let mask = mask.as_deref().map(|m| m.as_slice());
                        if length.is_none_or(|n| data.len() == n as usize)
                            && (expectation_holds(&data, offset as usize, &bytes, mask)
                                || alternatives
                                    .iter()
                                    .any(|a| expectation_holds(&data, offset as usize, a, mask)))
                        {
                            None
                        } else {
                            failed.insert(stream, true);
                            let end = (offset as usize + bytes.len()).min(data.len());
                            Some(Message::ExpectFailed {
                                seq,
                                actual: data.get(offset as usize..end).unwrap_or_default().to_vec(),
                            })
                        }
                    }
                    Message::Sync { seq } => {
                        failed.insert(stream, false);
                        Some(Message::Synced {
                            seq,
                            discarded: discarded.remove(&stream).unwrap_or(0),
                        })
                    }
                    _ => None,
                };
                if let Some(reply) = reply
                    && depart
                        .send((
                            tokio::time::Instant::now() + rtt / 2,
                            Message::Routed {
                                stream,
                                message: Box::new(reply),
                            },
                        ))
                        .is_err()
                {
                    break;
                }
            }
        });
        (format!("{id}/camera"), task, rx)
    }
    async fn available(url: &str) -> Client {
        let mut client = Client::connect(url, TOKEN).await.unwrap();
        for _ in 0..100 {
            if !client.devices().await.unwrap().is_empty() {
                return client;
            }
            tokio::task::yield_now().await;
        }
        panic!("inventory not published")
    }
    fn open() -> Operation {
        Operation::Open {
            configuration: 1,
            interface: 2,
            alternate: 3,
        }
    }
    fn read(length: usize) -> Operation {
        Operation::TransferIn {
            endpoint: 0x82,
            length,
        }
    }
    #[tokio::test]
    async fn authentication_and_version_are_required() {
        let (url, server) = server().await;
        assert!(Client::connect(&url, "wrong").await.is_err());
        let (mut socket, _) = connect_async(&url).await.unwrap();
        socket
            .send(wire(Message::Hello {
                reason: None,
                version: 99,
                role: "controller".into(),
                token: TOKEN.into(),
            }))
            .await
            .unwrap();
        assert!(!matches!(socket.next().await, Some(Ok(Wire::Binary(_)))));
        server.abort();
    }
    #[tokio::test]
    async fn leases_exclude_other_controllers_and_preserve_partial_failure() {
        let (url, server) = server().await;
        let (route, exporter, mut events) = fixture(&url, Duration::ZERO).await;
        let mut first = available(&url).await;
        let mut second = Client::connect(&url, TOKEN).await.unwrap();
        assert!(
            first
                .request(&route, open(), Duration::from_secs(1))
                .await
                .unwrap()
                .error
                .is_none()
        );
        assert!(matches!(events.recv().await, Some(Operation::Open { .. })));
        assert!(
            second
                .request(&route, open(), Duration::from_secs(1))
                .await
                .unwrap()
                .error
                .is_some()
        );
        assert!(events.try_recv().is_err());
        let partial = first
            .request(&route, read(7), Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(partial.error.as_deref(), Some("stall"));
        assert_eq!(partial.data, [1, 2, 3]);
        assert_eq!(partial.transferred, 3);
        assert!(
            first
                .request(&route, Operation::Close, Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(matches!(
            events.recv().await,
            Some(Operation::TransferIn { .. })
        ));
        // The relay's own cleanup releases the host claim.
        assert!(matches!(events.recv().await, Some(Operation::Close)));
        exporter.abort();
        server.abort();
    }
    #[tokio::test]
    async fn pipelines_stream_without_round_trips_and_stop_at_failures() {
        let rtt = Duration::from_millis(100);
        let (url, server) = server().await;
        let (route, exporter, mut events) = fixture(&url, rtt).await;
        let mut client = available(&url).await;
        client
            .request(&route, open(), Duration::from_secs(1))
            .await
            .unwrap();
        // 64 operations in one round trip, not 64.
        let started = std::time::Instant::now();
        let mut reads = Vec::new();
        for _ in 0..32 {
            client
                .submit(
                    &route,
                    Operation::TransferOut {
                        endpoint: 1,
                        data: vec![1; 12],
                    },
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            reads.push(
                client
                    .submit(&route, read(16), Duration::from_secs(1))
                    .await
                    .unwrap(),
            );
        }
        let outcomes = client.sync().await.unwrap();
        assert!(started.elapsed() < rtt * 4, "{:?}", started.elapsed());
        assert_eq!(outcomes.len(), 64);
        let Outcome::Completed(first) = &outcomes[1].1 else {
            panic!()
        };
        assert_eq!(first.data[0], reads[0] as u8);
        // An expectation that holds lets the rest run; one that fails discards
        // everything after it, without touching USB, until the sync.
        let of = client
            .submit(&route, read(4), Duration::from_secs(1))
            .await
            .unwrap();
        let held = client
            .expect(of, 1, vec![of as u8 + 1], None, None, vec![])
            .await
            .unwrap();
        let failed = client
            .expect(
                of,
                0,
                vec![0xee, 0xff],
                Some(vec![0xff, 0x0f]),
                None,
                vec![],
            )
            .await
            .unwrap();
        let skipped = client
            .submit(
                &route,
                Operation::TransferOut {
                    endpoint: 1,
                    data: vec![2],
                },
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        let outcomes: HashMap<u64, Outcome> = client.sync().await.unwrap().into_iter().collect();
        assert_eq!(outcomes[&held], Outcome::Held);
        assert_eq!(
            outcomes[&failed],
            Outcome::Failed(vec![of as u8, of as u8 + 1])
        );
        assert_eq!(outcomes[&skipped], Outcome::Discarded);
        while let Ok(op) = events.try_recv() {
            assert!(!matches!(op, Operation::TransferOut { ref data, .. } if data == &[2]));
        }
        // The pipeline runs again after the sync.
        assert!(
            client
                .request(&route, read(4), Duration::from_secs(1))
                .await
                .unwrap()
                .error
                .is_none()
        );
        exporter.abort();
        server.abort();
    }
    #[tokio::test]
    async fn ten_ms_usb_budget_survives_150_ms_exporter_round_trip() {
        let (url, server) = server().await;
        let (route, exporter, mut events) = fixture(&url, Duration::from_millis(150)).await;
        let mut client = available(&url).await;
        client
            .request(&route, open(), Duration::from_millis(10))
            .await
            .unwrap();
        assert!(matches!(events.recv().await, Some(Operation::Open { .. })));
        let completion = client
            .request(&route, read(10), Duration::from_millis(10))
            .await
            .unwrap();
        assert!(completion.error.is_none(), "{:?}", completion.error);
        assert!(matches!(
            events.recv().await,
            Some(Operation::TransferIn { length: 10, .. })
        ));
        client
            .request(&route, Operation::Close, Duration::from_millis(10))
            .await
            .unwrap();
        assert!(matches!(events.recv().await, Some(Operation::Close)));
        assert!(events.try_recv().is_err());
        exporter.abort();
        server.abort();
    }
    #[tokio::test]
    async fn canceled_request_fences_before_more_usb() {
        let (url, server) = server().await;
        let (route, exporter, mut events) = fixture(&url, Duration::from_millis(150)).await;
        let mut client = available(&url).await;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                client.request(&route, open(), Duration::from_millis(10))
            )
            .await
            .is_err()
        );
        let error = client
            .request(&route, open(), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("canceled"), "{error}");
        assert!(matches!(events.recv().await, Some(Operation::Open { .. })));
        assert!(events.try_recv().is_err());
        exporter.abort();
        server.abort();
    }
    #[tokio::test]
    async fn deadline_fences_without_replaying_transfer() {
        let (url, server) = server().await;
        let (route, exporter, mut events) = fixture(&url, Duration::ZERO).await;
        let mut client = available(&url).await;
        client
            .request(&route, open(), Duration::from_secs(1))
            .await
            .unwrap();
        events.recv().await;
        let expired = client
            .request(&route, read(9), Duration::from_millis(30))
            .await
            .unwrap();
        assert!(expired.error.unwrap().contains("completion unknown"));
        assert!(matches!(
            events.recv().await,
            Some(Operation::TransferIn { length: 9, .. })
        ));
        assert!(
            client
                .request(&route, Operation::Close, Duration::from_secs(1))
                .await
                .is_err()
        );
        // Only the relay's host-only release follows; the transfer is not replayed.
        assert!(matches!(events.recv().await, Some(Operation::Close)));
        assert!(events.try_recv().is_err());
        exporter.abort();
        server.abort();
    }
}
