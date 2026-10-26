//! Ephemeral rendezvous only. SDP is encrypted by the share-link key at peers;
//! no USB payload or session secret passes through this service.
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
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, mpsc};

type Sender = mpsc::Sender<Value>;
struct Room {
    host: Sender,
    clients: HashMap<String, Sender>,
}
#[derive(Clone, Default)]
struct Hub {
    rooms: Arc<Mutex<HashMap<String, Room>>>,
    serial: Arc<AtomicU64>,
}
pub fn router() -> Router {
    Router::new()
        .route(
            "/usb-proxy/signal",
            get(|State(hub): State<Hub>, ws: WebSocketUpgrade| async {
                ws.max_message_size(65536)
                    .max_frame_size(65536)
                    .on_upgrade(move |socket| serve(hub, socket))
            }),
        )
        .with_state(Hub::default())
}
async fn first(socket: &mut WebSocket) -> Option<Value> {
    match socket.recv().await? {
        Ok(Message::Text(text)) => serde_json::from_str(&text).ok(),
        _ => None,
    }
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
        return;
    };
    let room = room.to_owned();
    let host = join["role"] == "host";
    if join["type"] != "join" || (!host && join["role"] != "client") {
        return;
    }
    let peer = if host {
        "host".into()
    } else {
        hub.serial.fetch_add(1, Ordering::Relaxed).to_string()
    };
    let (tx, mut rx) = mpsc::channel::<Value>(16);
    {
        let mut rooms = hub.rooms.lock().await;
        if host {
            if rooms.len() >= 1024 || rooms.contains_key(&room) {
                return;
            }
            rooms.insert(
                room.clone(),
                Room {
                    host: tx.clone(),
                    clients: HashMap::new(),
                },
            );
        } else {
            let Some(active) = rooms.get_mut(&room) else {
                return;
            };
            if active.clients.len() >= 16 {
                return;
            }
            if active
                .host
                .try_send(json!({"type":"joined", "peer":peer}))
                .is_err()
            {
                return;
            }
            active.clients.insert(peer.clone(), tx.clone());
        }
    }
    let (mut output, mut input) = socket.split();
    let ready = json!({"type":"ready", "peer":peer});
    let mut alive = output
        .send(Message::Text(ready.to_string().into()))
        .await
        .is_ok();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    let mut answered = true;
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
                    if frame["type"] != "signal" { break; }
                    let Some(data) = frame["data"].as_str().filter(|s| s.len() <= 60000 && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))) else { break };
                    let rooms = hub.rooms.lock().await;
                    let Some(active) = rooms.get(&room) else { break };
                    let target = if host { frame["to"].as_str().and_then(|id| active.clients.get(id)) } else { Some(&active.host) };
                    if let Some(target) = target
                        && target.try_send(json!({"type":"signal", "from":peer, "data":data})).is_err() { break; }
                }
                _ => break,
            }
        }
    }
    let mut rooms = hub.rooms.lock().await;
    if host {
        if let Some(active) = rooms.remove(&room) {
            for client in active.clients.values() {
                let _ = client.try_send(json!({"type":"ended"}));
            }
        }
    } else if let Some(active) = rooms.get_mut(&room) {
        active.clients.remove(&peer);
        let _ = active.host.try_send(json!({"type":"left", "peer":peer}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_tungstenite::{
        WebSocketStream,
        smol::{ConnectStream, connect_async},
        tungstenite::Message as Wire,
    };
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
    async fn join(url: &str, role: &str) -> Socket {
        let mut socket = connect_async(url).await.unwrap().0;
        socket
            .send(Wire::Text(
                json!({"type":"join","room":"A".repeat(43),"role":role})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        socket
    }
    #[tokio::test]
    async fn rendezvous_routes_opaque_setup_and_revokes_rooms_with_the_owner() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/usb-proxy/signal", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router()).await.unwrap();
        });
        let mut host = join(&url, "host").await;
        assert_eq!(read(&mut host).await.unwrap()["type"], "ready");
        let mut client = join(&url, "client").await;
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
        let mut expired = join(&url, "client").await;
        assert!(read(&mut expired).await.is_none());
        task.abort();
    }
}
