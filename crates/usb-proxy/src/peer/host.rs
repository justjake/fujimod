//! Native owner side of the encrypted WebRTC share protocol.
use super::CandidatePolicy;
use super::native::{IceConfig, PeerSocket};
use super::turn;
use crate::{Error, Result};
use async_channel::{Receiver, Sender};
use async_tungstenite::{
    WebSocketStream,
    smol::{ConnectStream, connect_async},
    tungstenite::Message,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    thread,
    time::Duration,
};

type Socket = WebSocketStream<ConnectStream>;

fn error(value: impl std::fmt::Display) -> Error {
    Error(value.to_string())
}

async fn write(socket: &mut Socket, value: Value) -> Result<()> {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(error)
}

async fn read(socket: &mut Socket) -> Result<Value> {
    loop {
        match socket
            .next()
            .await
            .ok_or_else(|| error("signaling disconnected"))?
            .map_err(error)?
        {
            Message::Text(text) if text.len() <= 65536 => {
                return serde_json::from_str(&text).map_err(error);
            }
            Message::Ping(_) | Message::Pong(_) => (),
            _ => return Err(error("invalid signaling message")),
        }
    }
}

enum Wake {
    Incoming(Result<Value>),
    Outgoing(std::result::Result<Value, async_channel::RecvError>),
    Ready(std::result::Result<(String, Result<PeerSocket>), async_channel::RecvError>),
    Stop,
}

/// A joined native owner room. Dropping it leaves a persistent room available
/// for a later owner restart; call [`HostRoom::end`] to revoke the room.
pub struct HostRoom {
    signal: Sender<Value>,
    stop: Sender<()>,
    events: Receiver<HostEvent>,
    finished: Receiver<()>,
}

/// Events from the signaling room. Each controller has a separate RTC task.
pub enum HostEvent {
    Controller(String, PeerSocket),
    Left(String),
}

impl HostRoom {
    /// Join a share URL as its persistent owner using the ticket-file key.
    pub async fn join(ticket_link: &str, owner_key: &str, policy: CandidatePolicy) -> Result<Self> {
        if owner_key.is_empty() || owner_key.len() > 1024 {
            return Err(error("invalid owner key"));
        }
        let ticket = crate::share::Ticket::from_url(ticket_link)?
            .ok_or_else(|| error("invalid share URL"))?;
        let mut signal_url = url::Url::parse(&ticket.signal).map_err(error)?;
        signal_url
            .query_pairs_mut()
            .append_pair("room", &ticket.room);
        let (mut socket, _) = connect_async(signal_url.as_str()).await.map_err(error)?;
        write(
            &mut socket,
            json!({
                "type":"join",
                "role":"host",
                "room":ticket.room,
                "owner_key":owner_key,
            }),
        )
        .await?;
        let welcome = read(&mut socket).await?;
        if welcome["type"] == "error" {
            return Err(error(
                welcome["message"]
                    .as_str()
                    .unwrap_or("camera share unavailable"),
            ));
        }
        if welcome["type"] != "ready" || welcome["peer"] != "host" {
            return Err(error("invalid signaling welcome"));
        }
        let ice = IceConfig {
            policy,
            servers: turn::ice_servers(&welcome["ice_servers"]),
        };

        let (signal, outgoing) = async_channel::unbounded();
        let (stop, stopping) = async_channel::bounded(1);
        let (events_tx, events) = async_channel::unbounded();
        let (controllers, ready) = async_channel::unbounded();
        let (finished_tx, finished) = async_channel::bounded(1);
        let signal_to_loop = signal.clone();
        thread::Builder::new()
            .name("fujimod-share-host-signal".into())
            .spawn(move || {
                futures_lite::future::block_on(signal_loop(
                    ticket,
                    ice,
                    socket,
                    signal_to_loop,
                    outgoing,
                    stopping,
                    controllers,
                    ready,
                    events_tx,
                ));
                let _ = finished_tx.try_send(());
            })
            .map_err(error)?;
        Ok(Self {
            signal,
            stop,
            events,
            finished,
        })
    }

    /// Receive the next controller connection or departure notification.
    pub async fn next_event(&self) -> Result<HostEvent> {
        self.events
            .recv()
            .await
            .map_err(|_| error("signaling disconnected"))
    }

