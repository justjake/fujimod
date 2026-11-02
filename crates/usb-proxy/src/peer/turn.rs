//! TURN (RFC 5766 over UDP, long-term credentials as in RFC 8489) for native
//! share peers. A relayed candidate lets two peers connect when neither can
//! reach the other directly: each sends only to the TURN server.
//!
//! The client is sans-I/O. Its owner owns the socket, feeds it datagrams and
//! clock ticks, and transmits the bytes it returns.
use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt;
use serde_json::Value;
use sha1::Sha1;
use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    time::{Duration, Instant},
};

type HmacSha1 = Hmac<Sha1>;

const COOKIE: u32 = 0x2112_A442;
const HEADER: usize = 20;
const ALLOCATE: u16 = 0x0003;
const REFRESH: u16 = 0x0004;
const CREATE_PERMISSION: u16 = 0x0008;
const SEND: u16 = 0x0016;
const DATA_INDICATION: u16 = 0x0017;
const CLASS_MASK: u16 = 0x0110;
const SUCCESS: u16 = 0x0100;
const ERROR: u16 = 0x0110;

const USERNAME: u16 = 0x0006;
const MESSAGE_INTEGRITY: u16 = 0x0008;
const ERROR_CODE: u16 = 0x0009;
const LIFETIME: u16 = 0x000D;
const XOR_PEER_ADDRESS: u16 = 0x0012;
const DATA: u16 = 0x0013;
const REALM: u16 = 0x0014;
const NONCE: u16 = 0x0015;
const XOR_RELAYED_ADDRESS: u16 = 0x0016;
const REQUESTED_TRANSPORT: u16 = 0x0019;

/// An unanswered request is sent again this often, up to `RETRANSMISSIONS` times.
const RETRY_INTERVAL: Duration = Duration::from_millis(500);
const RETRANSMISSIONS: u32 = 6;
/// Lifetime assumed when the server does not state one (RFC 5766 §6.2).
const DEFAULT_LIFETIME: Duration = Duration::from_secs(600);
/// Permissions expire after five minutes (RFC 5766 §8).
const PERMISSION_INTERVAL: Duration = Duration::from_secs(240);
const REFRESH_RETRY: Duration = Duration::from_secs(5);
/// Authentication challenges answered before the allocation is refused.
const CHALLENGE_LIMIT: u32 = 4;

/// Long-term TURN credentials issued for one room.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// One `ice_servers` entry from a signaling join reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub credentials: Option<Credentials>,
}

/// The `ice_servers` of a join reply. Entries of an unexpected shape are skipped.
pub fn ice_servers(value: &Value) -> Vec<IceServer> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let urls = match &entry["urls"] {
                Value::String(url) => vec![url.clone()],
                Value::Array(list) => list
                    .iter()
                    .filter_map(|url| url.as_str().map(str::to_owned))
                    .collect(),
                _ => return None,
            };
            let credentials = match (entry["username"].as_str(), entry["credential"].as_str()) {
                (Some(username), Some(password)) => Some(Credentials {
                    username: username.to_owned(),
                    password: password.to_owned(),
                }),
                _ => None,
            };
            Some(IceServer { urls, credentials })
        })
        .collect()
}

/// The UDP TURN server to allocate on, and the credentials for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayTarget {
    pub host: String,
    pub port: u16,
    pub credentials: Credentials,
}

/// The first UDP TURN URL with credentials. TLS and TCP transports are not supported here.
pub fn relay_target(servers: &[IceServer]) -> Option<RelayTarget> {
    servers.iter().find_map(|server| {
        let credentials = server.credentials.clone()?;
        server
            .urls
            .iter()
            .find_map(|url| parse_udp_turn_url(url))
            .map(|(host, port)| RelayTarget {
                host,
                port,
                credentials,
            })
    })
}

/// `turn:host[:port][?transport=udp]`. UDP is the default transport; IPv6 literals are not supported.
fn parse_udp_turn_url(url: &str) -> Option<(String, u16)> {
    let rest = url.strip_prefix("turn:")?;
    let (address, query) = rest.split_once('?').unwrap_or((rest, ""));
    let transport = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("transport="));
    if transport.is_some_and(|transport| transport != "udp") || address.starts_with('[') {
        return None;
    }
    let (host, port) = match address.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (address, 3478),
    };
    (!host.is_empty()).then(|| (host.to_owned(), port))
}

/// Candidate addresses named by the `a=candidate:` lines of an SDP (IPv4 only).
/// Each one is a peer address the TURN server must allow to reach this client.
pub fn sdp_candidate_addresses(sdp: &str) -> Vec<IpAddr> {
    let mut addresses: Vec<IpAddr> = sdp
        .lines()
        .filter_map(|line| line.strip_prefix("a=candidate:"))
        .filter_map(|line| line.split_whitespace().nth(4)?.parse::<Ipv4Addr>().ok())
        .map(IpAddr::V4)
        .collect();
    addresses.sort();
    addresses.dedup();
    addresses
}

