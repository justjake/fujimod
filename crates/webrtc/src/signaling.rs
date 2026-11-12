//! Opaque SDP rendezvous for the development relay. Persistent rooms survive
//! their owner's socket and are authenticated by a hash of the owner key.
use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, mpsc};

type Sender = mpsc::Sender<Value>;
struct Room {
    owner_hash: Option<[u8; 32]>,
    host: Option<Sender>,
    clients: HashMap<String, Sender>,
    offline_since: Option<Instant>,
}
#[derive(Clone)]
struct Hub {
    rooms: Arc<Mutex<HashMap<String, Room>>>,
    serial: Arc<AtomicU64>,
    idle: Duration,
    /// Sent in every `ready` frame, as the Worker sends its TURN servers. Null sends none.
    ice_servers: Arc<Value>,
}
impl Hub {
    fn new(idle: Duration, ice_servers: Value) -> Self {
        Self {
            rooms: Arc::new(Mutex::new(HashMap::new())),
            serial: Arc::new(AtomicU64::new(0)),
            idle,
            ice_servers: Arc::new(ice_servers),
        }
    }
}

pub fn router() -> Router {
    router_with_share_idle(Duration::from_secs(7 * 24 * 60 * 60))
}

/// Create a development signaling service with a configurable persistent-room
/// idle lifetime. The default is seven days, matching the Worker service.
pub fn router_with_share_idle(idle: Duration) -> Router {
    router_with_hub(idle, Value::Null)
}

/// Create a development signaling service that also gives every peer `ice_servers`.
#[cfg(test)]
pub fn router_with_ice_servers(idle: Duration, ice_servers: Value) -> Router {
    router_with_hub(idle, ice_servers)
}

fn router_with_hub(idle: Duration, ice_servers: Value) -> Router {
    Router::new()
        .route(
            "/usb-proxy/signal",
            get(|State(hub): State<Hub>, ws: WebSocketUpgrade| async {
                ws.max_message_size(65536)
                    .max_frame_size(65536)
                    .on_upgrade(move |socket| serve(hub, socket))
            }),
        )
        .with_state(Hub::new(idle, ice_servers))
}

async fn first(socket: &mut WebSocket) -> Option<Value> {
    match socket.recv().await? {
        Ok(Message::Text(text)) => serde_json::from_str(&text).ok(),
        _ => None,
    }
}

async fn reject(socket: &mut WebSocket, message: &str) {
    let _ = socket
        .send(Message::Text(
            json!({"type":"error","message":message}).to_string().into(),
        ))
        .await;
    let _ = socket.send(Message::Close(None)).await;
}

