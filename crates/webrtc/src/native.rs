//! Native WebRTC peers on webrtc-rs. Each peer runs on its own I/O thread, so
//! it works under native Python, Tokio, smol or a plain block_on.
use super::ice;
use super::runtime::PeerRuntime;
use super::{CandidatePolicy, Pacing};
use crate::{Error, MAX_MESSAGE, Result, share::Ticket};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use async_channel::{Receiver, Sender};
use async_tungstenite::{
    WebSocketStream,
    smol::{ConnectStream, connect_async},
    tungstenite::Message,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::BytesMut;
use futures_util::StreamExt;
use rand::RngExt;
use serde_json::{Value, json};
use std::{
    future::Future,
    net::{SocketAddr, ToSocketAddrs},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit, RTCDataChannelState},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
        RTCIceGatheringState, RTCIceServer, RTCIceTransportPolicy, RTCPeerConnectionState,
        RTCSessionDescription,
    },
};
fn error(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}
type Socket = WebSocketStream<ConnectStream>;
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
            .ok_or_else(|| error("share unavailable"))?
            .map_err(error)?
        {
            Message::Text(text) if text.len() <= 65536 => {
                return serde_json::from_str(&text).map_err(error);
            }
            Message::Ping(_) | Message::Pong(_) => (),
            _ => return Err(error("signaling disconnected")),
        }
    }
}
fn cipher(ticket: &Ticket) -> Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(&URL_SAFE_NO_PAD.decode(&ticket.key).map_err(error)?).map_err(error)
}
fn seal(ticket: &Ticket, peer: &str, value: Value) -> Result<String> {
    let iv: [u8; 12] = rand::rng().random();
    let aad = format!("{}/{peer}", ticket.room);
    let encrypted = cipher(ticket)?
        .encrypt(
            &Nonce::from(iv),
            Payload {
                msg: value.to_string().as_bytes(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| error("signaling encryption failed"))?;
    let mut data = iv.to_vec();
    data.extend(encrypted);
    Ok(URL_SAFE_NO_PAD.encode(data))
}
fn unseal(ticket: &Ticket, peer: &str, value: &str) -> Result<Value> {
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(error)?;
    if bytes.len() < 28 {
        return Err(error("invalid encrypted signaling"));
    }
    let aad = format!("{}/{peer}", ticket.room);
    let decoded = cipher(ticket)?
        .decrypt(
            &Nonce::try_from(&bytes[..12]).map_err(error)?,
            Payload {
                msg: &bytes[12..],
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| error("signaling authentication failed"))?;
    serde_json::from_slice(&decoded).map_err(error)
}
pub struct PeerSocket {
    send: Sender<Vec<u8>>,
    receive: Receiver<Result<Vec<u8>>>,
    /// Closed when the I/O thread has flushed and exited.
    finished: Receiver<()>,
}
impl PeerSocket {
    pub(crate) async fn start<F, Fut>(name: &'static str, task: F) -> Result<Self>
    where
        F: FnOnce(PeerChannels) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + 'static,
    {
        let (tx, outbound) = async_channel::bounded(1);
        // The controller can queue replies while the caller is still sending;
        // its in-flight limit bounds this queue. A bounded queue can deadlock
        // the I/O thread when it tries to publish a result.
        let (incoming, rx) = async_channel::unbounded();
        let (ready, wait) = async_channel::bounded(1);
        let (finish, finished) = async_channel::bounded::<()>(1);
        thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                futures_lite::future::block_on(async {
                    if let Err(error) = task(PeerChannels {
                        outbound,
                        incoming: incoming.clone(),
                        ready: ready.clone(),
                    })
                    .await
                    {
                        let _ = ready.send(Err(error.to_string())).await;
                        let _ = incoming.send(Err(error)).await;
                    }
                });
                finish.close();
            })
            .map_err(error)?;
        wait.recv().await.map_err(error)?.map_err(error)?;
        Ok(Self {
            send: tx,
            receive: rx,
            finished,
        })
    }

    pub async fn connect(
        url: &str,
        progress: &crate::Progress,
        policy: CandidatePolicy,
        pacing: Pacing,
    ) -> Result<Self> {
        let ticket = Ticket::from_url(url)?.ok_or_else(|| error("invalid share URL"))?;
        progress("share link parsed");
        let progress = progress.clone();
        Self::start("fujimod-webrtc", move |channels| async move {
            run(ticket, channels, progress, policy, pacing).await
        })
        .await
    }

    pub(crate) async fn answer(
        ticket: Ticket,
        peer: String,
        sealed_offer: String,
        signaling: async_channel::Sender<Value>,
        ice: IceConfig,
    ) -> Result<Self> {
        Self::start("fujimod-webrtc-host", move |channels| async move {
            run_answer(ticket, peer, sealed_offer, signaling, ice, channels).await
        })
        .await
    }

    pub(crate) fn close_handle(&self) -> Sender<Vec<u8>> {
        self.send.clone()
    }
    /// Queue a final message, close, and wait up to a second for it to be sent.
    pub fn goodbye(&mut self, bytes: Vec<u8>) {
        if self.send.try_send(bytes).is_ok() {
            self.send.close();
            futures_lite::future::block_on(futures_lite::future::or(
                async {
                    let _ = self.finished.recv().await;
                },
                futures_timer::Delay::new(Duration::from_secs(1)),
            ));
        }
    }
    pub async fn send(&mut self, bytes: Vec<u8>) -> Result<()> {
        if self.send.send(bytes).await.is_ok() {
            return Ok(());
        }
        // The I/O thread has exited; its reason is queued for the receiver.
        while let Ok(item) = self.receive.try_recv() {
            item?;
        }
        Err(error("WebRTC connection closed"))
    }
    pub async fn receive(&mut self) -> Result<Vec<u8>> {
        self.receive
            .recv()
            .await
            .map_err(|_| error("peer disconnected; completion unknown"))?
    }
}

pub(crate) struct PeerChannels {
    outbound: Receiver<Vec<u8>>,
    incoming: Sender<Result<Vec<u8>>>,
    ready: Sender<std::result::Result<(), String>>,
}
impl Drop for PeerSocket {
    fn drop(&mut self) {
        self.send.close();
        self.receive.close();
    }
}

/// The ICE settings a room gives its peers: the candidate policy and the
/// TURN servers from the join reply.
#[derive(Clone, Debug)]
pub(crate) struct IceConfig {
    pub policy: CandidatePolicy,
    pub pacing: Pacing,
    pub servers: Vec<ice::IceServer>,
}

const LABEL: &str = "fujimod-usb";
/// Data channel messages carry one continuation byte and at most this many
/// payload bytes, the portable size for browsers and the camera's libpeer.
const FRAGMENT: usize = 16000;
/// Bytes handed to SCTP but not yet acknowledged. `send` waits above this.
const SEND_BUFFER: usize = 4 * 1024 * 1024;
/// How long candidate gathering waits for slow STUN and TURN servers.
const GATHER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the data channel may take to open.
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);

enum PeerEvent {
    Gathered,
    Failed,
    Channel(Arc<dyn DataChannel>),
}

struct Events(Sender<PeerEvent>);

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.0.try_send(PeerEvent::Gathered);
        }
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if matches!(
            state,
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
        ) {
            let _ = self.0.try_send(PeerEvent::Failed);
        }
    }
    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        let _ = self.0.try_send(PeerEvent::Channel(channel));
    }
}