fn padding(length: usize) -> usize {
    (4 - length % 4) % 4
}

fn set_length(bytes: &mut [u8], length: usize) {
    bytes[2..4].copy_from_slice(&(length as u16).to_be_bytes());
}

fn push_attribute(bytes: &mut Vec<u8>, kind: u16, value: &[u8]) {
    bytes.extend_from_slice(&kind.to_be_bytes());
    bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
    bytes.extend_from_slice(value);
    bytes.resize(bytes.len() + padding(value.len()), 0);
}

fn integrity_tag(prefix: &[u8], key: &[u8; 16]) -> [u8; 20] {
    let mut mac = HmacSha1::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(prefix);
    mac.finalize().into_bytes().into()
}

/// The long-term key: MD5 of `username:realm:password` (RFC 5389 §15.4).
fn long_term_key(credentials: &Credentials, realm: &str) -> [u8; 16] {
    md5::compute(format!(
        "{}:{}:{}",
        credentials.username, realm, credentials.password
    ))
    .0
}

fn encode(
    kind: u16,
    transaction: [u8; 12],
    attributes: &[(u16, Vec<u8>)],
    key: Option<&[u8; 16]>,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + 64);
    bytes.extend_from_slice(&kind.to_be_bytes());
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&COOKIE.to_be_bytes());
    bytes.extend_from_slice(&transaction);
    for (attribute, value) in attributes {
        push_attribute(&mut bytes, *attribute, value);
    }
    if let Some(key) = key {
        // The integrity tag covers the header with its length counting the tag itself.
        let length = bytes.len() - HEADER + 24;
        set_length(&mut bytes, length);
        let tag = integrity_tag(&bytes, key);
        push_attribute(&mut bytes, MESSAGE_INTEGRITY, &tag);
    }
    let length = bytes.len() - HEADER;
    set_length(&mut bytes, length);
    bytes
}

/// Verify the integrity tag at `offset` (RFC 5389 §15.4).
fn verify_integrity(bytes: &[u8], offset: usize, key: &[u8; 16]) -> bool {
    let mut prefix = bytes[..offset].to_vec();
    set_length(&mut prefix, offset - HEADER + 24);
    let mut mac = HmacSha1::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&prefix);
    mac.verify_slice(&bytes[offset + 4..offset + 24]).is_ok()
}

#[derive(Debug)]
struct Message {
    kind: u16,
    transaction: [u8; 12],
    attributes: Vec<(u16, Vec<u8>)>,
    /// Offset of the MESSAGE-INTEGRITY attribute, when present.
    integrity: Option<usize>,
}

impl Message {
    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER || bytes[0] & 0xC0 != 0 {
            return None;
        }
        let length = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
        if bytes.len() != HEADER + length
            || u32::from_be_bytes(bytes[4..8].try_into().ok()?) != COOKIE
        {
            return None;
        }
        let mut attributes = Vec::new();
        let mut integrity = None;
        let mut at = HEADER;
        while at < bytes.len() {
            if at + 4 > bytes.len() {
                return None;
            }
            let kind = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
            let size = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
            let end = (at + 4).checked_add(size)?;
            if end > bytes.len() {
                return None;
            }
            if kind == MESSAGE_INTEGRITY {
                if size != 20 {
                    return None;
                }
                integrity.get_or_insert(at);
            }
            attributes.push((kind, bytes[at + 4..end].to_vec()));
            at = (end + padding(size)).min(bytes.len());
        }
        Some(Self {
            kind: u16::from_be_bytes([bytes[0], bytes[1]]),
            transaction: bytes[8..20].try_into().ok()?,
            attributes,
            integrity,
        })
    }

    fn attribute(&self, kind: u16) -> Option<&[u8]> {
        self.attributes
            .iter()
            .find(|(attribute, _)| *attribute == kind)
            .map(|(_, value)| value.as_slice())
    }
}

/// XOR-PEER-ADDRESS and related attributes; IPv4 only.
fn encode_address(address: SocketAddr) -> Option<Vec<u8>> {
    let SocketAddr::V4(address) = address else {
        return None;
    };
    let port = address.port() ^ (COOKIE >> 16) as u16;
    let ip = u32::from(*address.ip()) ^ COOKIE;
    let mut value = vec![0, 1];
    value.extend_from_slice(&port.to_be_bytes());
    value.extend_from_slice(&ip.to_be_bytes());
    Some(value)
}

fn decode_address(value: &[u8]) -> Option<SocketAddr> {
    if value.len() != 8 || value[1] != 1 {
        return None;
    }
    let port = u16::from_be_bytes([value[2], value[3]]) ^ (COOKIE >> 16) as u16;
    let ip = u32::from_be_bytes([value[4], value[5], value[6], value[7]]) ^ COOKIE;
    Some(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip), port)))
}