async fn serve(hub: Hub, mut socket: WebSocket) {
    let Ok(Some(join)) = tokio::time::timeout(Duration::from_secs(10), first(&mut socket)).await
    else {
        return;
    };
    let Some(room) = join["room"].as_str().filter(|id| {
        id.len() == 43
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    }) else {
        reject(&mut socket, "Invalid room").await;
        return;
    };
    let room = room.to_owned();
    let host = join["role"] == "host";
    if join["type"] != "join" || (!host && join["role"] != "client") {
        reject(&mut socket, "Invalid room handshake").await;
        return;
    }
    if join.get("owner_key").is_some() && !join["owner_key"].is_string() {
        reject(&mut socket, "Invalid owner key").await;
        return;
    }
    let owner_key = join["owner_key"].as_str();
    if owner_key.is_some_and(|key| key.is_empty() || key.len() > 1024) {
        reject(&mut socket, "Invalid owner key").await;
        return;
    }
    let owner_hash = owner_key.map(|key| <[u8; 32]>::from(Sha256::digest(key.as_bytes())));
    let peer = if host {
        "host".to_owned()
    } else {
        hub.serial.fetch_add(1, Ordering::Relaxed).to_string()
    };
    let (tx, mut rx) = mpsc::channel::<Value>(16);
    let accepted = {
        let mut rooms = hub.rooms.lock().await;
        if host {
            if !rooms.contains_key(&room) && rooms.len() >= 1024 {
                Err("Too many rooms")
            } else {
                match rooms.get_mut(&room) {
                    None => {
                        rooms.insert(
                            room.clone(),
                            Room {
                                owner_hash,
                                host: Some(tx.clone()),
                                clients: HashMap::new(),
                                offline_since: None,
                            },
                        );
                        Ok(())
                    }
                    Some(active) => match (owner_hash, active.owner_hash) {
                        (None, None) => Err("Room already has an owner"),
                        (Some(_), None) => Err("Owner key not valid for this room"),
                        (None, Some(_)) => Err("Owner key required"),
                        (Some(presented), Some(stored)) if presented != stored => {
                            Err("Owner key does not match room")
                        }
                        (Some(_), Some(_)) if active.host.is_some() => {
                            Err("Room already has an owner")
                        }
                        (Some(_), Some(_)) => {
                            active.host = Some(tx.clone());
                            active.offline_since = None;
                            Ok(())
                        }
                    },
                }
            }
        } else {
            match rooms.get_mut(&room) {
                None => Err("Camera share unavailable"),
                Some(active) if active.owner_hash.is_some() && active.host.is_none() => {
                    Err("Camera share offline")
                }
                Some(active) if active.clients.len() >= 16 => Err("Room is full"),
                Some(active) => {
                    if active.host.as_ref().is_some_and(|owner| {
                        owner
                            .try_send(json!({"type":"joined","peer":peer}))
                            .is_err()
                    }) {
                        Err("Room busy")
                    } else if active.host.is_none() {
                        Err("Camera share unavailable")
                    } else {
                        active.clients.insert(peer.clone(), tx.clone());
                        Ok(())
                    }
                }
            }
        }
    };
    if let Err(message) = accepted {
        reject(&mut socket, message).await;
        return;
    }

    let (mut output, mut input) = socket.split();
    let mut ready = json!({"type":"ready", "peer":peer});
    if !hub.ice_servers.is_null() {
        ready["ice_servers"] = (*hub.ice_servers).clone();
    }
    let mut alive = output
        .send(Message::Text(ready.to_string().into()))
        .await
        .is_ok();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    let mut answered = true;
    let mut ended = false;
    while alive {
        tokio::select! {
            _ = heartbeat.tick() => {
                if !answered { break; }
                answered = false;
                alive = output.send(Message::Ping(vec![].into())).await.is_ok();
            }
            frame = rx.recv() => {
                let Some(frame) = frame else { break };
                alive = output.send(Message::Text(frame.to_string().into())).await.is_ok();
                if frame["type"] == "ended" { break; }
            }
            frame = input.next() => match frame {
                Some(Ok(Message::Pong(_))) => answered = true,
                Some(Ok(Message::Ping(data))) => alive = output.send(Message::Pong(data)).await.is_ok(),
                Some(Ok(Message::Text(text))) => {
                    let Ok(frame) = serde_json::from_str::<Value>(&text) else { break };
                    if host && frame["type"] == "end" {
                        revoke(&hub, &room).await;
                        ended = true;
                        break;
                    }
                    if frame["type"] != "signal" { break; }
                    let Some(data) = frame["data"].as_str().filter(|s| s.len() <= 60000 && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))) else { break };
                    let rooms = hub.rooms.lock().await;
                    let Some(active) = rooms.get(&room) else { break };
                    let target = if host {
                        frame["to"].as_str().and_then(|id| active.clients.get(id))
                    } else {
                        active.host.as_ref()
                    };
                    if let Some(target) = target
                        && target.try_send(json!({"type":"signal", "from":peer, "data":data})).is_err() { break; }
                    else if !host && active.owner_hash.is_some() && active.host.is_none() {
                        let _ = tx.try_send(json!({"type":"host_offline"}));
                    }
                }
                _ => break,
            }
        }
    }
    if !ended {
        leave(&hub, &room, &peer, host).await;
    }
}

async fn revoke(hub: &Hub, room: &str) {
    let mut rooms = hub.rooms.lock().await;
    if let Some(active) = rooms.remove(room) {
        for client in active.clients.values() {
            let _ = client.try_send(json!({"type":"ended"}));
        }
    }
}

