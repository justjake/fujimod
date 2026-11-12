//! A TURN server for tests (RFC 5766 over UDP, long-term credentials as in
//! RFC 8489), with the STUN message codec it needs.
use super::ice::Credentials;
use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt;
use sha1::Sha1;
use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
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
    use super::test_server::REALM_NAME;
    use super::*;

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
}
