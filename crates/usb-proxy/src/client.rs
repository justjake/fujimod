use crate::{Error, Result, protocol::*};
#[cfg(target_arch = "wasm32")]
use futures_util::SinkExt;
use futures_util::StreamExt;
use std::{
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

#[cfg(not(target_arch = "wasm32"))]
type WebSocket = async_tungstenite::WebSocketStream<async_tungstenite::smol::ConnectStream>;
#[cfg(target_arch = "wasm32")]
type WebSocket = gloo_net::websocket::futures::WebSocket;

fn err(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}
enum Socket {
    Web(Box<WebSocket>),
    Peer(crate::peer::PeerSocket),
}
async fn send(socket: &mut Socket, message: &Message) -> Result<()> {
    let bytes = message.encode()?;
    match socket {
        Socket::Peer(peer) => peer.send(bytes).await,
        Socket::Web(socket) => {
            #[cfg(not(target_arch = "wasm32"))]
            let message = async_tungstenite::tungstenite::Message::Binary(bytes.into());
            #[cfg(target_arch = "wasm32")]
            let message = gloo_net::websocket::Message::Bytes(bytes);
            socket.send(message).await.map_err(err)
        }
    }
}
async fn receive(socket: &mut Socket) -> Result<Message> {
    let bytes = match socket {
        Socket::Peer(peer) => peer.receive().await?,
        Socket::Web(socket) => loop {
            let message = socket
                .next()
                .await
                .ok_or_else(|| err("disconnected; completion unknown"))?
                .map_err(err)?;
            #[cfg(not(target_arch = "wasm32"))]
            match message {
                async_tungstenite::tungstenite::Message::Binary(bytes) => break bytes.to_vec(),
                async_tungstenite::tungstenite::Message::Ping(_)
                | async_tungstenite::tungstenite::Message::Pong(_) => continue,
                _ => return Err(err("expected binary frame")),
            }
            #[cfg(target_arch = "wasm32")]
            match message {
                gloo_net::websocket::Message::Bytes(bytes) => break bytes,
                _ => return Err(err("expected binary frame")),
            }
        },
    };
    if bytes.len() > MAX_MESSAGE {
        return Err(err("message exceeds limit"));
    }
    Message::decode(&bytes)
}
async fn deadline<T>(
    duration: Duration,
    task: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    futures_lite::future::or(task, async {
        futures_timer::Delay::new(duration).await;
        Err(err("deadline expired; completion unknown"))
    })
    .await
}

/// The connect reason shown to the device owner: FUJIMOD_USB_PROXY_REASON, or
/// this process's command line.
fn default_reason() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    return None;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let text = std::env::var("FUJIMOD_USB_PROXY_REASON")
            .unwrap_or_else(|_| std::env::args().collect::<Vec<_>>().join(" "));
        (!text.is_empty()).then(|| bounded_reason(&text))
    }
}

/// Receives each connection step as it completes, before any USB request.
pub type Progress = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// Time beyond an operation's own USB deadline allowed for the network and
/// the exporter's turnaround before its completion is declared unknown.
/// Generous for poor Wi-Fi: a lost connection is reported when the channel
/// closes, so this only bounds a silent stall.
const NETWORK_ALLOWANCE: Duration = Duration::from_secs(60);

/// What became of one pipelined operation.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// It ran; a USB failure is in `error` and failed the pipeline.
    Completed(Completion),
    /// Skipped without touching USB because an earlier operation failed.
    Discarded,
    /// An expectation held.
    Held,
    /// An expectation failed; these are the bytes found.
    Failed(Vec<u8>),
}

struct InFlight {
    seq: u64,
    payload: usize,
    timeout: Duration,
}