async fn leave(hub: &Hub, room: &str, peer: &str, host: bool) {
    let offline_since = {
        let mut rooms = hub.rooms.lock().await;
        if host {
            let Some(active) = rooms.get_mut(room) else {
                return;
            };
            if active.owner_hash.is_none() {
                if let Some(active) = rooms.remove(room) {
                    for client in active.clients.values() {
                        let _ = client.try_send(json!({"type":"ended"}));
                    }
                }
                return;
            }
            active.host = None;
            let since = Instant::now();
            active.offline_since = Some(since);
            for client in active.clients.values() {
                let _ = client.try_send(json!({"type":"host_offline"}));
            }
            Some(since)
        } else if let Some(active) = rooms.get_mut(room) {
            active.clients.remove(peer);
            if let Some(host) = &active.host {
                let _ = host.try_send(json!({"type":"left", "peer":peer}));
            }
            None
        } else {
            None
        }
    };
    if let Some(since) = offline_since {
        let hub = hub.clone();
        let room = room.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(hub.idle).await;
            let mut rooms = hub.rooms.lock().await;
            let expired = rooms.get(&room).is_some_and(|active| {
                active.owner_hash.is_some()
                    && active.host.is_none()
                    && active.offline_since == Some(since)
            });
            if expired && let Some(active) = rooms.remove(&room) {
                for client in active.clients.values() {
                    let _ = client.try_send(json!({"type":"ended"}));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HostEvent, HostRoom, PeerSocket, share::Ticket};
    use async_tungstenite::{
        WebSocketStream,
        smol::{ConnectStream, connect_async},
        tungstenite::Message as Wire,
    };
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    type Socket = WebSocketStream<ConnectStream>;

    async fn read(socket: &mut Socket) -> Option<Value> {
        loop {
            match socket.next().await? {
                Ok(Wire::Text(text)) => return serde_json::from_str(&text).ok(),
                Ok(Wire::Ping(_)) | Ok(Wire::Pong(_)) => (),
                _ => return None,
            }
        }
    }

    async fn raw_join(url: &str, role: &str, owner_key: Option<&str>) -> Socket {
        raw_join_room(url, &URL_SAFE_NO_PAD.encode([1; 32]), role, owner_key).await
    }

    async fn raw_join_room(url: &str, room: &str, role: &str, owner_key: Option<&str>) -> Socket {
        let mut socket = connect_async(url).await.unwrap().0;
        let mut frame = json!({"type":"join","room":room,"role":role});
        if let Some(key) = owner_key {
            frame["owner_key"] = json!(key);
        }
        socket
            .send(Wire::Text(frame.to_string().into()))
            .await
            .unwrap();
        socket
    }

    #[tokio::test]
    async fn persistent_owner_rejoins_and_native_peers_exchange_bulk_data() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router()).await.unwrap();
        });
        let ticket = Ticket {
            v: 1,
            signal: format!("ws://{address}/usb-proxy/signal"),
            room: URL_SAFE_NO_PAD.encode([9; 32]),
            key: URL_SAFE_NO_PAD.encode([4; 32]),
            stun: vec![],
        };
        let link = format!(
            "https://camera.example/#?remotecontrol={}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ticket).unwrap())
        );
        let owner_key = "native-owner-secret";
        let host = HostRoom::join(
            &link,
            owner_key,
            crate::CandidatePolicy::Any,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let progress: crate::Progress = Arc::new(|_| {});
        let mut controller = PeerSocket::connect(
            &link,
            &progress,
            crate::CandidatePolicy::Any,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let HostEvent::Controller(_, mut owner_peer) = host.next_event().await.unwrap() else {
            panic!("expected controller connection");
        };
        let upload: Vec<u8> = (0..3 * 1024 * 1024).map(|index| index as u8).collect();
        controller.send(upload.clone()).await.unwrap();
        assert_eq!(owner_peer.receive().await.unwrap(), upload);
        let download: Vec<u8> = (0..3 * 1024 * 1024)
            .map(|index| (index * 7) as u8)
            .collect();
        owner_peer.send(download.clone()).await.unwrap();
        assert_eq!(controller.receive().await.unwrap(), download);

        drop(controller);
        drop(owner_peer);
        drop(host);
        let mut offline = false;
        for _ in 0..40 {
            let mut waiting = raw_join_room(&ticket.signal, &ticket.room, "client", None).await;
            let response = read(&mut waiting).await.unwrap();
            if response["type"] == "error" {
                assert_eq!(response["message"], "Camera share offline");
                offline = true;
                break;
            }
            drop(waiting);
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            offline,
            "owner room did not become offline after disconnect"
        );

        let host = HostRoom::join(
            &link,
            owner_key,
            crate::CandidatePolicy::Any,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let mut controller = PeerSocket::connect(
            &link,
            &progress,
            crate::CandidatePolicy::Any,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let HostEvent::Controller(_, mut owner_peer) = host.next_event().await.unwrap() else {
            panic!("expected controller after owner restart");
        };
        let payload = b"rejoined room".to_vec();
        controller.send(payload.clone()).await.unwrap();
        assert_eq!(owner_peer.receive().await.unwrap(), payload);
        host.end().await.unwrap();
        assert!(controller.receive().await.is_err());
        assert!(
            PeerSocket::connect(
                &link,
                &progress,
                crate::CandidatePolicy::Any,
                crate::Pacing::Browser
            )
            .await
            .is_err()
        );
        server.abort();
    }

    #[tokio::test]
    async fn persistent_rooms_reject_missing_or_wrong_owner_keys_and_expire_idle() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router_with_share_idle(Duration::from_millis(50)))
                .await
                .unwrap();
        });
        let url = format!("ws://{address}/usb-proxy/signal");
        let owner = raw_join(&url, "host", Some("owner-key")).await;
        let mut owner = owner;
        assert_eq!(read(&mut owner).await.unwrap()["type"], "ready");
        let mut existing = raw_join(&url, "client", None).await;
        assert_eq!(read(&mut existing).await.unwrap()["type"], "ready");
        drop(owner);
        assert_eq!(read(&mut existing).await.unwrap()["type"], "host_offline");
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut no_key = raw_join(&url, "host", None).await;
        assert_eq!(read(&mut no_key).await.unwrap()["type"], "error");
        let mut wrong_key = raw_join(&url, "host", Some("wrong-key")).await;
        assert_eq!(read(&mut wrong_key).await.unwrap()["type"], "error");
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(read(&mut existing).await.unwrap()["type"], "ended");
        let mut client = raw_join(&url, "client", None).await;
        assert_eq!(read(&mut client).await.unwrap()["type"], "error");
        server.abort();
    }

    #[tokio::test]
    async fn rendezvous_routes_opaque_setup_and_revokes_ordinary_rooms_with_the_owner() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/usb-proxy/signal", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router()).await.unwrap();
        });
        let mut host = raw_join(&url, "host", None).await;
        assert_eq!(read(&mut host).await.unwrap()["type"], "ready");
        let mut keyed_host = raw_join(&url, "host", Some("not-persistent")).await;
        assert_eq!(read(&mut keyed_host).await.unwrap()["type"], "error");
        let mut client = raw_join(&url, "client", None).await;
        let ready = read(&mut client).await.unwrap();
        let peer = ready["peer"].as_str().unwrap();
        assert_eq!(read(&mut host).await.unwrap()["type"], "joined");
        client
            .send(Wire::Text(
                json!({"type":"signal","data":"opaque_ciphertext"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let received = read(&mut host).await.unwrap();
        assert_eq!(received["from"], peer);
        assert_eq!(received["data"], "opaque_ciphertext");
        drop(host);
        assert_eq!(read(&mut client).await.unwrap()["type"], "ended");
        let mut expired = raw_join(&url, "client", None).await;
        assert_eq!(read(&mut expired).await.unwrap()["type"], "error");
        task.abort();
    }
}
