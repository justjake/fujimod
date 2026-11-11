//! str0m is driven by one owned I/O task. It works under native Python, Tokio,
//! smol or a plain block_on; no caller runtime or callbacks are required.
use super::CandidatePolicy;
use super::datagram::{Datagram, DatagramIo};
use super::turn::{self, Action, TurnClient};
use crate::{Error, Result, protocol::MAX_MESSAGE, share::Ticket};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use async_channel::{Receiver, Sender};
use async_io::Async;
use async_tungstenite::{
    WebSocketStream,
    smol::{ConnectStream, connect_async},
    tungstenite::Message,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use rand::RngExt;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    future::Future,
    net::{SocketAddr, ToSocketAddrs},
    thread,
    time::{Duration, Instant},
};
use str0m::{
    Candidate, Event, Input, Output, Rtc,
    change::{SdpAnswer, SdpOffer},
    channel::ChannelId,
    net::{Protocol, Receive},
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
    ) -> Result<Self> {
        let ticket = Ticket::from_url(url)?.ok_or_else(|| error("invalid share URL"))?;
        progress("share link parsed");
        let progress = progress.clone();
        Self::start("fujimod-webrtc", move |channels| async move {
            run(ticket, channels, progress, policy).await
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

fn drain_setup(rtc: &mut Rtc) -> Result<()> {
    loop {
        if let Output::Timeout(_) = rtc.poll_output().map_err(error)? {
            return Ok(());
        }
    }
}

/// The ICE settings a room gives its peers: the candidate policy and the
/// TURN servers from the join reply.
#[derive(Clone, Debug)]
pub(crate) struct IceConfig {
    pub policy: CandidatePolicy,
    pub servers: Vec<turn::IceServer>,
}

/// How long a TURN allocation may take before the connection goes without a relay.
const ALLOCATION_TIMEOUT: Duration = Duration::from_secs(5);

/// A UDP allocation whose relayed address is a candidate of this connection.
/// Its socket is dedicated, so every datagram it sends is a relay datagram.
struct TurnRelay {
    client: TurnClient,
    /// Local address of the socket that talks to the TURN server.
    socket: SocketAddr,
    server: SocketAddr,
    relayed: SocketAddr,
}

/// The UDP sockets and optional TURN relay that carry one peer connection.
struct Network {
    udp: DatagramIo,
    relay: Option<TurnRelay>,
    policy: CandidatePolicy,
}

impl Network {
    /// Carry out TURN actions: transmit to the server, and hand relayed data to ICE.
    async fn apply(&mut self, rtc: &mut Rtc, actions: Vec<Action>) -> Result<()> {
        let Some(relay) = self.relay.as_ref() else {
            return Ok(());
        };
        let (socket, server, relayed) = (relay.socket, relay.server, relay.relayed);
        for action in actions {
            match action {
                Action::Transmit(bytes) => {
                    // A failed datagram is lost like any other; ICE retransmits.
                    // Relay-only connections have no other path, so they fail.
                    let sent = self.udp.send(socket, server, &bytes).await;
                    if sent.is_err() && self.policy == CandidatePolicy::RelayOnly {
                        return Err(error("TURN server unreachable"));
                    }
                }
                Action::Received { peer, data } => {
                    if let Ok(receive) = Receive::new(Protocol::Udp, peer, relayed, &data) {
                        rtc.handle_input(Input::Receive(Instant::now(), receive))
                            .map_err(error)?;
                    }
                }
                Action::Failed(reason) => {
                    if self.policy == CandidatePolicy::RelayOnly {
                        return Err(error(format!("TURN relay lost: {reason}")));
                    }
                }
                Action::Allocated(_) => {}
            }
        }
        Ok(())
    }

    /// Route an outgoing ICE packet through the relay when it leaves the relay
    /// address or its socket. Returns the TURN actions, or `None` for a direct packet.
    fn relay_send(
        &mut self,
        source: SocketAddr,
        destination: SocketAddr,
        contents: &[u8],
    ) -> Option<Vec<Action>> {
        let relay = self
            .relay
            .as_mut()
            .filter(|relay| source == relay.relayed || source == relay.socket)?;
        Some(relay.client.send(Instant::now(), destination, contents))
    }

    /// Permit the candidate addresses in the peer's SDP to reach the relay.
    async fn permit_candidates(&mut self, rtc: &mut Rtc, sdp: &str) -> Result<()> {
        let Some(relay) = self.relay.as_mut() else {
            return Ok(());
        };
        let now = Instant::now();
        let actions: Vec<Action> = turn::sdp_candidate_addresses(sdp)
            .into_iter()
            .flat_map(|peer| relay.client.permit(now, peer))
            .collect();
        self.apply(rtc, actions).await
    }
}

/// Allocate a UDP relay on the TURN server, reached through the interface route.
async fn allocate_relay(target: &turn::RelayTarget) -> Result<(TurnRelay, std::net::UdpSocket)> {
    let server = (target.host.as_str(), target.port)
        .to_socket_addrs()
        .map_err(error)?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| error("TURN server has no IPv4 address"))?;
    let route = std::net::UdpSocket::bind("0.0.0.0:0").map_err(error)?;
    route.connect(server).map_err(error)?;
    let ip = route.local_addr().map_err(error)?.ip();
    let socket = Async::new(std::net::UdpSocket::bind(SocketAddr::new(ip, 0)).map_err(error)?)
        .map_err(error)?;
    let local = socket.get_ref().local_addr().map_err(error)?;
    let mut client = TurnClient::new(target.credentials.clone());
    let deadline = Instant::now() + ALLOCATION_TIMEOUT;
    let mut actions = client.start(Instant::now());
    let mut buffer = vec![0u8; 2048];
    loop {
        let mut relayed = None;
        for action in actions.drain(..) {
            match action {
                Action::Transmit(bytes) => {
                    socket.send_to(&bytes, server).await.map_err(error)?;
                }
                Action::Allocated(address) => relayed = Some(address),
                Action::Failed(reason) => return Err(error(reason)),
                Action::Received { .. } => {}
            }
        }
        if let Some(relayed) = relayed {
            let socket = socket.into_inner().map_err(error)?;
            let relay = TurnRelay {
                client,
                socket: local,
                server,
                relayed,
            };
            return Ok((relay, socket));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(error("the TURN server did not allocate a relay in time"));
        }
        let wait = client
            .next_timeout()
            .map_or(deadline, |at| at.min(deadline))
            .saturating_duration_since(now);
        let event =
            futures_lite::future::or(async { Some(socket.recv_from(&mut buffer).await) }, async {
                futures_timer::Delay::new(wait).await;
                None
            })
            .await;
        actions = match event {
            Some(Ok((size, from))) if from == server => {
                client.handle(Instant::now(), &buffer[..size])
            }
            Some(Ok(_)) => Vec::new(),
            Some(Err(failure)) => return Err(error(failure)),
            None => client.tick(Instant::now()),
        };
    }
}

/// Bind local sockets, allocate the TURN relay and gather candidates. Under
/// `CandidatePolicy::RelayOnly`, no host or reflexive candidate is advertised.
async fn gather_candidates(
    rtc: &mut Rtc,
    ticket: &Ticket,
    ice: &IceConfig,
    progress: &crate::Progress,
) -> Result<Network> {
    let mut sockets = Vec::new();
    if ice.policy == CandidatePolicy::Any {
        let mut local = Vec::new();
        for iface in if_addrs::get_if_addrs().map_err(error)? {
            if iface.ip().is_ipv4() {
                local.push(iface.ip());
            }
        }
        local.sort();
        local.dedup();
        for ip in &local {
            let socket = std::net::UdpSocket::bind(SocketAddr::new(*ip, 0)).map_err(error)?;
            let addr = socket.local_addr().map_err(error)?;
            rtc.add_local_candidate(Candidate::host(addr, "udp").map_err(error)?);
            drain_setup(rtc)?;
            sockets.push(socket);
        }
        progress(&format!("local candidates: {}", local.len()));
        add_reflexive_candidates(rtc, ticket, &sockets, progress)?;
    }
    let relay = match turn::relay_target(&ice.servers) {
        None => None,
        Some(target) => match allocate_relay(&target).await {
            Ok(allocation) => Some(allocation),
            Err(failure) if ice.policy == CandidatePolicy::RelayOnly => return Err(failure),
            Err(failure) => {
                progress(&format!("TURN relay unavailable: {failure}"));
                None
            }
        },
    };
    if ice.policy == CandidatePolicy::RelayOnly && relay.is_none() {
        return Err(error(
            "no TURN relay is available for a relay-only connection",
        ));
    }
    let relay = relay.map(|(relay, socket)| {
        sockets.push(socket);
        relay
    });
    if let Some(relay) = &relay {
        rtc.add_local_candidate(
            Candidate::relayed(relay.relayed, relay.socket, "udp").map_err(error)?,
        );
        drain_setup(rtc)?;
        progress("TURN relay candidate ready");
    }
    Ok(Network {
        udp: DatagramIo::new(sockets).map_err(error)?,
        relay,
        policy: ice.policy,
    })
}

fn add_reflexive_candidates(
    rtc: &mut Rtc,
    ticket: &Ticket,
    sockets: &[std::net::UdpSocket],
    progress: &crate::Progress,
) -> Result<()> {
    // An optional STUN lookup uses the same socket as ICE, without a TURN path.
    for server in &ticket.stun {
        let name = server.trim_start_matches("stun:");
        let name = if name.contains(':') {
            name.to_owned()
        } else {
            format!("{name}:3478")
        };
        if let Ok(addresses) = name.to_socket_addrs()
            && let Some(server) = addresses.into_iter().find(SocketAddr::is_ipv4)
        {
            let mut client = stunclient::StunClient::new(server);
            client.set_timeout(Duration::from_secs(2));
            let route = std::net::UdpSocket::bind("0.0.0.0:0").map_err(error)?;
            route.connect(server).map_err(error)?;
            let ip = route.local_addr().map_err(error)?.ip();
            let Some(socket) = sockets
                .iter()
                .find(|socket| socket.local_addr().is_ok_and(|addr| addr.ip() == ip))
            else {
                continue;
            };
            let found = client.query_external_address(socket);
            progress(&match &found {
                Ok(external) => format!("STUN {server}: external address {external}"),
                Err(e) => format!("STUN {server}: no reply ({e})"),
            });
            if let Ok(external) = found {
                let base = socket.local_addr().map_err(error)?;
                rtc.add_local_candidate(
                    Candidate::server_reflexive(external, base, "udp").map_err(error)?,
                );
                drain_setup(rtc)?;
            }
        }
    }
    Ok(())
}

async fn run(
    ticket: Ticket,
    channels: PeerChannels,
    progress: crate::Progress,
    policy: CandidatePolicy,
) -> Result<()> {
    let PeerChannels {
        outbound,
        incoming,
        ready,
    } = channels;
    // str0m counts unacknowledged bytes against this bound. Its 128 KiB
    // default limits a bulk sender to 128 KiB per RTT (6.6 MB/s at 20 ms).
    // Reserve a bounded bulk window rather than the interactive default.
    let mut rtc = Rtc::builder()
        .set_sctp_max_buffered_amount(512 * 1024)
        .build(Instant::now());
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
        servers: turn::ice_servers(&welcome["ice_servers"]),
    };
    let mut network = gather_candidates(&mut rtc, &ticket, &ice, &progress).await?;
    let mut change = rtc.sdp_api();
    change.add_channel("fujimod-usb".into());
    let (offer, pending) = change
        .apply()
        .ok_or_else(|| error("missing WebRTC offer"))?;
    drain_setup(&mut rtc)?;
    write(&mut signal, json!({"type":"signal","data":seal(&ticket,&peer,json!({"type":"offer","sdp":offer.to_sdp_string()}))?})).await?;
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
    let sdp = answer["sdp"].as_str().ok_or_else(|| error("missing SDP"))?;
    rtc.sdp_api()
        .accept_answer(pending, SdpAnswer::from_sdp_string(sdp).map_err(error)?)
        .map_err(error)?;
    network.permit_candidates(&mut rtc, sdp).await?;
    progress("WebRTC answer accepted; connecting ICE");
    pump(
        rtc,
        network,
        None,
        outbound,
        incoming,
        ready,
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
    let PeerChannels {
        outbound,
        incoming,
        ready,
    } = channels;
    let prepared = async {
        let offer = unseal(&ticket, &peer, &sealed_offer)?;
        if offer["type"] != "offer" {
            return Err(error("invalid WebRTC offer"));
        }
        let sdp = offer["sdp"].as_str().ok_or_else(|| error("missing SDP"))?;
        let mut rtc = Rtc::builder()
            .set_sctp_max_buffered_amount(512 * 1024)
            .build(Instant::now());
        let progress: crate::Progress = std::sync::Arc::new(|_| {});
        let mut network = gather_candidates(&mut rtc, &ticket, &ice, &progress).await?;
        let offer = SdpOffer::from_sdp_string(sdp).map_err(error)?;
        let answer = rtc.sdp_api().accept_offer(offer).map_err(error)?;
        drain_setup(&mut rtc)?;
        network.permit_candidates(&mut rtc, sdp).await?;
        Ok::<_, Error>((rtc, network, answer.to_sdp_string()))
    }
    .await;
    let (rtc, network, sdp) = match prepared {
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
    pump(
        rtc,
        network,
        None,
        outbound,
        incoming,
        ready,
        std::sync::Arc::new(|_| {}),
        None,
    )
    .await
}

async fn pump(
    mut rtc: Rtc,
    mut network: Network,
    mut channel: Option<ChannelId>,
    outbound: Receiver<Vec<u8>>,
    incoming: Sender<Result<Vec<u8>>>,
    ready: Sender<std::result::Result<(), String>>,
    progress: crate::Progress,
    mut signal: Option<&mut Socket>,
) -> Result<()> {
    let mut assembled = Vec::new();
    let mut packets: VecDeque<Vec<u8>> = VecDeque::new();
    let mut writable = true;
    let mut signal_alive = signal.is_some();
    let started = Instant::now();
    // Set when the controller closes: flush queued messages, then exit.
    let mut closing: Option<Instant> = None;
    loop {
        if let Some(relay) = network.relay.as_mut() {
            let actions = relay.client.tick(Instant::now());
            network.apply(&mut rtc, actions).await?;
        }
        let output_timeout = loop {
            match rtc.poll_output().map_err(error)? {
                Output::Timeout(at) => break at,
                Output::Transmit(packet) => {
                    if let Some(actions) =
                        network.relay_send(packet.source, packet.destination, &packet.contents)
                    {
                        network.apply(&mut rtc, actions).await?;
                        continue;
                    }
                    // A candidate pair this host cannot route to (an IPv6 or
                    // foreign LAN address) is a failed ICE check, not a failed
                    // connection: ICE carries on with the other pairs.
                    if let Err(failure) = network
                        .udp
                        .send(packet.source, packet.destination, &packet.contents)
                        .await
                    {
                        let unreachable =
                            matches!(
                                failure.kind(),
                                std::io::ErrorKind::HostUnreachable
                                    | std::io::ErrorKind::NetworkUnreachable
                                    | std::io::ErrorKind::AddrNotAvailable
                            ) || matches!(failure.raw_os_error(), Some(49 | 50 | 51 | 65));
                        if !unreachable {
                            return Err(error(failure));
                        }
                    }
                }
                Output::Event(event) => match event {
                    Event::IceConnectionStateChange(state) => {
                        progress(&format!("ICE {state:?}"));
                    }
                    Event::Connected => progress("DTLS connected"),
                    Event::ChannelOpen(id, label) if label == "fujimod-usb" => {
                        channel = Some(id);
                        progress("WebRTC data channel open");
                        if ready.send(Ok(())).await.is_err() {
                            return Ok(());
                        }
                    }
                    Event::ChannelData(data) => {
                        if Some(data.id) != channel
                            || !data.binary
                            || data.data.is_empty()
                            || data.data[0] > 1
                            || assembled.len() + data.data.len() - 1 > MAX_MESSAGE
                        {
                            return Err(error("invalid peer frame"));
                        }
                        assembled.extend_from_slice(&data.data[1..]);
                        if data.data[0] == 1 {
                            incoming
                                .send(Ok(std::mem::take(&mut assembled)))
                                .await
                                .map_err(error)?;
                        }
                    }
                    Event::ChannelClose(_) => {
                        return Err(error("peer disconnected; completion unknown"));
                    }
                    _ => (),
                },
            }
        };
        // The TURN client's retransmissions and refreshes also wake the loop.
        let timeout = match network.relay.as_ref().and_then(|r| r.client.next_timeout()) {
            Some(at) if at < output_timeout => at,
            _ => output_timeout,
        };
        if let Some(since) = closing
            && ((packets.is_empty() && since.elapsed() > Duration::from_millis(200))
                || since.elapsed() > Duration::from_secs(1))
        {
            return Ok(());
        }
        if !rtc.is_alive() || (channel.is_none() && started.elapsed() > Duration::from_secs(20)) {
            return Err(error("direct WebRTC connection unavailable"));
        }
        if timeout <= Instant::now() {
            rtc.handle_input(Input::Timeout(Instant::now()))
                .map_err(error)?;
            writable = true;
            continue;
        }
        // Consume ready packets before filling SCTP again. ACKs free its
        // congestion/receive windows; letting them sit behind an entire bulk
        // frame produces bursts, loss and expensive retransmission scans.
        let received = futures_lite::future::poll_once(network.udp.receive()).await;
        if received.is_none()
            && writable
            && let (Some(id), Some(packet)) = (channel, packets.front())
        {
            let accepted = rtc
                .channel(id)
                .ok_or_else(|| error("data channel closed"))?
                .write(true, packet)
                .map_err(error)?;
            if accepted {
                packets.pop_front();
            } else {
                writable = false;
            }
            continue; // Drain the complete RTC output after every mutation.
        }
        // With no queued packet, wait for outbound work below. Spinning
        // here starves the controller's first Hello and all later sends.
        enum Wake {
            Out(std::result::Result<Vec<u8>, async_channel::RecvError>),
            Udp(std::io::Result<Datagram>),
            Tick,
            Signal(Result<Value>),
        }
        let wake = if let Some(received) = received {
            Wake::Udp(received)
        } else {
            futures_lite::future::or(
                async {
                    if closing.is_some() {
                        std::future::pending::<()>().await;
                    }
                    Wake::Out(outbound.recv().await)
                },
                futures_lite::future::or(
                    async { Wake::Udp(network.udp.receive().await) },
                    futures_lite::future::or(
                        async {
                            let wait = timeout.saturating_duration_since(Instant::now());
                            let wait = if closing.is_some() {
                                wait.min(Duration::from_millis(20))
                            } else {
                                wait
                            };
                            futures_timer::Delay::new(wait).await;
                            Wake::Tick
                        },
                        async {
                            if !signal_alive {
                                std::future::pending::<()>().await;
                            }
                            match signal.as_mut() {
                                Some(socket) => Wake::Signal(read(socket).await),
                                None => std::future::pending::<Wake>().await,
                            }
                        },
                    ),
                ),
            )
            .await
        };
        match wake {
            Wake::Out(Ok(bytes)) => {
                if bytes.len() > MAX_MESSAGE {
                    return Err(error("USB message exceeds limit"));
                }
                for (index, chunk) in bytes.chunks(16000).enumerate() {
                    let mut packet = vec![u8::from((index + 1) * 16000 >= bytes.len())];
                    packet.extend_from_slice(chunk);
                    packets.push_back(packet);
                }
            }
            Wake::Out(Err(_)) => closing = Some(Instant::now()),
            Wake::Udp(result) => {
                let packet = result.map_err(error)?;
                writable = true;
                let from_turn = network
                    .relay
                    .as_mut()
                    .filter(|relay| packet.local == relay.socket);
                if let Some(relay) = from_turn {
                    // Only the TURN server speaks on the relay socket.
                    if packet.remote == relay.server {
                        let actions = relay
                            .client
                            .handle(Instant::now(), &packet.bytes[..packet.size]);
                        network.apply(&mut rtc, actions).await?;
                    }
                } else if let Ok(receive) = Receive::new(
                    Protocol::Udp,
                    packet.remote,
                    packet.local,
                    &packet.bytes[..packet.size],
                ) {
                    rtc.handle_input(Input::Receive(Instant::now(), receive))
                        .map_err(error)?;
                }
                network.udp.rearm(packet);
            }
            Wake::Tick => {
                rtc.handle_input(Input::Timeout(Instant::now()))
                    .map_err(error)?;
                writable = true;
            }
            Wake::Signal(Ok(frame)) if frame["type"] == "ended" => {
                return Err(error("camera sharing ended"));
            }
            Wake::Signal(Err(_)) => signal_alive = false,
            Wake::Signal(Ok(_)) => (),
        }
    }
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

#[cfg(all(test, feature = "relay"))]
mod relay_tests {
    use super::*;
    use crate::{CandidatePolicy, HostEvent, HostRoom, peer::turn::test_server};
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
    async fn relay_only_share_flows_through_turn_relays() {
        let credentials = test_server::credentials();
        let turn = test_server::spawn(credentials.clone());
        let ice = serde_json::json!([{
            "urls": [format!("turn:{}?transport=udp", turn.address)],
            "username": credentials.username,
            "credential": credentials.password,
        }]);
        let (link, _) = signaling_with(ice).await;
        let host = HostRoom::join(&link, "native-owner-secret", CandidatePolicy::RelayOnly)
            .await
            .unwrap();
        let steps = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let log = steps.clone();
        let progress: crate::Progress =
            std::sync::Arc::new(move |step: &str| log.lock().unwrap().push(step.to_owned()));
        let mut controller = PeerSocket::connect(&link, &progress, CandidatePolicy::RelayOnly)
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
        let host = HostRoom::join(&link, "native-owner-secret", CandidatePolicy::Any)
            .await
            .unwrap();
        let progress: crate::Progress = std::sync::Arc::new(|_| {});
        let failure = PeerSocket::connect(&link, &progress, CandidatePolicy::RelayOnly)
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
        let host = HostRoom::join(&link, "native-owner-secret", CandidatePolicy::RelayOnly)
            .await
            .unwrap();
        let steps = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let log = steps.clone();
        let progress: crate::Progress = std::sync::Arc::new(move |step: &str| {
            println!("{step}");
            log.lock().unwrap().push(step.to_owned());
        });
        let mut controller = PeerSocket::connect(&link, &progress, CandidatePolicy::RelayOnly)
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