/// STUN and TURN servers for the policy. Direct-only connections get no TURN
/// server, so no relay is ever allocated; relay-only ones get no STUN server.
fn ice_servers(ticket: &Ticket, ice: &IceConfig) -> Vec<RTCIceServer> {
    let turn_url = |url: &str| url.starts_with("turn:") || url.starts_with("turns:");
    let mut servers = Vec::new();
    if ice.policy != CandidatePolicy::RelayOnly && !ticket.stun.is_empty() {
        servers.push(RTCIceServer {
            urls: ticket
                .stun
                .iter()
                .map(|url| {
                    if url.starts_with("stun:") {
                        url.clone()
                    } else {
                        format!("stun:{url}")
                    }
                })
                .collect(),
            ..Default::default()
        });
    }
    for server in &ice.servers {
        let urls: Vec<String> = server
            .urls
            .iter()
            .filter(|url| match ice.policy {
                CandidatePolicy::DirectOnly => !turn_url(url),
                CandidatePolicy::RelayOnly => turn_url(url),
                CandidatePolicy::Any => true,
            })
            .cloned()
            .collect();
        if urls.is_empty() {
            continue;
        }
        let (username, credential) = server
            .credentials
            .as_ref()
            .map(|c| (c.username.clone(), c.password.clone()))
            .unwrap_or_default();
        servers.push(RTCIceServer {
            urls,
            username,
            credential,
        });
    }
    servers
}