fn error_code(message: &Message) -> Option<u16> {
    let value = message.attribute(ERROR_CODE)?;
    (value.len() >= 4).then(|| u16::from(value[2] & 0x07) * 100 + u16::from(value[3]))
}

fn lifetime(message: &Message) -> Duration {
    message
        .attribute(LIFETIME)
        .and_then(|value| <[u8; 4]>::try_from(value).ok())
        .map_or(DEFAULT_LIFETIME, |value| {
            Duration::from_secs(u64::from(u32::from_be_bytes(value)))
        })
}

/// What the owner of the socket must do next.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Send these bytes to the TURN server.
    Transmit(Vec<u8>),
    /// The relayed transport address is ready to advertise.
    Allocated(SocketAddr),
    /// Application data arrived from a peer through the relay.
    Received { peer: SocketAddr, data: Vec<u8> },
    /// The allocation is refused or lost; no further relaying is possible.
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    Allocate,
    Refresh,
    Permission(IpAddr),
}

struct Pending {
    purpose: Purpose,
    bytes: Vec<u8>,
    sent: Instant,
    retransmissions: u32,
}

/// One UDP allocation on a TURN server.
pub struct TurnClient {
    credentials: Credentials,
    realm: Option<String>,
    nonce: Option<String>,
    key: Option<[u8; 16]>,
    challenges: u32,
    pending: HashMap<[u8; 12], Pending>,
    relayed: Option<SocketAddr>,
    lifetime: Duration,
    next_refresh: Option<Instant>,
    next_permissions: Option<Instant>,
    permissions: BTreeSet<IpAddr>,
}

