//! str0m is driven by one owned I/O task. It works under native Python, Tokio,
//! smol or a plain block_on; no caller runtime or callbacks are required.
use super::datagram::{Datagram, DatagramIo};
use crate::{Error, Result, protocol::MAX_MESSAGE, share::Ticket};
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
use futures_util::StreamExt;
use rand::Rng;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    net::{SocketAddr, ToSocketAddrs},
    time::{Duration, Instant},
};
use str0m::{
    Candidate, Event, Input, Output, Rtc,
    change::SdpAnswer,
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
            Nonce::from_slice(&iv),
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
            Nonce::from_slice(&bytes[..12]),
            Payload {
                msg: &bytes[12..],
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| error("signaling authentication failed"))?;
    serde_json::from_slice(&decoded).map_err(error)
}
pub(crate) struct PeerSocket {
    send: Sender<Vec<u8>>,
    receive: Receiver<Result<Vec<u8>>>,
    /// Closed when the I/O thread has flushed and exited.
    finished: Receiver<()>,
}
impl PeerSocket {
    pub async fn connect(url: &str, progress: &crate::Progress) -> Result<Self> {
        let ticket = Ticket::from_url(url)?.ok_or_else(|| error("invalid share URL"))?;
        progress("share link parsed");
        let progress = progress.clone();
        let (tx, outbound) = async_channel::bounded(1);
        // Unbounded: results queue while the controller is still sending; its
        // in-flight limit bounds them. A bounded queue would block the I/O thread.
        let (incoming, rx) = async_channel::unbounded();
        let (ready, wait) = async_channel::bounded(1);
        let (finish, finished) = async_channel::bounded::<()>(1);
        std::thread::Builder::new()
            .name("fujimod-webrtc".into())
            .spawn(move || {
                futures_lite::future::block_on(async {
                    if let Err(error) =
                        run(ticket, outbound, incoming.clone(), ready.clone(), progress).await
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
async fn run(
    ticket: Ticket,
    outbound: Receiver<Vec<u8>>,
    incoming: Sender<Result<Vec<u8>>>,
    ready: Sender<std::result::Result<(), String>>,
    progress: crate::Progress,
) -> Result<()> {
    // str0m counts unacknowledged bytes against this bound. Its 128 KiB
    // default limits a bulk sender to 128 KiB per RTT (6.6 MB/s at 20 ms).
    // Reserve a bounded bulk window rather than the interactive default.
    let mut rtc = Rtc::builder()
        .set_sctp_max_buffered_amount(512 * 1024)
        .build(Instant::now());
    let mut local = Vec::new();
    for iface in if_addrs::get_if_addrs().map_err(error)? {
        if iface.ip().is_ipv4() {
            local.push(iface.ip());
        }
    }
    local.sort();
    local.dedup();
    let mut sockets = Vec::new();
    for ip in &local {
        let socket = std::net::UdpSocket::bind(SocketAddr::new(*ip, 0)).map_err(error)?;
        let addr = socket.local_addr().map_err(error)?;
        rtc.add_local_candidate(Candidate::host(addr, "udp").map_err(error)?);
        drain_setup(&mut rtc)?;
        sockets.push(socket);
    }
    progress(&format!("local candidates: {}", local.len()));
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
                drain_setup(&mut rtc)?;
            }
        }
    }
    let mut udp = DatagramIo::new(sockets).map_err(error)?;
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
    let peer = welcome["peer"]
        .as_str()
        .filter(|_| welcome["type"] == "ready")
        .ok_or_else(|| error("share unavailable: no owner in this room"))?
        .to_owned();
    progress("joined room; share owner present");
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
    rtc.sdp_api()
        .accept_answer(
            pending,
            SdpAnswer::from_sdp_string(answer["sdp"].as_str().ok_or_else(|| error("missing SDP"))?)
                .map_err(error)?,
        )
        .map_err(error)?;
    progress("WebRTC answer accepted; connecting ICE");
    let mut channel: Option<ChannelId> = None;
    let mut assembled = Vec::new();
    let mut packets: VecDeque<Vec<u8>> = VecDeque::new();
    let mut writable = true;
    let mut signal_alive = true;
    let started = Instant::now();
    // Set when the controller closes: flush queued messages, then exit.
    let mut closing: Option<Instant> = None;
    loop {
        let timeout = loop {
            match rtc.poll_output().map_err(error)? {
                Output::Timeout(at) => break at,
                Output::Transmit(packet) => {
                    // A candidate pair this host cannot route to (an IPv6 or
                    // foreign LAN address) is a failed ICE check, not a failed
                    // connection: ICE carries on with the other pairs.
                    if let Err(failure) = udp
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
        let received = futures_lite::future::poll_once(udp.receive()).await;
        if received.is_none() && writable {
            if let (Some(id), Some(packet)) = (channel, packets.front()) {
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
        }
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
                    async { Wake::Udp(udp.receive().await) },
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
                            Wake::Signal(read(&mut signal).await)
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
                if let Ok(receive) = Receive::new(
                    Protocol::Udp,
                    packet.remote,
                    packet.local,
                    &packet.bytes[..packet.size],
                ) {
                    rtc.handle_input(Input::Receive(Instant::now(), receive))
                        .map_err(error)?;
                }
                udp.rearm(packet);
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