/// The peer's SDP with its relayed candidates removed under
/// `CandidatePolicy::DirectOnly`.
fn remote_sdp(sdp: &str, policy: CandidatePolicy) -> String {
    if policy != CandidatePolicy::DirectOnly {
        return sdp.to_owned();
    }
    sdp.split_inclusive('\n')
        .filter(|line| !(line.starts_with("a=candidate:") && line.contains(" typ relay")))
        .collect()
}

/// Report the gathered candidates, and refuse a relay-only connection that has no relay.
fn check_candidates(sdp: &str, policy: CandidatePolicy, progress: &crate::Progress) -> Result<()> {
    let count = |kind: &str| {
        sdp.lines()
            .filter(|line| line.starts_with("a=candidate:") && line.contains(kind))
            .count()
    };
    let (host, reflexive, relay) = (count(" typ host"), count(" typ srflx"), count(" typ relay"));
    if host > 0 {
        progress(&format!("local candidates: {host}"));
    }
    if reflexive > 0 {
        progress(&format!("server-reflexive candidates: {reflexive}"));
    }
    if relay > 0 {
        progress("TURN relay candidate ready");
    } else if policy == CandidatePolicy::RelayOnly {
        return Err(error(
            "no TURN relay is available for a relay-only connection",
        ));
    }
    Ok(())
}

/// The local IPv4 address that routes to each UDP TURN server.
fn turn_routes(servers: &[RTCIceServer]) -> Vec<std::net::IpAddr> {
    let mut routes = Vec::new();
    for url in servers.iter().flat_map(|server| &server.urls) {
        let Some(rest) = url.strip_prefix("turn:") else {
            continue;
        };
        let (address, query) = rest.split_once('?').unwrap_or((rest, ""));
        if query.contains("transport=") && !query.contains("transport=udp") {
            continue;
        }
        let address = if address.contains(':') {
            address.to_owned()
        } else {
            format!("{address}:3478")
        };
        let Some(server) = address
            .to_socket_addrs()
            .ok()
            .and_then(|mut found| found.find(SocketAddr::is_ipv4))
        else {
            continue;
        };
        if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
            && socket.connect(server).is_ok()
            && let Ok(local) = socket.local_addr()
        {
            routes.push(local.ip());
        }
    }
    routes
}