/// One controller connection, pipelined like the Postgres extended-query
/// protocol: `submit` and `expect` send at once without waiting, the exporter
/// runs operations strictly in order, and `sync` waits for every result.
/// After a failure the exporter discards later operations until the sync.
/// Never retried: a lost connection or expired deadline permanently fences it.
pub struct Client {
    socket: Socket,
    next_seq: u64,
    usable: bool,
    /// Set while a call waits; still set at the next call if it was canceled.
    busy: bool,
    /// Operations sent since the last sync, in order; `true` for expectations.
    unsynced: Vec<(u64, bool)>,
    /// Operations awaiting their result, oldest first, for flow control and deadlines.
    in_flight: VecDeque<InFlight>,
    in_flight_bytes: usize,
    outcomes: BTreeMap<u64, Outcome>,
    /// The first failed operation since the last sync.
    failed: Option<u64>,
    /// The error that fenced this connection, sent as the disconnect reason.
    failure: Option<String>,
}
impl Client {
    pub async fn connect(url: &str, token: &str) -> Result<Self> {
        Self::connect_with_progress(url, token, std::sync::Arc::new(|_: &str| {})).await
    }
    /// Connect, reporting each step. A failure names the last step reached, so
    /// signaling, WebRTC and the proxy handshake can be told apart.
    pub async fn connect_with_progress(url: &str, token: &str, progress: Progress) -> Result<Self> {
        let reached = std::sync::Arc::new(std::sync::Mutex::new(String::from("started")));
        let report: Progress = {
            let reached = reached.clone();
            std::sync::Arc::new(move |step: &str| {
                *reached.lock().unwrap() = step.to_owned();
                progress(step);
            })
        };
        Self::connect_reporting(url, token, report)
            .await
            .map_err(|e| {
                err(format!(
                    "{} (last step reached: {})",
                    e.0,
                    reached.lock().unwrap()
                ))
            })
    }
    async fn connect_reporting(url: &str, token: &str, progress: Progress) -> Result<Self> {
        let ticket = crate::share::Ticket::from_url(url)?;
        let token = ticket.as_ref().map(|t| t.key.as_str()).unwrap_or(token);
        deadline(
            Duration::from_secs(if ticket.is_some() { 30 } else { 10 }),
            async {
                let mut socket = if ticket.is_some() {
                    Socket::Peer(crate::peer::PeerSocket::connect(url, &progress).await?)
                } else {
                    #[cfg(not(target_arch = "wasm32"))]
                    let socket = async_tungstenite::smol::connect_async(url)
                        .await
                        .map_err(err)?
                        .0;
                    #[cfg(target_arch = "wasm32")]
                    let socket = WebSocket::open(url).map_err(err)?;
                    progress("relay WebSocket connected");
                    Socket::Web(Box::new(socket))
                };
                send(
                    &mut socket,
                    &Message::Hello {
                        version: VERSION,
                        role: "controller".into(),
                        token: token.into(),
                        reason: default_reason(),
                    },
                )
                .await?;
                progress("proxy handshake sent");
                match receive(&mut socket).await? {
                    Message::Ready {
                        version: VERSION, ..
                    } => Ok({
                        progress("proxy handshake ready");
                        Self {
                            socket,
                            next_seq: 0,
                            usable: true,
                            busy: false,
                            unsynced: Vec::new(),
                            in_flight: VecDeque::new(),
                            in_flight_bytes: 0,
                            outcomes: BTreeMap::new(),
                            failed: None,
                            failure: None,
                        }
                    }),
                    _ => Err(err("relay rejected protocol handshake")),
                }
            },
        )
        .await
    }
    fn fence(&mut self, error: Error) -> Error {
        self.usable = false;
        self.failure.get_or_insert_with(|| error.0.clone());
        error
    }
    /// Start a call. A call still marked busy was canceled mid-wait, so
    /// what it sent may or may not have run: fence.
    fn begin(&mut self) -> Result<()> {
        if self.busy {
            return Err(self.fence(err("an earlier call was canceled; completion unknown")));
        }
        // After a USB failure, calls up to the sync only record discards.
        if self.failed.is_none() {
            self.check_usable()?;
        }
        self.busy = true;
        Ok(())
    }
    fn check_usable(&self) -> Result<()> {
        if self.usable {
            Ok(())
        } else {
            Err(err("connection fenced; completion unknown"))
        }
    }
    /// Send one operation now; its outcome arrives with the next `sync`.
    pub async fn submit(&mut self, device: &str, op: Operation, timeout: Duration) -> Result<u64> {
        if timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(err("timeout must be 1–120000 ms"));
        }
        op.validate()?;
        let payload = op.payload();
        self.begin()?;
        while self.failed.is_none()
            && !self.in_flight.is_empty()
            && (self.in_flight.len() >= MAX_IN_FLIGHT
                || self.in_flight_bytes + payload > MAX_IN_FLIGHT_BYTES)
        {
            self.receive_one().await?;
        }
        self.next_seq += 1;
        let seq = self.next_seq;
        // After a failure the exporter would discard it; it is not sent.
        if self.failed.is_none() {
            let message = Message::Execute {
                seq,
                device: device.into(),
                timeout_ms: timeout.as_millis() as u32,
                op,
            };
            send(&mut self.socket, &message)
                .await
                .map_err(|e| self.fence(e))?;
            self.in_flight.push_back(InFlight {
                seq,
                payload,
                timeout,
            });
            self.in_flight_bytes += payload;
        }
        self.busy = false;
        self.unsynced.push((seq, false));
        Ok(seq)
    }
    /// Fail the pipeline at the exporter unless bytes `offset..` of IN result
    /// `of` equal `bytes` (both ANDed with `mask` when given). Later operations
    /// then run only if it held, without a round trip.
    pub async fn expect(
        &mut self,
        of: u64,
        offset: usize,
        bytes: Vec<u8>,
        mask: Option<Vec<u8>>,
        length: Option<u32>,
        alternatives: Vec<Vec<u8>>,
    ) -> Result<u64> {
        let message = Message::Expect {
            seq: self.next_seq + 1,
            of,
            offset: u32::try_from(offset).map_err(|_| err("expectation offset out of range"))?,
            bytes,
            mask: mask.map(serde_bytes::ByteBuf::from),
            length,
            alternatives: alternatives
                .into_iter()
                .map(serde_bytes::ByteBuf::from)
                .collect(),
        };
        message.validate()?;
        self.begin()?;
        self.next_seq += 1;
        if self.failed.is_none() {
            send(&mut self.socket, &message)
                .await
                .map_err(|e| self.fence(e))?;
        }
        self.busy = false;
        self.unsynced.push((self.next_seq, true));
        Ok(self.next_seq)
    }
    /// Wait until every operation sent so far has finished or been discarded,
    /// and return their outcomes in order. Ends a failed pipeline's discarding.
    pub async fn sync(&mut self) -> Result<Vec<(u64, Outcome)>> {
        if self.unsynced.is_empty() {
            return Ok(Vec::new());
        }
        if self.busy {
            return Err(self.fence(err("an earlier call was canceled; completion unknown")));
        }
        let mut synced = false;
        if self.usable {
            self.next_seq += 1;
            let seq = self.next_seq;
            self.busy = true;
            let sent = send(&mut self.socket, &Message::Sync { seq }).await;
            match sent {
                Ok(()) => {
                    loop {
                        match self.receive_one().await {
                            Ok(Some(done)) if done == seq => {
                                synced = true;
                                break;
                            }
                            Ok(_) => (),
                            Err(e) if self.failed.is_some() => {
                                // The exporter ends the connection after a USB
                                // failure; nothing after it ran.
                                let _ = self.fence(e);
                                break;
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
                Err(e) if self.failed.is_some() => drop(self.fence(e)),
                Err(e) => return Err(self.fence(e)),
            }
        } else if self.failed.is_none() {
            return Err(err("connection fenced; completion unknown"));
        }
        self.busy = false;
        let failed = self.failed.take();
        let mut outcomes = Vec::with_capacity(self.unsynced.len());
        for (seq, expectation) in std::mem::take(&mut self.unsynced) {
            let outcome = match self.outcomes.remove(&seq) {
                Some(outcome) => outcome,
                None if failed.is_some_and(|f| seq > f) => Outcome::Discarded,
                None if expectation && synced => Outcome::Held,
                None => return Err(self.fence(err("missing pipeline result; completion unknown"))),
            };
            outcomes.push((seq, outcome));
        }
        self.in_flight.clear();
        self.in_flight_bytes = 0;
        Ok(outcomes)
    }
    /// Receive one message within the oldest operation's deadline; returns
    /// the sequence number of a Synced.
    async fn receive_one(&mut self) -> Result<Option<u64>> {
        let wait = self.in_flight.front().map_or(Duration::ZERO, |f| f.timeout) + NETWORK_ALLOWANCE;
        let message = deadline(wait, receive(&mut self.socket))
            .await
            .map_err(|e| self.fence(e))?;
        match message {
            Message::Result { seq, completion } => {
                let Some(position) = self.in_flight.iter().position(|f| f.seq == seq) else {
                    return Err(self.fence(err("unexpected pipeline result")));
                };
                let done = self.in_flight.remove(position).unwrap();
                self.in_flight_bytes -= done.payload;
                if let Some(error) = &completion.error {
                    self.failed.get_or_insert(seq);
                    self.failure.get_or_insert_with(|| error.clone());
                    // Every exporter ends the connection after a USB failure.
                    self.usable = false;
                    self.release_after(seq);
                }
                self.outcomes.insert(seq, Outcome::Completed(completion));
                Ok(None)
            }
            Message::ExpectFailed { seq, actual } => {
                if !self.unsynced.iter().any(|&(s, e)| s == seq && e) {
                    return Err(self.fence(err("unexpected expectation result")));
                }
                self.failed.get_or_insert(seq);
                self.release_after(seq);
                self.outcomes.insert(seq, Outcome::Failed(actual));
                Ok(None)
            }
            Message::Synced { seq, .. } => Ok(Some(seq)),
            _ => Err(self.fence(err("unexpected message from exporter"))),
        }
    }
    /// Operations after a failure will be discarded; stop waiting for them.
    fn release_after(&mut self, failed: u64) {
        self.in_flight.retain(|f| f.seq < failed);
        self.in_flight_bytes = self.in_flight.iter().map(|f| f.payload).sum();
    }
    /// One operation in its own pipeline.
    pub async fn request(
        &mut self,
        device: &str,
        op: Operation,
        timeout: Duration,
    ) -> Result<Completion> {
        let seq = self.submit(device, op, timeout).await?;
        match self.sync().await?.into_iter().find(|(s, _)| *s == seq) {
            Some((_, Outcome::Completed(completion))) => Ok(completion),
            _ => Err(err("operation not run")),
        }
    }
    /// Send the owner a notice. It reaches the owner in pipeline order, when
    /// the exporter has finished every earlier operation.
    pub async fn notify_owner(&mut self, message: Message) -> Result<()> {
        if !message.is_notice() {
            return Err(err(
                "only status, attention, activity and progress messages are notices",
            ));
        }
        self.begin()?;
        // A USB failure has ended the connection; the notice has no reader.
        if self.usable {
            send(&mut self.socket, &message)
                .await
                .map_err(|e| self.fence(e))?;
        }
        self.busy = false;
        Ok(())
    }
    pub async fn devices(&mut self) -> Result<Vec<Device>> {
        let completion = self
            .request("", Operation::List, Duration::from_secs(5))
            .await?;
        if let Some(error) = completion.error {
            return Err(err(error));
        }
        Ok(completion.devices)
    }
    /// Discovery owns only descriptors. Opening uses a fresh controller socket.
    pub async fn discover(url: &str, token: &str) -> Result<Vec<RemoteDevice>> {
        let mut client = Self::connect(url, token).await?;
        Ok(client
            .devices()
            .await?
            .into_iter()
            .map(|descriptor| RemoteDevice {
                descriptor,
                url: url.into(),
                token: token.into(),
            })
            .collect())
    }
}

/// Tell the owner why this controller is leaving: "done", or the error that
/// fenced the connection. Best effort; a lost goodbye shows as a lost connection.
#[cfg(not(target_arch = "wasm32"))]
impl Drop for Client {
    fn drop(&mut self) {
        if let Socket::Peer(peer) = &mut self.socket {
            let reason = match &self.failure {
                Some(error) => bounded_reason(&format!("ended after error: {error}")),
                None => "done".to_owned(),
            };
            if let Ok(bytes) = (Message::Goodbye { reason }).encode() {
                peer.goodbye(bytes);
            }
        }
    }
}

#[derive(Clone)]
pub struct RemoteDevice {
    pub descriptor: Device,
    url: String,
    token: String,
}
impl std::fmt::Debug for RemoteDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteDevice")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}
impl RemoteDevice {
    pub async fn connect(&self) -> Result<Client> {
        Client::connect(&self.url, &self.token).await
    }
}