    /// End and revoke this room on the signaling service.
    pub async fn end(&self) -> Result<()> {
        self.signal
            .send(json!({"type":"end"}))
            .await
            .map_err(|_| error("signaling disconnected"))?;
        futures_lite::future::or(
            async {
                self.finished
                    .recv()
                    .await
                    .map_err(|_| error("signaling disconnected"))
            },
            async {
                futures_timer::Delay::new(Duration::from_secs(3)).await;
                Err(error("signaling end timed out"))
            },
        )
        .await
    }
}

impl Drop for HostRoom {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
    }
}

async fn signal_loop(
    ticket: crate::share::Ticket,
    ice: IceConfig,
    mut socket: Socket,
    signaling: Sender<Value>,
    outgoing: Receiver<Value>,
    stopping: Receiver<()>,
    controllers: Sender<(String, Result<PeerSocket>)>,
    ready: Receiver<(String, Result<PeerSocket>)>,
    events: Sender<HostEvent>,
) {
    let mut active = HashMap::<String, Sender<Vec<u8>>>::new();
    let mut pending = HashSet::<String>::new();
    let mut left = HashSet::<String>::new();
    let mut alive = true;
    while alive {
        let wake = futures_lite::future::or(
            async { Wake::Incoming(read(&mut socket).await) },
            futures_lite::future::or(
                async { Wake::Outgoing(outgoing.recv().await) },
                futures_lite::future::or(async { Wake::Ready(ready.recv().await) }, async {
                    let _ = stopping.recv().await;
                    Wake::Stop
                }),
            ),
        )
        .await;
        match wake {
            Wake::Incoming(Ok(frame)) if frame["type"] == "joined" => (),
            Wake::Incoming(Ok(frame)) if frame["type"] == "left" => {
                if let Some(id) = frame["peer"].as_str() {
                    let id = id.to_owned();
                    if let Some(send) = active.remove(&id) {
                        send.close();
                    }
                    if pending.contains(&id) {
                        left.insert(id.clone());
                    }
                    let _ = events.send(HostEvent::Left(id)).await;
                }
            }
            Wake::Incoming(Ok(frame)) if frame["type"] == "signal" => {
                let Some(id) = frame["from"].as_str().map(str::to_owned) else {
                    continue;
                };
                let Some(data) = frame["data"].as_str().map(str::to_owned) else {
                    continue;
                };
                if active.contains_key(&id) || !pending.insert(id.clone()) {
                    continue;
                }
                let ticket = ticket.clone();
                let ice = ice.clone();
                let signal = signaling.clone();
                let controllers = controllers.clone();
                let peer_id = id.clone();
                let ready_id = id.clone();
                if thread::Builder::new()
                    .name("fujimod-webrtc-controller".into())
                    .spawn(move || {
                        futures_lite::future::block_on(async move {
                            let result =
                                PeerSocket::answer(ticket, peer_id.clone(), data, signal, ice)
                                    .await;
                            let _ = controllers.send((ready_id, result)).await;
                        });
                    })
                    .is_err()
                {
                    pending.remove(&id);
                }
            }
            Wake::Incoming(Ok(frame)) if frame["type"] == "ended" => alive = false,
            Wake::Incoming(Ok(frame)) if frame["type"] == "error" => alive = false,
            Wake::Incoming(Ok(_)) => (),
            Wake::Incoming(Err(_)) => alive = false,
            Wake::Outgoing(Ok(frame)) => {
                if write(&mut socket, frame.clone()).await.is_err() || frame["type"] == "end" {
                    alive = false;
                }
            }
            Wake::Outgoing(Err(_)) => alive = false,
            Wake::Ready(Ok((id, Ok(peer)))) => {
                pending.remove(&id);
                if left.remove(&id) {
                    peer.close_handle().close();
                } else {
                    active.insert(id.clone(), peer.close_handle());
                    if events.send(HostEvent::Controller(id, peer)).await.is_err() {
                        alive = false;
                    }
                }
            }
            Wake::Ready(Ok((id, Err(_)))) => {
                pending.remove(&id);
                left.remove(&id);
            }
            Wake::Ready(Err(_)) => alive = false,
            Wake::Stop => alive = false,
        }
    }
    signaling.close();
    // A signaling disconnect must not tear down already established RTC
    // channels. Their owners retain the corresponding PeerSocket values.
    drop(active);
    events.close();
}