/// A peer connection with one UDP socket on each local IPv4 interface.
async fn connection(
    ticket: &Ticket,
    ice: &IceConfig,
    events: Sender<PeerEvent>,
) -> Result<impl PeerConnection> {
    let servers = ice_servers(ticket, ice);
    let mut local: Vec<std::net::IpAddr> = if ice.policy == CandidatePolicy::RelayOnly {
        // Relayed traffic only leaves through the interface that reaches TURN.
        let routes = turn_routes(&servers);
        if routes.is_empty() {
            return Err(error(
                "no TURN relay is available for a relay-only connection",
            ));
        }
        routes
    } else {
        if_addrs::get_if_addrs()
            .map_err(error)?
            .into_iter()
            .map(|iface| iface.ip())
            .filter(std::net::IpAddr::is_ipv4)
            .collect()
    };
    local.sort();
    local.dedup();
    let configuration = RTCConfigurationBuilder::new()
        .with_ice_servers(servers)
        .with_ice_transport_policy(if ice.policy == CandidatePolicy::RelayOnly {
            RTCIceTransportPolicy::Relay
        } else {
            RTCIceTransportPolicy::All
        })
        .build();
    PeerConnectionBuilder::new()
        .with_configuration(configuration)
        .with_handler(Arc::new(Events(events)))
        .with_runtime(PeerRuntime::shared(ice.pacing))
        .with_udp_addrs(local.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect())
        .with_data_channel_send_buffer_limit(SEND_BUFFER)
        .build()
        .await
        .map_err(error)
}

/// Set the local description and wait for ICE gathering to finish, so the
/// SDP carries every candidate (signaling does not trickle).
async fn gathered<P: PeerConnection>(
    peer: &P,
    description: RTCSessionDescription,
    events: &Receiver<PeerEvent>,
    policy: CandidatePolicy,
    progress: &crate::Progress,
) -> Result<String> {
    peer.set_local_description(description)
        .await
        .map_err(error)?;
    let wait = async {
        loop {
            match events.recv().await.map_err(error)? {
                PeerEvent::Gathered => return Ok(()),
                PeerEvent::Failed => return Err(error("WebRTC connection failed")),
                PeerEvent::Channel(_) => (),
            }
        }
    };
    // An interface that cannot reach a STUN or TURN server must not hold up
    // the others: after the deadline, offer the candidates gathered so far.
    futures_lite::future::or(wait, async {
        futures_timer::Delay::new(GATHER_TIMEOUT).await;
        Ok(())
    })
    .await?;
    let sdp = peer
        .local_description()
        .await
        .ok_or_else(|| error("missing local WebRTC description"))?
        .sdp;
    check_candidates(&sdp, policy, progress)?;
    Ok(sdp)
}