impl TurnClient {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            credentials,
            realm: None,
            nonce: None,
            key: None,
            challenges: 0,
            pending: HashMap::new(),
            relayed: None,
            lifetime: DEFAULT_LIFETIME,
            next_refresh: None,
            next_permissions: None,
            permissions: BTreeSet::new(),
        }
    }

    #[cfg(test)]
    pub fn relayed(&self) -> Option<SocketAddr> {
        self.relayed
    }

    /// Begin the allocation handshake.
    pub fn start(&mut self, now: Instant) -> Vec<Action> {
        self.request(now, Purpose::Allocate)
    }

    /// Process a datagram from the TURN server.
    pub fn handle(&mut self, now: Instant, bytes: &[u8]) -> Vec<Action> {
        let Some(message) = Message::decode(bytes) else {
            return Vec::new();
        };
        if message.kind == DATA_INDICATION {
            return match (
                message.attribute(XOR_PEER_ADDRESS).and_then(decode_address),
                message.attribute(DATA),
            ) {
                (Some(peer), Some(data)) => vec![Action::Received {
                    peer,
                    data: data.to_vec(),
                }],
                _ => Vec::new(),
            };
        }
        let Some(purpose) = self
            .pending
            .get(&message.transaction)
            .map(|pending| pending.purpose)
        else {
            return Vec::new();
        };
        if let (Some(offset), Some(key)) = (message.integrity, self.key)
            && !verify_integrity(bytes, offset, &key)
        {
            return Vec::new();
        }
        self.pending.remove(&message.transaction);
        match message.kind & CLASS_MASK {
            SUCCESS => self.succeeded(now, purpose, &message),
            ERROR => self.refused(now, purpose, &message),
            _ => Vec::new(),
        }
    }

    /// Allow `peer` to send to this allocation. Permissions are refreshed on their own.
    pub fn permit(&mut self, now: Instant, peer: IpAddr) -> Vec<Action> {
        if !self.permissions.insert(peer) || self.relayed.is_none() {
            return Vec::new();
        }
        self.request(now, Purpose::Permission(peer))
    }

    /// Relay `data` to `peer` with a Send indication, permitting the peer first.
    pub fn send(&mut self, now: Instant, peer: SocketAddr, data: &[u8]) -> Vec<Action> {
        let Some(address) = encode_address(peer) else {
            return Vec::new();
        };
        if self.relayed.is_none() {
            return Vec::new();
        }
        let mut actions = self.permit(now, peer.ip());
        let transaction: [u8; 12] = rand::rng().random();
        let bytes = encode(
            SEND,
            transaction,
            &[(XOR_PEER_ADDRESS, address), (DATA, data.to_vec())],
            None,
        );
        actions.push(Action::Transmit(bytes));
        actions
    }

    /// Retransmit overdue requests and refresh the allocation and permissions when due.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let mut actions = Vec::new();
        let transactions: Vec<[u8; 12]> = self
            .pending
            .iter()
            .filter(|(_, pending)| now >= pending.sent + RETRY_INTERVAL)
            .map(|(transaction, _)| *transaction)
            .collect();
        for transaction in transactions {
            let Some(pending) = self.pending.get_mut(&transaction) else {
                continue;
            };
            if pending.retransmissions < RETRANSMISSIONS {
                pending.retransmissions += 1;
                pending.sent = now;
                actions.push(Action::Transmit(pending.bytes.clone()));
                continue;
            }
            let purpose = pending.purpose;
            self.pending.remove(&transaction);
            match purpose {
                Purpose::Allocate => {
                    actions.push(Action::Failed("the TURN server did not answer".into()))
                }
                Purpose::Refresh => {
                    actions.push(Action::Failed("the TURN refresh timed out".into()))
                }
                Purpose::Permission(_) => {}
            }
        }
        if self.next_refresh.is_some_and(|at| now >= at) {
            self.next_refresh = Some(now + REFRESH_RETRY);
            actions.extend(self.request(now, Purpose::Refresh));
        }
        if self.next_permissions.is_some_and(|at| now >= at) {
            self.next_permissions = Some(now + PERMISSION_INTERVAL);
            for peer in self.permissions.clone() {
                actions.extend(self.request(now, Purpose::Permission(peer)));
            }
        }
        actions
    }

    /// The next instant at which `tick` has work to do.
    pub fn next_timeout(&self) -> Option<Instant> {
        self.pending
            .values()
            .map(|pending| pending.sent + RETRY_INTERVAL)
            .chain(self.next_refresh)
            .chain(self.next_permissions)
            .min()
    }

    fn request(&mut self, now: Instant, purpose: Purpose) -> Vec<Action> {
        let mut attributes = Vec::new();
        let kind = match purpose {
            Purpose::Allocate => {
                // UDP transport, RFC 5766 §6.1.
                attributes.push((REQUESTED_TRANSPORT, vec![17, 0, 0, 0]));
                ALLOCATE
            }
            Purpose::Refresh => REFRESH,
            Purpose::Permission(peer) => {
                let Some(address) = encode_address(SocketAddr::new(peer, 0)) else {
                    return Vec::new();
                };
                attributes.push((XOR_PEER_ADDRESS, address));
                CREATE_PERMISSION
            }
        };
        let transaction: [u8; 12] = rand::rng().random();
        let bytes = match (self.realm.clone(), self.nonce.clone(), self.key) {
            (Some(realm), Some(nonce), Some(key)) => {
                attributes.push((USERNAME, self.credentials.username.clone().into_bytes()));
                attributes.push((REALM, realm.into_bytes()));
                attributes.push((NONCE, nonce.into_bytes()));
                encode(kind, transaction, &attributes, Some(&key))
            }
            _ => encode(kind, transaction, &attributes, None),
        };
        self.pending.insert(
            transaction,
            Pending {
                purpose,
                bytes: bytes.clone(),
                sent: now,
                retransmissions: 0,
            },
        );
        vec![Action::Transmit(bytes)]
    }

    fn succeeded(&mut self, now: Instant, purpose: Purpose, message: &Message) -> Vec<Action> {
        match purpose {
            Purpose::Allocate => {
                let Some(relayed) = message
                    .attribute(XOR_RELAYED_ADDRESS)
                    .and_then(decode_address)
                else {
                    return vec![Action::Failed(
                        "the TURN allocation reply named no relayed address".into(),
                    )];
                };
                self.relayed = Some(relayed);
                self.lifetime = lifetime(message);
                self.challenges = 0;
                self.next_refresh = Some(now + self.lifetime / 2);
                self.next_permissions = Some(now + PERMISSION_INTERVAL);
                let mut actions = vec![Action::Allocated(relayed)];
                for peer in self.permissions.clone() {
                    actions.extend(self.request(now, Purpose::Permission(peer)));
                }
                actions
            }
            Purpose::Refresh => {
                self.lifetime = lifetime(message);
                self.next_refresh = Some(now + self.lifetime / 2);
                Vec::new()
            }
            Purpose::Permission(_) => Vec::new(),
        }
    }

    fn refused(&mut self, now: Instant, purpose: Purpose, message: &Message) -> Vec<Action> {
        let code = error_code(message).unwrap_or(0);
        if code == 401 || code == 438 {
            // A challenge or stale nonce: adopt the server's realm and nonce, then resend.
            let realm = message
                .attribute(REALM)
                .map(|v| String::from_utf8_lossy(v).into_owned());
            let nonce = message
                .attribute(NONCE)
                .map(|v| String::from_utf8_lossy(v).into_owned());
            self.challenges += 1;
            if let (Some(realm), Some(nonce)) = (realm, nonce)
                && self.challenges <= CHALLENGE_LIMIT
            {
                self.key = Some(long_term_key(&self.credentials, &realm));
                self.realm = Some(realm);
                self.nonce = Some(nonce);
                return self.request(now, purpose);
            }
        }
        match purpose {
            Purpose::Allocate => vec![Action::Failed(format!(
                "the TURN allocation was refused (error {code})"
            ))],
            Purpose::Refresh => vec![Action::Failed(format!(
                "the TURN refresh was refused (error {code})"
            ))],
            Purpose::Permission(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_server {
    //! A TURN server for tests: long-term credentials, permissions, Send and
    //! Data indications. Each allocation relays through a loopback socket.
    use super::*;
    use std::{
        net::UdpSocket,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        thread::{self, JoinHandle},
    };

    pub const REALM_NAME: &str = "fujimod.test";

    /// Credentials the test server accepts.
    pub fn credentials() -> Credentials {
        Credentials {
            username: "1700000000:room".into(),
            password: "shared-secret".into(),
        }
    }
    const NONCE_VALUE: &str = "nonce-1";

    #[derive(Default)]
    pub struct Stats {
        pub allocations: AtomicUsize,
        pub sends: AtomicUsize,
        pub indications: AtomicUsize,
    }

    pub struct Server {
        pub address: SocketAddr,
        pub credentials: Credentials,
        pub stats: Arc<Stats>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    struct Allocation {
        relay: Arc<UdpSocket>,
        permissions: Arc<Mutex<BTreeSet<IpAddr>>>,
    }

    pub fn spawn(credentials: Credentials) -> Server {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
        socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let address = socket.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Stats::default());
        let server = {
            let (stop, stats, credentials) = (stop.clone(), stats.clone(), credentials.clone());
            thread::spawn(move || run(socket, credentials, stop, stats))
        };
        Server {
            address,
            credentials: credentials.clone(),
            stats,
            stop,
            thread: Some(server),
        }
    }

    fn run(
        socket: Arc<UdpSocket>,
        credentials: Credentials,
        stop: Arc<AtomicBool>,
        stats: Arc<Stats>,
    ) {
        let key = long_term_key(&credentials, REALM_NAME);
        let mut allocations: HashMap<SocketAddr, Allocation> = HashMap::new();
        let mut buffer = vec![0u8; 2048];
        while !stop.load(Ordering::SeqCst) {
            let Ok((size, from)) = socket.recv_from(&mut buffer) else {
                continue;
            };
            let Some(message) = Message::decode(&buffer[..size]) else {
                continue;
            };
            if message.kind == SEND {
                if let (Some(peer), Some(data), Some(allocation)) = (
                    message.attribute(XOR_PEER_ADDRESS).and_then(decode_address),
                    message.attribute(DATA),
                    allocations.get(&from),
                ) {
                    // Count before relaying: the peer may reply before this thread resumes.
                    stats.sends.fetch_add(1, Ordering::SeqCst);
                    let _ = allocation.relay.send_to(data, peer);
                }
                continue;
            }
            let method = message.kind & !CLASS_MASK;
            if !matches!(method, ALLOCATE | REFRESH | CREATE_PERMISSION) {
                continue;
            }
            let authorized = match (message.attribute(USERNAME), message.integrity) {
                (Some(user), Some(offset))
                    if user == credentials.username.as_bytes()
                        && message.attribute(NONCE) == Some(NONCE_VALUE.as_bytes()) =>
                {
                    verify_integrity(&buffer[..size], offset, &key)
                }
                _ => false,
            };
            if !authorized {
                let challenge = vec![
                    (ERROR_CODE, vec![0, 0, 4, 1, b'U']),
                    (REALM, REALM_NAME.as_bytes().to_vec()),
                    (NONCE, NONCE_VALUE.as_bytes().to_vec()),
                ];
                let bytes = encode(method | ERROR, message.transaction, &challenge, None);
                let _ = socket.send_to(&bytes, from);
                continue;
            }
            let reply = match method {
                ALLOCATE => {
                    let relay = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
                    relay
                        .set_read_timeout(Some(Duration::from_millis(20)))
                        .unwrap();
                    let relayed = relay.local_addr().unwrap();
                    let permissions = Arc::new(Mutex::new(BTreeSet::new()));
                    spawn_relay(
                        relay.clone(),
                        permissions.clone(),
                        socket.clone(),
                        from,
                        stop.clone(),
                        stats.clone(),
                    );
                    allocations.insert(from, Allocation { relay, permissions });
                    stats.allocations.fetch_add(1, Ordering::SeqCst);
                    let attributes = vec![
                        (XOR_RELAYED_ADDRESS, encode_address(relayed).unwrap()),
                        (LIFETIME, 600u32.to_be_bytes().to_vec()),
                    ];
                    encode(
                        ALLOCATE | SUCCESS,
                        message.transaction,
                        &attributes,
                        Some(&key),
                    )
                }
                REFRESH => {
                    if !allocations.contains_key(&from) {
                        encode(
                            REFRESH | ERROR,
                            message.transaction,
                            &[(ERROR_CODE, vec![0, 0, 4, 37])],
                            None,
                        )
                    } else {
                        let attributes = vec![(LIFETIME, 600u32.to_be_bytes().to_vec())];
                        encode(
                            REFRESH | SUCCESS,
                            message.transaction,
                            &attributes,
                            Some(&key),
                        )
                    }
                }
                _ => {
                    let peer = message.attribute(XOR_PEER_ADDRESS).and_then(decode_address);
                    match (allocations.get(&from), peer) {
                        (Some(allocation), Some(peer)) => {
                            allocation.permissions.lock().unwrap().insert(peer.ip());
                            encode(
                                CREATE_PERMISSION | SUCCESS,
                                message.transaction,
                                &[],
                                Some(&key),
                            )
                        }
                        _ => encode(
                            CREATE_PERMISSION | ERROR,
                            message.transaction,
                            &[(ERROR_CODE, vec![0, 0, 4, 37])],
                            None,
                        ),
                    }
                }
            };
            let _ = socket.send_to(&reply, from);
        }
    }

    /// Relay datagrams from permitted peers to the client as Data indications.
    fn spawn_relay(
        relay: Arc<UdpSocket>,
        permissions: Arc<Mutex<BTreeSet<IpAddr>>>,
        control: Arc<UdpSocket>,
        client: SocketAddr,
        stop: Arc<AtomicBool>,
        stats: Arc<Stats>,
    ) {
        thread::spawn(move || {
            let mut buffer = vec![0u8; 2048];
            while !stop.load(Ordering::SeqCst) {
                let Ok((size, peer)) = relay.recv_from(&mut buffer) else {
                    continue;
                };
                if !permissions.lock().unwrap().contains(&peer.ip()) {
                    continue;
                }
                let transaction: [u8; 12] = rand::rng().random();
                let bytes = encode(
                    DATA_INDICATION,
                    transaction,
                    &[
                        (XOR_PEER_ADDRESS, encode_address(peer).unwrap()),
                        (DATA, buffer[..size].to_vec()),
                    ],
                    None,
                );
                stats.indications.fetch_add(1, Ordering::SeqCst);
                let _ = control.send_to(&bytes, client);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{REALM_NAME, spawn};
    use super::*;
    use std::net::ToSocketAddrs;
    use std::sync::atomic::Ordering;

    fn credentials() -> Credentials {
        Credentials {
            username: "1700000000:room".into(),
            password: "shared-secret".into(),
        }
    }

    #[test]
    fn integrity_detects_tampering_and_the_wrong_key() {
        let key = long_term_key(&credentials(), REALM_NAME);
        let attributes = vec![
            (USERNAME, b"1700000000:room".to_vec()),
            (REALM, REALM_NAME.as_bytes().to_vec()),
            (NONCE, b"nonce".to_vec()),
        ];
        let bytes = encode(ALLOCATE, [7; 12], &attributes, Some(&key));
        let message = Message::decode(&bytes).unwrap();
        let offset = message.integrity.unwrap();
        assert!(verify_integrity(&bytes, offset, &key));
        assert!(!verify_integrity(&bytes, offset, &[0; 16]));
        let mut tampered = bytes.clone();
        tampered[HEADER + 4] ^= 1;
        assert!(!verify_integrity(&tampered, offset, &key));
    }

    #[test]
    fn decode_rejects_truncated_and_foreign_messages() {
        let bytes = encode(SEND, [1; 12], &[(DATA, vec![1, 2, 3])], None);
        assert!(Message::decode(&bytes).is_some());
        assert!(Message::decode(&bytes[..bytes.len() - 1]).is_none());
        let mut wrong_cookie = bytes.clone();
        wrong_cookie[4] ^= 0xff;
        assert!(Message::decode(&wrong_cookie).is_none());
        assert!(Message::decode(&[0; 19]).is_none());
    }

    #[test]
    fn xor_addresses_round_trip_and_hide_the_address() {
        let address: SocketAddr = "203.0.113.9:3478".parse().unwrap();
        let encoded = encode_address(address).unwrap();
        assert_eq!(decode_address(&encoded), Some(address));
        assert!(!encoded.windows(4).any(|window| window == [203, 0, 113, 9]));
        assert!(encode_address("[::1]:1".parse().unwrap()).is_none());
    }

    #[test]
    fn only_udp_turn_urls_are_selected() {
        assert_eq!(
            parse_udp_turn_url("turn:turn.example.test:3478?transport=udp"),
            Some(("turn.example.test".into(), 3478))
        );
        assert_eq!(
            parse_udp_turn_url("turn:turn.example.test"),
            Some(("turn.example.test".into(), 3478))
        );
        assert_eq!(
            parse_udp_turn_url("turn:turn.example.test:53?transport=udp"),
            Some(("turn.example.test".into(), 53))
        );
        assert_eq!(
            parse_udp_turn_url("turns:turn.example.test:5349?transport=tcp"),
            None
        );
        assert_eq!(
            parse_udp_turn_url("turn:turn.example.test:3478?transport=tcp"),
            None
        );
        assert_eq!(parse_udp_turn_url("stun:stun.example.test:3478"), None);
        assert_eq!(parse_udp_turn_url("turn:[2001:db8::1]:3478"), None);
    }

    #[test]
    fn ice_servers_keep_credentials_and_pick_the_udp_relay() {
        let reply = serde_json::json!([
            {"urls": ["stun:stun.example.test:3478"]},
            {
                "urls": [
                    "turn:turn.example.test:3478?transport=udp",
                    "turn:turn.example.test:3478?transport=tcp",
                    "turns:turn.example.test:5349?transport=tcp"
                ],
                "username": "1700000000:room",
                "credential": "shared-secret"
            },
            {"urls": 7},
        ]);
        let servers = ice_servers(&reply);
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].credentials, None);
        let target = relay_target(&servers).unwrap();
        assert_eq!(
            (target.host.as_str(), target.port),
            ("turn.example.test", 3478)
        );
        assert_eq!(
            target.credentials,
            credentials_for("1700000000:room", "shared-secret")
        );
        assert!(
            relay_target(&ice_servers(
                &serde_json::json!([{"urls": ["turn:a:3478"]}])
            ))
            .is_none()
        );
        assert!(ice_servers(&serde_json::json!({"not": "a list"})).is_empty());
    }

    fn credentials_for(username: &str, password: &str) -> Credentials {
        Credentials {
            username: username.into(),
            password: password.into(),
        }
    }

    #[test]
    fn sdp_candidates_name_their_addresses_once() {
        let sdp = "v=0\r\na=candidate:1 1 udp 2130706431 192.0.2.4 5000 typ host\r\n\
                   a=candidate:2 1 udp 1694498815 198.51.100.7 61000 typ relay raddr 192.0.2.4 rport 5000\r\n\
                   a=candidate:3 1 udp 1 2001:db8::1 9 typ host\r\n\
                   a=candidate:4 1 udp 1 192.0.2.4 5001 typ srflx\r\n";
        assert_eq!(
            sdp_candidate_addresses(sdp),
            vec![
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 4)),
                IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)),
            ]
        );
    }

    /// Run the client's traffic with the server until `finished` holds for the
    /// actions seen so far, or until `window` elapses. Returns every action seen.
    fn exchange(
        client: &mut TurnClient,
        socket: &std::net::UdpSocket,
        server: SocketAddr,
        mut actions: Vec<Action>,
        window: Duration,
        finished: impl Fn(&[Action]) -> bool,
    ) -> Vec<Action> {
        let mut seen = Vec::new();
        let deadline = Instant::now() + window;
        let mut buffer = vec![0u8; 2048];
        loop {
            for action in actions.drain(..) {
                if let Action::Transmit(bytes) = &action {
                    socket.send_to(bytes, server).unwrap();
                }
                seen.push(action);
            }
            if finished(&seen) || Instant::now() >= deadline {
                return seen;
            }
            socket
                .set_read_timeout(Some(Duration::from_millis(20)))
                .unwrap();
            actions = match socket.recv_from(&mut buffer) {
                Ok((size, from)) if from == server => {
                    client.handle(Instant::now(), &buffer[..size])
                }
                _ => client.tick(Instant::now()),
            };
        }
    }

    #[test]
    fn client_allocates_permits_and_relays_both_ways() {
        let server = spawn(credentials());
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut client = TurnClient::new(server.credentials.clone());
        let actions = client.start(Instant::now());
        let allocated = exchange(
            &mut client,
            &socket,
            server.address,
            actions,
            Duration::from_secs(10),
            |seen| {
                seen.iter()
                    .any(|action| matches!(action, Action::Allocated(_) | Action::Failed(_)))
            },
        );
        let relayed = allocated
            .iter()
            .find_map(|action| match action {
                Action::Allocated(address) => Some(*address),
                _ => None,
            })
            .expect("allocation");
        assert_eq!(client.relayed(), Some(relayed));

        // The send permits the peer's address before it relays, so the server
        // processes the permission first; the peer then receives the datagram.
        let peer_address = peer.local_addr().unwrap();
        let send = client.send(Instant::now(), peer_address, b"from client");
        exchange(
            &mut client,
            &socket,
            server.address,
            send,
            Duration::from_millis(200),
            |_| false,
        );
        let mut buffer = vec![0u8; 2048];
        let (size, from) = peer.recv_from(&mut buffer).expect("relayed datagram");
        assert_eq!(&buffer[..size], b"from client");
        assert_eq!(from, relayed);

        peer.send_to(b"from peer", relayed).unwrap();
        let received = exchange(
            &mut client,
            &socket,
            server.address,
            Vec::new(),
            Duration::from_secs(5),
            |seen| {
                seen.iter()
                    .any(|action| matches!(action, Action::Received { .. }))
            },
        );
        assert!(received.iter().any(|action| matches!(
            action,
            Action::Received { peer, data } if *peer == peer_address && data == b"from peer"
        )));
        assert_eq!(server.stats.allocations.load(Ordering::SeqCst), 1);
        assert!(server.stats.sends.load(Ordering::SeqCst) >= 1);
        assert!(server.stats.indications.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn wrong_password_is_refused_after_the_challenge_limit() {
        let server = spawn(credentials());
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let wrong = Credentials {
            username: credentials().username,
            password: "not-the-secret".into(),
        };
        let mut client = TurnClient::new(wrong);
        let actions = client.start(Instant::now());
        let outcome = exchange(
            &mut client,
            &socket,
            server.address,
            actions,
            Duration::from_secs(10),
            |seen| {
                seen.iter()
                    .any(|action| matches!(action, Action::Allocated(_) | Action::Failed(_)))
            },
        );
        assert!(matches!(outcome.last(), Some(Action::Failed(_))));
        assert_eq!(client.relayed(), None);
    }

    /// Relay a datagram between two allocations on a live TURN server. Credentials
    /// come from the environment (a room's minted `ice_servers` entry), never from source.
    #[test]
    #[ignore = "uses a live TURN server; set FUJIMOD_TURN_LIVE_URL, _USER and _CREDENTIAL"]
    fn live_relay_round_trip() {
        let url = std::env::var("FUJIMOD_TURN_LIVE_URL").expect("FUJIMOD_TURN_LIVE_URL");
        let credentials = Credentials {
            username: std::env::var("FUJIMOD_TURN_LIVE_USER").expect("FUJIMOD_TURN_LIVE_USER"),
            password: std::env::var("FUJIMOD_TURN_LIVE_CREDENTIAL")
                .expect("FUJIMOD_TURN_LIVE_CREDENTIAL"),
        };
        let (host, port) = parse_udp_turn_url(&url).expect("a UDP turn: URL");
        let server = (host.as_str(), port)
            .to_socket_addrs()
            .unwrap()
            .find(SocketAddr::is_ipv4)
            .expect("an IPv4 TURN address");
        let socket_a = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let socket_b = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let mut a = TurnClient::new(credentials.clone());
        let mut b = TurnClient::new(credentials);
        let start_a = a.start(Instant::now());
        let allocated_a = exchange(
            &mut a,
            &socket_a,
            server,
            start_a,
            Duration::from_secs(10),
            |seen| {
                seen.iter()
                    .any(|action| matches!(action, Action::Allocated(_) | Action::Failed(_)))
            },
        );
        let start_b = b.start(Instant::now());
        let allocated_b = exchange(
            &mut b,
            &socket_b,
            server,
            start_b,
            Duration::from_secs(10),
            |seen| {
                seen.iter()
                    .any(|action| matches!(action, Action::Allocated(_) | Action::Failed(_)))
            },
        );
        let relay_a = allocated_a
            .iter()
            .find_map(|action| match action {
                Action::Allocated(address) => Some(*address),
                _ => None,
            })
            .expect("allocation A");
        let relay_b = allocated_b
            .iter()
            .find_map(|action| match action {
                Action::Allocated(address) => Some(*address),
                _ => None,
            })
            .expect("allocation B");
        println!("allocated relays on {server}: A {relay_a}, B {relay_b}");

        // Each side permits the other's relay address, then A sends to B's relay.
        let permit_a = a.permit(Instant::now(), relay_b.ip());
        let permit_b = b.permit(Instant::now(), relay_a.ip());
        exchange(
            &mut a,
            &socket_a,
            server,
            permit_a,
            Duration::from_millis(500),
            |_| false,
        );
        exchange(
            &mut b,
            &socket_b,
            server,
            permit_b,
            Duration::from_millis(500),
            |_| false,
        );
        let send = a.send(Instant::now(), relay_b, b"relay ping");
        exchange(
            &mut a,
            &socket_a,
            server,
            send,
            Duration::from_millis(300),
            |_| false,
        );
        let received = exchange(
            &mut b,
            &socket_b,
            server,
            Vec::new(),
            Duration::from_secs(10),
            |seen| {
                seen.iter()
                    .any(|action| matches!(action, Action::Received { .. }))
            },
        );
        assert!(
            received.iter().any(|action| matches!(
                action,
                Action::Received { peer, data } if *peer == relay_a && data == b"relay ping"
            )),
            "B did not receive A's relayed datagram: {received:?}"
        );
        println!("RELAY OK: A -> TURN -> B");
    }
}