async fn run(
    ticket: Ticket,
    channels: PeerChannels,
    progress: crate::Progress,
    policy: CandidatePolicy,
    pacing: Pacing,
) -> Result<()> {
    let mut signal_url = url::Url::parse(&ticket.signal).map_err(error)?;
    signal_url
        .query_pairs_mut()
        .append_pair("room", &ticket.room);
    let (mut signal, _) = connect_async(signal_url.as_str()).await.map_err(error)?;
    progress("signaling connected");
    write(
        &mut signal,
        json!({"type":"join","role":"client","room":ticket.room}),
    )
    .await?;
    let welcome = read(&mut signal).await?;
    if welcome["type"] == "error" {
        return Err(error(
            welcome["message"]
                .as_str()
                .unwrap_or("camera share unavailable"),
        ));
    }
    let peer = welcome["peer"]
        .as_str()
        .filter(|_| welcome["type"] == "ready")
        .ok_or_else(|| error("share unavailable: no owner in this room"))?
        .to_owned();
    progress("joined room; share owner present");
    // The join reply carries the TURN servers for this room, so gather after it.
    let ice = IceConfig {
        policy,
        pacing,
        servers: ice::ice_servers(&welcome["ice_servers"]),
    };
    let (events_tx, events) = async_channel::unbounded();
    let connection = connection(&ticket, &ice, events_tx).await?;
    let channel = connection
        .create_data_channel(
            LABEL,
            Some(RTCDataChannelInit {
                ordered: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(error)?;
    let offer = connection.create_offer(None).await.map_err(error)?;
    let offer = gathered(&connection, offer, &events, policy, &progress).await?;
    write(
        &mut signal,
        json!({"type":"signal","data":seal(&ticket,&peer,json!({"type":"offer","sdp":offer}))?}),
    )
    .await?;
    progress("WebRTC offer sent; waiting for owner's answer");
    let answer = read(&mut signal).await?;
    if answer["type"] != "signal" || answer["from"] != "host" {
        return Err(error("share ended"));
    }
    let answer = unseal(
        &ticket,
        &peer,
        answer["data"]
            .as_str()
            .ok_or_else(|| error("invalid signaling reply"))?,
    )?;
    if answer["type"] == "error" {
        return Err(error(format!(
            "share owner could not connect: {}",
            answer["message"].as_str().unwrap_or("no reason given")
        )));
    }
    if answer["type"] != "answer" {
        return Err(error("invalid WebRTC answer"));
    }
    progress("WebRTC answer received");
    let sdp = remote_sdp(
        answer["sdp"].as_str().ok_or_else(|| error("missing SDP"))?,
        policy,
    );
    connection
        .set_remote_description(RTCSessionDescription::answer(sdp).map_err(error)?)
        .await
        .map_err(error)?;
    progress("WebRTC answer accepted; connecting ICE");
    pump(
        connection,
        Some(channel),
        events,
        channels,
        progress,
        Some(&mut signal),
    )
    .await
}

async fn run_answer(
    ticket: Ticket,
    peer: String,
    sealed_offer: String,
    signaling: async_channel::Sender<Value>,
    ice: IceConfig,
    channels: PeerChannels,
) -> Result<()> {
    let progress: crate::Progress = Arc::new(|_| {});
    let (events_tx, events) = async_channel::unbounded();
    let prepared = async {
        let offer = unseal(&ticket, &peer, &sealed_offer)?;
        if offer["type"] != "offer" {
            return Err(error("invalid WebRTC offer"));
        }
        let sdp = remote_sdp(
            offer["sdp"].as_str().ok_or_else(|| error("missing SDP"))?,
            ice.policy,
        );
        let connection = connection(&ticket, &ice, events_tx).await?;
        connection
            .set_remote_description(RTCSessionDescription::offer(sdp).map_err(error)?)
            .await
            .map_err(error)?;
        let answer = connection.create_answer(None).await.map_err(error)?;
        let answer = gathered(&connection, answer, &events, ice.policy, &progress).await?;
        Ok::<_, Error>((connection, answer))
    }
    .await;
    let (connection, sdp) = match prepared {
        Ok(prepared) => prepared,
        Err(failure) => {
            let frame = json!({
                "type": "signal",
                "to": peer,
                "data": seal(&ticket, &peer, json!({
                    "type": "error",
                    "message": failure.to_string(),
                }))?,
            });
            let _ = signaling.send(frame).await;
            return Err(failure);
        }
    };
    signaling
        .send(json!({
            "type": "signal",
            "to": peer,
            "data": seal(&ticket, &peer, json!({"type":"answer","sdp":sdp}))?,
        }))
        .await
        .map_err(error)?;
    pump(connection, None, events, channels, progress, None).await
}

/// Wait for the data channel (the answerer receives it from the peer) to open.
async fn open(
    channel: Option<Arc<dyn DataChannel>>,
    events: &Receiver<PeerEvent>,
) -> Result<Arc<dyn DataChannel>> {
    let channel = match channel {
        Some(channel) => channel,
        None => loop {
            match events.recv().await.map_err(error)? {
                PeerEvent::Channel(channel) if channel.label().await.map_err(error)? == LABEL => {
                    break channel;
                }
                PeerEvent::Failed => return Err(error("direct WebRTC connection unavailable")),
                _ => (),
            }
        },
    };
    if channel.ready_state().await.map_err(error)? == RTCDataChannelState::Open {
        return Ok(channel);
    }
    let opened = async {
        loop {
            match channel.poll().await {
                Some(DataChannelEvent::OnOpen) => return Ok(()),
                Some(DataChannelEvent::OnClose) | None => {
                    return Err(error("direct WebRTC connection unavailable"));
                }
                Some(_) => (),
            }
        }
    };
    let failed = async {
        loop {
            if let Ok(PeerEvent::Failed) | Err(_) = events.recv().await {
                return Err(error("direct WebRTC connection unavailable"));
            }
        }
    };
    futures_lite::future::or(opened, failed).await?;
    Ok(channel)
}

/// Move messages between the controller channels and the data channel until
/// either side closes. A closed outbound queue flushes, then ends cleanly.
async fn pump<P: PeerConnection>(
    connection: P,
    channel: Option<Arc<dyn DataChannel>>,
    events: Receiver<PeerEvent>,
    channels: PeerChannels,
    progress: crate::Progress,
    signal: Option<&mut Socket>,
) -> Result<()> {
    let PeerChannels {
        outbound,
        incoming,
        ready,
    } = channels;
    let channel = futures_lite::future::or(open(channel, &events), async {
        futures_timer::Delay::new(OPEN_TIMEOUT).await;
        Err(error("direct WebRTC connection unavailable"))
    })
    .await?;
    progress("WebRTC data channel open");
    if ready.send(Ok(())).await.is_err() {
        let _ = connection.close().await;
        return Ok(());
    }
    let receiving = async {
        let mut assembled = Vec::new();
        loop {
            match channel.poll().await {
                Some(DataChannelEvent::OnMessage(message)) => {
                    let data = message.data;
                    if message.is_string
                        || data.is_empty()
                        || data[0] > 1
                        || assembled.len() + data.len() - 1 > MAX_MESSAGE
                    {
                        return Err(error("invalid peer frame"));
                    }
                    assembled.extend_from_slice(&data[1..]);
                    if data[0] == 1 {
                        incoming
                            .send(Ok(std::mem::take(&mut assembled)))
                            .await
                            .map_err(error)?;
                    }
                }
                Some(DataChannelEvent::OnClose) | None => {
                    return Err(error("peer disconnected; completion unknown"));
                }
                Some(_) => (),
            }
        }
    };
    let sending = async {
        while let Ok(bytes) = outbound.recv().await {
            if bytes.len() > MAX_MESSAGE {
                return Err(error("message exceeds limit"));
            }
            let count = bytes.len().div_ceil(FRAGMENT).max(1);
            for index in 0..count {
                let chunk = &bytes[(index * FRAGMENT).min(bytes.len())
                    ..((index + 1) * FRAGMENT).min(bytes.len())];
                let mut packet = BytesMut::with_capacity(chunk.len() + 1);
                packet.extend_from_slice(&[u8::from(index + 1 == count)]);
                packet.extend_from_slice(chunk);
                channel.send(packet).await.map_err(error)?;
            }
        }
        // The controller closed: give queued messages up to a second to be acknowledged.
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline && channel.outstanding_bytes().await.unwrap_or(0) > 0 {
            futures_timer::Delay::new(Duration::from_millis(10)).await;
        }
        Ok(())
    };
    let failed = async {
        loop {
            if let Ok(PeerEvent::Failed) | Err(_) = events.recv().await {
                return Err(error("WebRTC connection closed"));
            }
        }
    };
    let ended = async {
        let Some(socket) = signal else {
            return std::future::pending().await;
        };
        loop {
            match read(socket).await {
                Ok(frame) if frame["type"] == "ended" => {
                    return Err(error("camera sharing ended"));
                }
                Ok(_) => (),
                Err(_) => return std::future::pending().await,
            }
        }
    };
    let result = futures_lite::future::or(
        sending,
        futures_lite::future::or(receiving, futures_lite::future::or(failed, ended)),
    )
    .await;
    let _ = connection.close().await;
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signaling_is_authenticated_and_bound_to_the_room_and_peer() {
        let ticket = Ticket {
            v: 1,
            signal: "wss://example.test/signal".into(),
            room: URL_SAFE_NO_PAD.encode([1; 32]),
            key: URL_SAFE_NO_PAD.encode([2; 32]),
            stun: vec![],
        };
        let text = seal(&ticket, "7", json!({"type":"offer","sdp":"private SDP"})).unwrap();
        assert!(!text.contains("private"));
        assert_eq!(unseal(&ticket, "7", &text).unwrap()["sdp"], "private SDP");
        assert!(unseal(&ticket, "8", &text).is_err());
        let mut other = ticket.clone();
        other.room = URL_SAFE_NO_PAD.encode([3; 32]);
        assert!(unseal(&other, "7", &text).is_err());
        other = ticket.clone();
        other.key = URL_SAFE_NO_PAD.encode([4; 32]);
        assert!(unseal(&other, "7", &text).is_err());
    }
}

#[cfg(test)]
mod direct_tests {
    use super::*;

    #[test]
    fn direct_only_drops_relayed_remote_candidates() {
        let sdp = "v=0\r\na=candidate:1 1 udp 1 10.0.0.2 5000 typ host\r\na=candidate:2 1 udp 1 203.0.113.9 6000 typ relay raddr 0.0.0.0 rport 0\r\na=end\r\n";
        let direct = remote_sdp(sdp, CandidatePolicy::DirectOnly);
        assert!(direct.contains("typ host"));
        assert!(!direct.contains("typ relay"));
        assert!(direct.ends_with("a=end\r\n"));
        assert_eq!(remote_sdp(sdp, CandidatePolicy::Any), sdp);
    }
}

#[cfg(all(test, feature = "signaling"))]
mod relay_tests {
    use super::*;
    use crate::{CandidatePolicy, HostEvent, HostRoom, turn::test_server};
    use std::sync::Mutex;

    async fn signaling_with(ice: Value) -> (String, Ticket) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                crate::signaling::router_with_ice_servers(Duration::from_secs(60), ice),
            )
            .await
            .unwrap();
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
        (link, ticket)
    }

    #[tokio::test]
    async fn direct_only_share_never_contacts_turn() {
        let credentials = test_server::credentials();
        let turn = test_server::spawn(credentials.clone());
        let ice = serde_json::json!([{
            "urls": [format!("turn:{}?transport=udp", turn.address)],
            "username": credentials.username,
            "credential": credentials.password,
        }]);
        let (link, _) = signaling_with(ice).await;
        let host = HostRoom::join(
            &link,
            "native-owner-secret",
            CandidatePolicy::DirectOnly,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let progress: crate::Progress = std::sync::Arc::new(|_| {});
        let mut controller = PeerSocket::connect(
            &link,
            &progress,
            CandidatePolicy::DirectOnly,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let HostEvent::Controller(_, mut owner_peer) = host.next_event().await.unwrap() else {
            panic!("expected controller connection");
        };
        controller.send(vec![7; 4096]).await.unwrap();
        assert_eq!(owner_peer.receive().await.unwrap(), vec![7; 4096]);
        assert_eq!(
            turn.stats
                .allocations
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        host.end().await.unwrap();
    }

    #[tokio::test]
    async fn relay_only_share_flows_through_turn_relays() {
        let credentials = test_server::credentials();
        let turn = test_server::spawn(credentials.clone());
        let ice = serde_json::json!([{
            "urls": [format!("turn:{}?transport=udp", turn.address)],
            "username": credentials.username,
            "credential": credentials.password,
        }]);
        let (link, _) = signaling_with(ice).await;
        let host = HostRoom::join(
            &link,
            "native-owner-secret",
            CandidatePolicy::RelayOnly,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let steps = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let log = steps.clone();
        let progress: crate::Progress =
            std::sync::Arc::new(move |step: &str| log.lock().unwrap().push(step.to_owned()));
        let mut controller = PeerSocket::connect(
            &link,
            &progress,
            CandidatePolicy::RelayOnly,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let HostEvent::Controller(_, mut owner_peer) = host.next_event().await.unwrap() else {
            panic!("expected controller connection");
        };
        let upload: Vec<u8> = (0..1024 * 1024).map(|index| index as u8).collect();
        controller.send(upload.clone()).await.unwrap();
        assert_eq!(owner_peer.receive().await.unwrap(), upload);
        owner_peer.send(upload.clone()).await.unwrap();
        assert_eq!(controller.receive().await.unwrap(), upload);

        let steps = steps.lock().unwrap().clone();
        assert!(
            steps
                .iter()
                .any(|step| step == "TURN relay candidate ready")
        );
        assert!(
            !steps
                .iter()
                .any(|step| step.starts_with("local candidates"))
        );
        assert!(turn.stats.sends.load(std::sync::atomic::Ordering::SeqCst) > 0);
        assert!(
            turn.stats
                .indications
                .load(std::sync::atomic::Ordering::SeqCst)
                > 0
        );
        host.end().await.unwrap();
    }

    #[tokio::test]
    async fn relay_only_connection_is_refused_without_a_turn_server() {
        let (link, _) = signaling_with(serde_json::Value::Null).await;
        let host = HostRoom::join(
            &link,
            "native-owner-secret",
            CandidatePolicy::Any,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let progress: crate::Progress = std::sync::Arc::new(|_| {});
        let failure = PeerSocket::connect(
            &link,
            &progress,
            CandidatePolicy::RelayOnly,
            crate::Pacing::Browser,
        )
        .await
        .err()
        .expect("relay-only connection needs TURN");
        assert!(failure.to_string().contains("relay"), "{failure}");
        host.end().await.unwrap();
    }

    /// A relay-only WebRTC data channel between two native peers, both relayed by
    /// the live Cloudflare TURN server. Signaling is local (the share TLS path is
    /// tested separately); TURN credentials come from the environment.
    #[tokio::test]
    #[ignore = "uses a live TURN server; set FUJIMOD_TURN_LIVE_URL, _USER and _CREDENTIAL"]
    async fn relay_only_data_channel_through_live_cloudflare_turn() {
        let url = std::env::var("FUJIMOD_TURN_LIVE_URL").expect("FUJIMOD_TURN_LIVE_URL");
        let ice = serde_json::json!([{
            "urls": [url],
            "username": std::env::var("FUJIMOD_TURN_LIVE_USER").expect("FUJIMOD_TURN_LIVE_USER"),
            "credential": std::env::var("FUJIMOD_TURN_LIVE_CREDENTIAL").expect("FUJIMOD_TURN_LIVE_CREDENTIAL"),
        }]);
        let (link, _) = signaling_with(ice).await;
        let host = HostRoom::join(
            &link,
            "native-owner-secret",
            CandidatePolicy::RelayOnly,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let steps = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let log = steps.clone();
        let progress: crate::Progress = std::sync::Arc::new(move |step: &str| {
            println!("{step}");
            log.lock().unwrap().push(step.to_owned());
        });
        let mut controller = PeerSocket::connect(
            &link,
            &progress,
            CandidatePolicy::RelayOnly,
            crate::Pacing::Browser,
        )
        .await
        .unwrap();
        let HostEvent::Controller(_, mut owner_peer) = host.next_event().await.unwrap() else {
            panic!("expected controller connection");
        };
        let payload: Vec<u8> = (0..256 * 1024).map(|index| (index * 13) as u8).collect();
        controller.send(payload.clone()).await.unwrap();
        assert_eq!(owner_peer.receive().await.unwrap(), payload);
        owner_peer.send(payload.clone()).await.unwrap();
        assert_eq!(controller.receive().await.unwrap(), payload);
        println!("RELAY-ONLY DATA CHANNEL OK through Cloudflare TURN");
        host.end().await.unwrap();
    }
}
