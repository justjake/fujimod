//! Host-side virtual network behind the MT5931 data path.
//!
//! The peer is a deterministic IPv4 network: one DHCP server, one gateway,
//! two DNS servers, an optional NTP server, and a TCP relay to a host
//! loopback port. Ethernet frames are consumed from the chip's data FIFO and
//! produced for its receive FIFO. Packet headers and checksums use smoltcp's
//! wire layer; the server state is our own and serializes with the device.
use serde::{Deserialize, Serialize};
use smoltcp::wire::{
    DhcpMessageType, DhcpPacket, DhcpRepr, EthernetAddress, EthernetFrame, IpAddress, IpProtocol,
    Ipv4Address, Ipv4Packet, TcpPacket, UdpPacket,
};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};

const DHCP_CLIENT_PORT: u16 = 68;
const DHCP_SERVER_PORT: u16 = 67;
const DNS_PORT: u16 = 53;
const NTP_PORT: u16 = 123;
const MAX_CONNECTIONS: usize = 8;
const TCP_WINDOW: u16 = 8192;
const TCP_MSS: usize = 1024;
const DHCP_LEASE_SECONDS: u32 = 3600;
const HOST_CONNECT_TIMEOUT_MS: u64 = 200;
const TCP_RETRANSMIT_MS: u64 = 1000;
const NTP_UNIX_OFFSET_SECONDS: u64 = 2_208_988_800;
const NTP_MAX_UNIX_SECONDS: u64 = u32::MAX as u64 - NTP_UNIX_OFFSET_SECONDS;

/// One NTP server on a fixed UTC time, advancing with the network clock.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ntp {
    /// Name the DNS server resolves to `address`.
    pub hostname: String,
    pub address: [u8; 4],
    /// UTC at the network's zero tick, seconds since 1970.
    pub seconds: u64,
}

impl Ntp {
    fn validate(&self) -> Result<(), String> {
        if self.hostname.is_empty() || self.hostname.len() > 253 {
            return Err("virtual NTP name must be 1..=253 bytes".into());
        }
        if self.address == [0; 4] || self.address == [255; 4] {
            return Err("virtual NTP address must be unicast".into());
        }
        if self.seconds > NTP_MAX_UNIX_SECONDS {
            return Err("virtual NTP time is outside the 32-bit NTP epoch".into());
        }
        Ok(())
    }
}

/// One configured access point and its attached virtual network. A present
/// WPA2 passphrase advertises RSN/CCMP; absence advertises an open network.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPoint {
    pub ssid: String,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rcpi: u8,
    pub gateway: [u8; 4],
    pub dns_servers: [[u8; 4]; 2],
    pub hostname: String,
    pub host: [u8; 4],
    pub host_port: u16,
    pub local_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wpa2_passphrase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ntp: Option<Ntp>,
}

impl AccessPoint {
    pub fn validate(&self) -> Result<(), String> {
        if self.ssid.is_empty() || self.ssid.len() > 32 {
            return Err("virtual access point SSID must be 1..=32 bytes".into());
        }
        if self.bssid == [0; 6] || self.bssid[0] & 1 != 0 {
            return Err("virtual access point BSSID must be a nonzero unicast address".into());
        }
        if !(1..=14).contains(&self.channel) {
            return Err("virtual access point channel must be 1..=14".into());
        }
        if self.rcpi > 220 {
            return Err("virtual access point RCPI must be at most 220".into());
        }
        if self.gateway[3] == 0 || self.gateway[3] == 255 {
            return Err("virtual gateway must be a host address in its subnet".into());
        }
        if self
            .dns_servers
            .iter()
            .any(|dns| *dns == [0; 4] || *dns == [255; 4])
        {
            return Err("virtual DNS server addresses must be unicast".into());
        }
        if self.host == [0; 4] || self.host == [255; 4] {
            return Err("virtual host address must be unicast".into());
        }
        if self.hostname.is_empty() || self.hostname.len() > 253 {
            return Err("virtual DNS name must be 1..=253 bytes".into());
        }
        if self.host_port == 0 || self.local_port == 0 {
            return Err("virtual host and local ports must be nonzero".into());
        }
        if let Some(passphrase) = &self.wpa2_passphrase
            && !(8..=63).contains(&passphrase.len())
        {
            return Err("virtual WPA2 passphrase must be 8..=63 bytes".into());
        }
        if let Some(ntp) = &self.ntp {
            ntp.validate()?;
        }
        Ok(())
    }

    pub fn subnet(&self) -> [u8; 4] {
        [self.gateway[0], self.gateway[1], self.gateway[2], 0]
    }

    pub fn netmask(&self) -> [u8; 4] {
        [255, 255, 255, 0]
    }

    pub fn broadcast(&self) -> [u8; 4] {
        [self.gateway[0], self.gateway[1], self.gateway[2], 255]
    }

    pub fn client_address(&self) -> [u8; 4] {
        let subnet = self.subnet();
        let mut address = [subnet[0], subnet[1], subnet[2], 100];
        while self.configured_addresses().contains(&address) {
            address[3] = address[3].wrapping_add(1);
            if address[3] == 255 {
                address[3] = 1;
            }
        }
        address
    }

    fn configured_addresses(&self) -> Vec<[u8; 4]> {
        let mut addresses = vec![
            self.gateway,
            self.dns_servers[0],
            self.dns_servers[1],
            self.host,
        ];
        if let Some(ntp) = &self.ntp {
            addresses.push(ntp.address);
        }
        addresses
    }

    fn local_addresses(&self) -> Vec<[u8; 4]> {
        let mut addresses = self.configured_addresses();
        addresses.push(self.client_address());
        addresses
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub client: [u8; 6],
    pub address: [u8; 4],
    pub transaction: u32,
    pub expires_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TcpState {
    SynReceived,
    Established,
    FinWait,
    Closing,
    Closed,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    guest: [u8; 4],
    guest_port: u16,
    state: TcpState,
    server_sequence: u32,
    guest_sequence: u32,
    server_next: u32,
    guest_next: u32,
    server_acked: u32,
    pending: Vec<u8>,
    unsent: usize,
    retransmit_at: u64,
    host_eof: bool,
    host_fin_sent: bool,
    guest_fin: bool,
    #[serde(skip)]
    stream: Option<TcpStream>,
}

impl Connection {
    fn new(
        guest: [u8; 4],
        guest_port: u16,
        guest_sequence: u32,
        server_sequence: u32,
        local_port: u16,
    ) -> Self {
        let stream = connect_host(local_port);
        Self {
            guest,
            guest_port,
            state: if stream.is_some() {
                TcpState::SynReceived
            } else {
                TcpState::Closed
            },
            server_sequence,
            guest_sequence,
            server_next: server_sequence.wrapping_add(1),
            guest_next: guest_sequence.wrapping_add(1),
            server_acked: server_sequence,
            pending: Vec::new(),
            unsent: 0,
            retransmit_at: 0,
            host_eof: stream.is_none(),
            host_fin_sent: false,
            guest_fin: false,
            stream,
        }
    }
}

fn connect_host(port: u16) -> Option<TcpStream> {
    let address: SocketAddr = ([127, 0, 0, 1], port).into();
    let stream = TcpStream::connect_timeout(
        &address,
        std::time::Duration::from_millis(HOST_CONNECT_TIMEOUT_MS),
    )
    .ok()?;
    stream.set_nonblocking(true).ok()?;
    Some(stream)
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct Network {
    guest_mac: Option<[u8; 6]>,
    lease: Option<Lease>,
    connections: Vec<Connection>,
    sequence_counter: u32,
    polls: u64,
    dns_questions: u64,
    ntp_requests: u64,
}

impl Network {
    pub fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({
            "guest_mac": self.guest_mac,
            "lease": self.lease,
            "connections": serde_json::to_value(&self.connections).unwrap_or_default(),
            "sequence_counter": self.sequence_counter,
            "polls": self.polls,
            "dns_questions": self.dns_questions,
            "ntp_requests": self.ntp_requests,
        })
    }

    pub fn guest_mac(&self) -> Option<[u8; 6]> {
        self.guest_mac
    }

    pub fn lease(&self) -> Option<&Lease> {
        self.lease.as_ref()
    }

    /// Consume one Ethernet frame from the station and return frames for it.
    pub fn receive(&mut self, ap: &AccessPoint, frame: &[u8], now_ms: u64) -> Vec<Vec<u8>> {
        let Ok(ethernet) = EthernetFrame::new_checked(frame) else {
            return Vec::new();
        };
        let source = ethernet.src_addr();
        if source != EthernetAddress::BROADCAST && source != EthernetAddress([0; 6]) {
            self.guest_mac = Some(source.0);
        }
        match ethernet.ethertype() {
            smoltcp::wire::EthernetProtocol::Arp => self.receive_arp(ap, ethernet.payload()),
            smoltcp::wire::EthernetProtocol::Ipv4 => {
                self.receive_ipv4(ap, ethernet.payload(), now_ms)
            }
            _ => Vec::new(),
        }
    }

    /// Poll host-side TCP streams and produce any newly available frames.
    pub fn poll(&mut self, ap: &AccessPoint, now_ms: u64) -> Vec<Vec<u8>> {
        self.polls = self.polls.saturating_add(1);
        let mut frames = Vec::new();
        let guest_mac = self.guest_mac.unwrap_or([0xff; 6]);
        let mut index = 0;
        while index < self.connections.len() {
            let remove = poll_connection(
                ap,
                &mut self.connections[index],
                now_ms,
                &mut frames,
                guest_mac,
            );
            if remove {
                self.connections.remove(index);
            } else {
                index += 1;
            }
        }
        frames
    }

    /// Whether a host TCP connection is still relaying data.
    pub fn has_connections(&self) -> bool {
        !self.connections.is_empty()
    }

    pub fn next_poll_ms(&self) -> Option<u64> {
        (!self.connections.is_empty()).then_some(1)
    }

    fn receive_arp(&mut self, ap: &AccessPoint, payload: &[u8]) -> Vec<Vec<u8>> {
        if payload.len() < 28
            || payload[..2] != [0, 1]
            || payload[2..4] != [8, 0]
            || payload[4] != 6
        {
            return Vec::new();
        }
        let operation = u16::from_be_bytes([payload[6], payload[7]]);
        let sender_mac: [u8; 6] = payload[8..14].try_into().unwrap();
        if operation != 1 {
            return Vec::new();
        }
        let target: [u8; 4] = payload[24..28].try_into().unwrap();
        if !ap.local_addresses().contains(&target) {
            return Vec::new();
        }
        self.guest_mac = Some(sender_mac);
        let mut reply = vec![0u8; 28];
        reply[..2].copy_from_slice(&[0, 1]);
        reply[2..4].copy_from_slice(&[8, 0]);
        reply[4] = 6;
        reply[5] = 4;
        reply[6..8].copy_from_slice(&2u16.to_be_bytes());
        reply[8..14].copy_from_slice(&ap.bssid);
        reply[14..18].copy_from_slice(&target);
        reply[18..24].copy_from_slice(&sender_mac);
        reply[24..28].copy_from_slice(&payload[14..18]);
        vec![ethernet_frame(&sender_mac, &ap.bssid, 0x0806, &reply)]
    }

    fn receive_ipv4(&mut self, ap: &AccessPoint, payload: &[u8], now_ms: u64) -> Vec<Vec<u8>> {
        let Ok(ip) = Ipv4Packet::new_checked(payload) else {
            return Vec::new();
        };
        let source = ip.src_addr().octets();
        let destination = ip.dst_addr().octets();
        let payload = ip.payload();
        match ip.next_header() {
            IpProtocol::Udp => self.receive_udp(ap, source, destination, payload, now_ms),
            IpProtocol::Tcp => self.receive_tcp(ap, source, destination, payload, now_ms),
            IpProtocol::Icmp => Vec::new(),
            _ => Vec::new(),
        }
    }

    fn receive_udp(
        &mut self,
        ap: &AccessPoint,
        source: [u8; 4],
        destination: [u8; 4],
        payload: &[u8],
        now_ms: u64,
    ) -> Vec<Vec<u8>> {
        let Ok(udp) = UdpPacket::new_checked(payload) else {
            return Vec::new();
        };
        let source_port = udp.src_port();
        let destination_port = udp.dst_port();
        let payload = udp.payload();
        if destination_port == DHCP_SERVER_PORT && source_port == DHCP_CLIENT_PORT {
            return self.receive_dhcp(ap, source, destination, payload, now_ms);
        }
        if destination_port == DNS_PORT && ap.dns_servers.contains(&destination) {
            return self.receive_dns(ap, source, destination, source_port, payload);
        }
        if destination_port == NTP_PORT
            && let Some(ntp) = &ap.ntp
            && destination == ntp.address
        {
            return self.receive_ntp(ap, ntp, source, destination, source_port, payload, now_ms);
        }
        Vec::new()
    }

    fn receive_dhcp(
        &mut self,
        ap: &AccessPoint,
        source: [u8; 4],
        destination: [u8; 4],
        payload: &[u8],
        now_ms: u64,
    ) -> Vec<Vec<u8>> {
        let Ok(packet) = DhcpPacket::new_checked(payload) else {
            return Vec::new();
        };
        let Ok(repr) = DhcpRepr::parse(&packet) else {
            return Vec::new();
        };
        let client = repr.client_hardware_address.0;
        self.guest_mac = Some(client);
        let offered = ap.client_address();
        let requested = repr.requested_ip.map(|ip| ip.octets());
        let message_type = match repr.message_type {
            DhcpMessageType::Discover => DhcpMessageType::Offer,
            DhcpMessageType::Request => DhcpMessageType::Ack,
            _ => return Vec::new(),
        };
        if message_type == DhcpMessageType::Ack
            && requested.is_some_and(|requested| requested != offered)
        {
            return Vec::new();
        }
        self.lease = Some(Lease {
            client,
            address: offered,
            transaction: repr.transaction_id,
            expires_ms: now_ms.saturating_add(u64::from(DHCP_LEASE_SECONDS) * 1000),
        });
        let dns = [
            Ipv4Address::new(
                ap.dns_servers[0][0],
                ap.dns_servers[0][1],
                ap.dns_servers[0][2],
                ap.dns_servers[0][3],
            ),
            Ipv4Address::new(
                ap.dns_servers[1][0],
                ap.dns_servers[1][1],
                ap.dns_servers[1][2],
                ap.dns_servers[1][3],
            ),
        ];
        let reply = DhcpRepr {
            message_type,
            transaction_id: repr.transaction_id,
            secs: 0,
            client_hardware_address: repr.client_hardware_address,
            client_ip: repr.client_ip,
            your_ip: Ipv4Address::new(offered[0], offered[1], offered[2], offered[3]),
            server_ip: Ipv4Address::new(ap.gateway[0], ap.gateway[1], ap.gateway[2], ap.gateway[3]),
            router: Some(Ipv4Address::new(
                ap.gateway[0],
                ap.gateway[1],
                ap.gateway[2],
                ap.gateway[3],
            )),
            subnet_mask: Some(Ipv4Address::new(
                ap.netmask()[0],
                ap.netmask()[1],
                ap.netmask()[2],
                ap.netmask()[3],
            )),
            relay_agent_ip: Ipv4Address::UNSPECIFIED,
            broadcast: repr.broadcast || source == [0; 4],
            requested_ip: None,
            client_identifier: None,
            server_identifier: Some(Ipv4Address::new(
                ap.gateway[0],
                ap.gateway[1],
                ap.gateway[2],
                ap.gateway[3],
            )),
            parameter_request_list: None,
            dns_servers: Some(dns.into_iter().collect()),
            max_size: None,
            lease_duration: Some(DHCP_LEASE_SECONDS),
            renew_duration: Some(DHCP_LEASE_SECONDS / 2),
            rebind_duration: Some(DHCP_LEASE_SECONDS * 7 / 8),
            additional_options: &[],
        };
        let mut dhcp = vec![0u8; reply.buffer_len()];
        let mut out = DhcpPacket::new_unchecked(&mut dhcp[..]);
        if reply.emit(&mut out).is_err() {
            return Vec::new();
        }
        let broadcast = reply.broadcast;
        let destination_mac = if broadcast { [255; 6] } else { client };
        let destination_ip = if broadcast {
            [255, 255, 255, 255]
        } else {
            offered
        };
        let _ = destination;
        let frame = udp_frame(
            &destination_mac,
            &ap.bssid,
            ap.gateway,
            destination_ip,
            DHCP_SERVER_PORT,
            DHCP_CLIENT_PORT,
            &dhcp,
        );
        vec![frame]
    }

    fn receive_dns(
        &mut self,
        ap: &AccessPoint,
        source: [u8; 4],
        destination: [u8; 4],
        source_port: u16,
        payload: &[u8],
    ) -> Vec<Vec<u8>> {
        self.dns_questions = self.dns_questions.saturating_add(1);
        let Some(mac) = self.guest_mac else {
            return Vec::new();
        };
        let Some(query) = parse_dns_query(payload) else {
            return Vec::new();
        };
        let expected = ap.hostname.trim_end_matches('.').to_ascii_lowercase();
        let ntp_name = ap
            .ntp
            .as_ref()
            .map(|ntp| ntp.hostname.trim_end_matches('.').to_ascii_lowercase());
        let address = if query.name.eq_ignore_ascii_case(expected.as_bytes()) {
            Some(ap.host)
        } else if ntp_name.is_some_and(|name| query.name.eq_ignore_ascii_case(name.as_bytes())) {
            ap.ntp.as_ref().map(|ntp| ntp.address)
        } else {
            None
        };
        let known = address.is_some();
        let answer = known && query.kind == 1;
        let mut reply = Vec::with_capacity(query.question.len() + 28);
        reply.extend_from_slice(&query.identifier.to_be_bytes());
        let mut flags = 0x8000u16 | 0x0400; // response, authoritative
        if query.flags & 0x0100 != 0 {
            flags |= 0x0080; // recursion desired -> available
        }
        if !known {
            flags |= 3; // NXDOMAIN
        }
        reply.extend_from_slice(&flags.to_be_bytes());
        reply.extend_from_slice(&1u16.to_be_bytes());
        reply.extend_from_slice(&(if answer { 1u16 } else { 0 }).to_be_bytes());
        reply.extend_from_slice(&0u16.to_be_bytes());
        reply.extend_from_slice(&0u16.to_be_bytes());
        reply.extend_from_slice(&query.question);
        if answer {
            reply.extend_from_slice(&[0xc0, 0x0c]);
            reply.extend_from_slice(&1u16.to_be_bytes());
            reply.extend_from_slice(&1u16.to_be_bytes());
            reply.extend_from_slice(&60u32.to_be_bytes());
            reply.extend_from_slice(&4u16.to_be_bytes());
            reply.extend_from_slice(&address.unwrap_or([0; 4]));
        }
        let frame = udp_frame(
            &mac,
            &ap.bssid,
            destination,
            source,
            DNS_PORT,
            source_port,
            &reply,
        );
        vec![frame]
    }

    fn receive_ntp(
        &mut self,
        ap: &AccessPoint,
        ntp: &Ntp,
        source: [u8; 4],
        destination: [u8; 4],
        source_port: u16,
        payload: &[u8],
        now_ms: u64,
    ) -> Vec<Vec<u8>> {
        self.ntp_requests = self.ntp_requests.saturating_add(1);
        let Some(mac) = self.guest_mac else {
            return Vec::new();
        };
        if payload.len() < 48 {
            return Vec::new();
        }
        let version = (payload[0] >> 3) & 7;
        let mode = payload[0] & 7;
        if !(3..=4).contains(&version) || mode != 3 {
            return Vec::new();
        }
        let seconds = ntp.seconds.saturating_add(now_ms / 1000);
        let timestamp = ntp_timestamp(seconds, (now_ms % 1000) as u32);
        let mut reply = vec![0u8; 48];
        reply[0] = (version << 3) | 4;
        reply[1] = 2;
        reply[2] = 6;
        reply[3] = 0xec;
        reply[4..8].copy_from_slice(&0x0000_0010u32.to_be_bytes());
        reply[8..12].copy_from_slice(&0x0000_0010u32.to_be_bytes());
        reply[12..16].copy_from_slice(b"LOCL");
        reply[16..24].copy_from_slice(&timestamp);
        reply[24..32].copy_from_slice(&payload[40..48]);
        reply[32..40].copy_from_slice(&timestamp);
        reply[40..48].copy_from_slice(&timestamp);
        let frame = udp_frame(
            &mac,
            &ap.bssid,
            destination,
            source,
            NTP_PORT,
            source_port,
            &reply,
        );
        vec![frame]
    }

    fn receive_tcp(
        &mut self,
        ap: &AccessPoint,
        source: [u8; 4],
        destination: [u8; 4],
        payload: &[u8],
        now_ms: u64,
    ) -> Vec<Vec<u8>> {
        let Ok(tcp) = TcpPacket::new_checked(payload) else {
            return Vec::new();
        };
        if destination != ap.host || tcp.dst_port() != ap.host_port || ap.local_port == 0 {
            return Vec::new();
        }
        let source_port = tcp.src_port();
        let mut frames = Vec::new();
        let index = self.connections.iter().position(|connection| {
            connection.guest == source && connection.guest_port == source_port
        });
        if tcp.syn() && !tcp.ack() {
            if index.is_none() {
                if self.connections.len() >= MAX_CONNECTIONS {
                    frames.push(Self::tcp_reset(
                        ap,
                        self.guest_mac.unwrap_or([0xff; 6]),
                        source,
                        source_port,
                        tcp.seq_number().0 as u32,
                    ));
                    return frames;
                }
                self.sequence_counter = self.sequence_counter.wrapping_add(1);
                let server_sequence = 0x5a000000u32.wrapping_add(self.sequence_counter);
                let connection = Connection::new(
                    source,
                    source_port,
                    tcp.seq_number().0 as u32,
                    server_sequence,
                    ap.local_port,
                );
                self.connections.push(connection);
            }
            let connection = self
                .connections
                .iter_mut()
                .find(|connection| {
                    connection.guest == source && connection.guest_port == source_port
                })
                .unwrap();
            if connection.state == TcpState::Closed {
                frames.push(Self::tcp_reset(
                    ap,
                    self.guest_mac.unwrap_or([0xff; 6]),
                    source,
                    source_port,
                    tcp.seq_number().0 as u32,
                ));
            } else if connection.state == TcpState::SynReceived {
                frames.push(Self::tcp_segment(
                    ap,
                    self.guest_mac.unwrap_or([0xff; 6]),
                    connection,
                    connection.server_sequence,
                    true,
                    true,
                    false,
                    &[],
                ));
            }
            return frames;
        }
        let Some(index) = index else {
            frames.push(Self::tcp_reset(
                ap,
                self.guest_mac.unwrap_or([0xff; 6]),
                source,
                source_port,
                tcp.seq_number().0 as u32,
            ));
            return frames;
        };
        let mut remove = false;
        {
            let connection = &mut self.connections[index];
            if tcp.rst() {
                remove = true;
            } else {
                let acknowledgement = tcp.ack_number().0 as u32;
                if tcp.ack() {
                    if connection.state == TcpState::SynReceived
                        && acknowledgement == connection.server_next
                    {
                        connection.server_acked = acknowledgement;
                        connection.state = TcpState::Established;
                    } else {
                        acknowledge(connection, acknowledgement);
                    }
                    if connection.state == TcpState::FinWait
                        && acknowledgement == connection.server_next
                    {
                        remove = true;
                    }
                }
                let sequence = tcp.seq_number().0 as u32;
                let payload = tcp.payload();
                if !payload.is_empty() {
                    if sequence == connection.guest_next {
                        if let Some(stream) = connection.stream.as_mut() {
                            match stream.write_all(payload) {
                                Ok(()) => {}
                                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                                Err(_) => {
                                    connection.stream = None;
                                    remove = true;
                                }
                            }
                        }
                        connection.guest_next =
                            connection.guest_next.wrapping_add(payload.len() as u32);
                    }
                    if !remove {
                        frames.push(Self::tcp_segment(
                            ap,
                            self.guest_mac.unwrap_or([0xff; 6]),
                            connection,
                            connection.server_next,
                            true,
                            false,
                            false,
                            &[],
                        ));
                    }
                }
                if tcp.fin() && !connection.guest_fin {
                    connection.guest_fin = true;
                    connection.guest_next = connection.guest_next.wrapping_add(1);
                    if let Some(stream) = connection.stream.as_mut() {
                        let _ = stream.shutdown(std::net::Shutdown::Write);
                    }
                    connection.state = if connection.pending.is_empty() && connection.host_eof {
                        TcpState::Closed
                    } else {
                        TcpState::Closing
                    };
                    frames.push(Self::tcp_segment(
                        ap,
                        self.guest_mac.unwrap_or([0xff; 6]),
                        connection,
                        connection.server_next,
                        true,
                        false,
                        true,
                        &[],
                    ));
                    if connection.state == TcpState::Closed {
                        remove = true;
                    }
                }
                if connection.state == TcpState::Established && connection.guest_fin {
                    connection.state = TcpState::Closing;
                }
                if remove {
                    // No further state is observed after a reset or final close.
                }
            }
        }
        if remove {
            self.connections.remove(index);
        }
        frames.extend(self.poll(ap, now_ms));
        frames
    }

    fn tcp_reset(
        ap: &AccessPoint,
        destination_mac: [u8; 6],
        source: [u8; 4],
        source_port: u16,
        sequence: u32,
    ) -> Vec<u8> {
        let mut payload = vec![0u8; 20];
        {
            let mut tcp = TcpPacket::new_unchecked(&mut payload[..]);
            tcp.set_src_port(ap.host_port);
            tcp.set_dst_port(source_port);
            tcp.set_seq_number(smoltcp::wire::TcpSeqNumber(0));
            tcp.set_ack_number(smoltcp::wire::TcpSeqNumber(sequence.wrapping_add(1) as i32));
            tcp.set_header_len(20);
            tcp.set_rst(true);
            tcp.set_ack(true);
            tcp.set_window_len(0);
            tcp.fill_checksum(&ipv4(ap.host), &ipv4(source));
        }
        ipv4_frame(
            &ap.bssid,
            destination_mac,
            ap.host,
            source,
            IpProtocol::Tcp,
            &payload,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn tcp_segment(
        ap: &AccessPoint,
        destination_mac: [u8; 6],
        connection: &Connection,
        sequence: u32,
        ack: bool,
        syn: bool,
        fin: bool,
        data: &[u8],
    ) -> Vec<u8> {
        let mut payload = vec![0u8; 20 + data.len()];
        {
            let mut tcp = TcpPacket::new_unchecked(&mut payload[..]);
            tcp.set_src_port(ap.host_port);
            tcp.set_dst_port(connection.guest_port);
            tcp.set_seq_number(smoltcp::wire::TcpSeqNumber(sequence as i32));
            tcp.set_ack_number(smoltcp::wire::TcpSeqNumber(connection.guest_next as i32));
            tcp.set_header_len(20);
            tcp.set_ack(ack);
            tcp.set_syn(syn);
            tcp.set_fin(fin);
            tcp.set_window_len(TCP_WINDOW);
            tcp.set_urgent_at(0);
            if !data.is_empty() {
                tcp.payload_mut().copy_from_slice(data);
            }
            tcp.fill_checksum(&ipv4(ap.host), &ipv4(connection.guest));
        }
        ipv4_frame(
            &ap.bssid,
            destination_mac,
            ap.host,
            connection.guest,
            IpProtocol::Tcp,
            &payload,
        )
    }
}

fn acknowledge(connection: &mut Connection, acknowledgement: u32) {
    let acked = acknowledgement.wrapping_sub(connection.server_acked);
    if acked <= TCP_WINDOW as u32 && acked as usize <= connection.pending.len() {
        connection.pending.drain(..acked as usize);
        // Unsent counts the tail of pending that never went into a segment;
        // acknowledging already-sent bytes leaves it unchanged.
        connection.unsent = connection.unsent.min(connection.pending.len());
        connection.server_acked = acknowledgement;
    }
}

fn poll_connection(
    ap: &AccessPoint,
    connection: &mut Connection,
    now_ms: u64,
    frames: &mut Vec<Vec<u8>>,
    guest_mac: [u8; 6],
) -> bool {
    if connection.state == TcpState::Closed {
        return true;
    }
    if let Some(stream) = connection.stream.as_mut() {
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => {
                    connection.host_eof = true;
                    break;
                }
                Ok(count) => {
                    connection.pending.extend_from_slice(&buffer[..count]);
                    connection.unsent += count;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(_) => {
                    connection.stream = None;
                    connection.host_eof = true;
                    break;
                }
            }
        }
    } else if connection.state != TcpState::Closing {
        return true;
    }

    if connection.state == TcpState::SynReceived {
        return false;
    }

    if connection.unsent > 0 && connection.server_acked == connection.server_next {
        let start = connection.pending.len() - connection.unsent;
        let count = connection.unsent.min(TCP_MSS);
        let data = connection.pending[start..start + count].to_vec();
        frames.push(Network::tcp_segment(
            ap,
            guest_mac,
            connection,
            connection.server_next,
            true,
            false,
            false,
            &data,
        ));
        connection.server_next = connection.server_next.wrapping_add(count as u32);
        connection.unsent -= count;
        connection.retransmit_at = now_ms.saturating_add(TCP_RETRANSMIT_MS);
        return false;
    }

    if connection.server_next != connection.server_acked && now_ms >= connection.retransmit_at {
        let count = connection.pending.len().min(TCP_MSS);
        let data = connection.pending[..count].to_vec();
        frames.push(Network::tcp_segment(
            ap,
            guest_mac,
            connection,
            connection.server_acked,
            true,
            false,
            false,
            &data,
        ));
        connection.retransmit_at = now_ms.saturating_add(TCP_RETRANSMIT_MS);
        return false;
    }

    if connection.host_eof && connection.pending.is_empty() && !connection.host_fin_sent {
        connection.host_fin_sent = true;
        connection.server_next = connection.server_next.wrapping_add(1);
        connection.state = TcpState::FinWait;
        frames.push(Network::tcp_segment(
            ap,
            guest_mac,
            connection,
            connection.server_next.wrapping_sub(1),
            true,
            false,
            true,
            &[],
        ));
        return false;
    }
    false
}

fn ipv4(address: [u8; 4]) -> IpAddress {
    IpAddress::Ipv4(Ipv4Address::new(
        address[0], address[1], address[2], address[3],
    ))
}

struct DnsQuery {
    identifier: u16,
    flags: u16,
    name: Vec<u8>,
    kind: u16,
    question: Vec<u8>,
}

fn parse_dns_query(payload: &[u8]) -> Option<DnsQuery> {
    if payload.len() < 12 || u16::from_be_bytes([payload[4], payload[5]]) != 1 {
        return None;
    }
    let identifier = u16::from_be_bytes([payload[0], payload[1]]);
    let flags = u16::from_be_bytes([payload[2], payload[3]]);
    let mut offset = 12;
    let mut name = Vec::new();
    loop {
        let length = *payload.get(offset)? as usize;
        if length == 0 {
            offset += 1;
            break;
        }
        if length & 0xc0 != 0 || length > 63 || offset + 1 + length > payload.len() {
            return None;
        }
        if !name.is_empty() {
            name.push(b'.');
        }
        name.extend_from_slice(&payload[offset + 1..offset + 1 + length]);
        offset += 1 + length;
    }
    if offset + 4 > payload.len() {
        return None;
    }
    let kind = u16::from_be_bytes([payload[offset], payload[offset + 1]]);
    let class = u16::from_be_bytes([payload[offset + 2], payload[offset + 3]]);
    if class != 1 {
        return None;
    }
    Some(DnsQuery {
        identifier,
        flags,
        name,
        kind,
        question: payload[12..offset + 4].to_vec(),
    })
}

fn ntp_timestamp(seconds: u64, milliseconds: u32) -> [u8; 8] {
    let fraction = (u64::from(milliseconds) << 32) / 1000;
    let value = (seconds.saturating_add(NTP_UNIX_OFFSET_SECONDS) << 32) | fraction;
    value.to_be_bytes()
}

fn ethernet_frame(
    destination: &[u8; 6],
    source: &[u8; 6],
    ethertype: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut frame = vec![0u8; 14 + payload.len()];
    frame[..6].copy_from_slice(destination);
    frame[6..12].copy_from_slice(source);
    frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
    frame[14..].copy_from_slice(payload);
    frame
}

fn ipv4_frame(
    source_mac: &[u8; 6],
    destination_mac: [u8; 6],
    source: [u8; 4],
    destination: [u8; 4],
    protocol: IpProtocol,
    payload: &[u8],
) -> Vec<u8> {
    let mut ip = vec![0u8; 20 + payload.len()];
    {
        let mut packet = Ipv4Packet::new_unchecked(&mut ip[..]);
        packet.set_version(4);
        packet.set_header_len(20);
        packet.set_total_len((20 + payload.len()) as u16);
        packet.set_ident(0);
        packet.set_dont_frag(true);
        packet.set_hop_limit(64);
        packet.set_next_header(protocol);
        packet.set_src_addr(Ipv4Address::new(source[0], source[1], source[2], source[3]));
        packet.set_dst_addr(Ipv4Address::new(
            destination[0],
            destination[1],
            destination[2],
            destination[3],
        ));
        packet.payload_mut().copy_from_slice(payload);
        packet.fill_checksum();
    }
    ethernet_frame(&destination_mac, source_mac, 0x0800, &ip)
}

fn udp_frame(
    destination_mac: &[u8; 6],
    source_mac: &[u8; 6],
    source: [u8; 4],
    destination: [u8; 4],
    source_port: u16,
    destination_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut udp = vec![0u8; 8 + payload.len()];
    {
        let mut packet = UdpPacket::new_unchecked(&mut udp[..]);
        packet.set_src_port(source_port);
        packet.set_dst_port(destination_port);
        packet.set_len((8 + payload.len()) as u16);
        packet.payload_mut().copy_from_slice(payload);
        packet.fill_checksum(&ipv4(source), &ipv4(destination));
    }
    let mut ip = vec![0u8; 20 + udp.len()];
    {
        let total = ip.len() as u16;
        let mut packet = Ipv4Packet::new_unchecked(&mut ip[..]);
        packet.set_version(4);
        packet.set_header_len(20);
        packet.set_total_len(total);
        packet.set_ident(0);
        packet.set_dont_frag(true);
        packet.set_hop_limit(64);
        packet.set_next_header(IpProtocol::Udp);
        packet.set_src_addr(Ipv4Address::new(source[0], source[1], source[2], source[3]));
        packet.set_dst_addr(Ipv4Address::new(
            destination[0],
            destination[1],
            destination[2],
            destination[3],
        ));
        packet.payload_mut().copy_from_slice(&udp);
        packet.fill_checksum();
    }
    ethernet_frame(destination_mac, source_mac, 0x0800, &ip)
}

#[cfg(test)]
pub(crate) fn test_dhcp_discover(mac: [u8; 6]) -> Vec<u8> {
    let repr = DhcpRepr {
        message_type: DhcpMessageType::Discover,
        transaction_id: 0x12345678,
        secs: 0,
        client_hardware_address: EthernetAddress(mac),
        client_ip: Ipv4Address::UNSPECIFIED,
        your_ip: Ipv4Address::UNSPECIFIED,
        server_ip: Ipv4Address::UNSPECIFIED,
        router: None,
        subnet_mask: None,
        relay_agent_ip: Ipv4Address::UNSPECIFIED,
        broadcast: true,
        requested_ip: None,
        client_identifier: None,
        server_identifier: None,
        parameter_request_list: None,
        dns_servers: None,
        max_size: None,
        lease_duration: None,
        renew_duration: None,
        rebind_duration: None,
        additional_options: &[],
    };
    let mut payload = vec![0u8; repr.buffer_len()];
    let mut packet = DhcpPacket::new_unchecked(&mut payload[..]);
    repr.emit(&mut packet).unwrap();
    udp_frame(
        &[255; 6],
        &mac,
        [0; 4],
        [255, 255, 255, 255],
        68,
        67,
        &payload,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap() -> AccessPoint {
        AccessPoint {
            ssid: "Camera OTA".into(),
            bssid: [2, 0, 0, 0, 0, 1],
            channel: 6,
            rcpi: 180,
            gateway: [192, 168, 4, 1],
            dns_servers: [[192, 168, 4, 1], [192, 168, 4, 2]],
            hostname: "ota.test".into(),
            host: [203, 0, 113, 10],
            host_port: 443,
            local_port: 8080,
            wpa2_passphrase: None,
            ntp: None,
        }
    }

    fn ap_with_ntp() -> AccessPoint {
        let mut ap = ap();
        ap.ntp = Some(Ntp {
            hostname: "time.test".into(),
            address: [192, 168, 4, 1],
            seconds: 1_791_331_200,
        });
        ap
    }

    fn dhcp_request(kind: DhcpMessageType, mac: [u8; 6]) -> Vec<u8> {
        let repr = DhcpRepr {
            message_type: kind,
            transaction_id: 0x12345678,
            secs: 0,
            client_hardware_address: EthernetAddress(mac),
            client_ip: Ipv4Address::UNSPECIFIED,
            your_ip: Ipv4Address::UNSPECIFIED,
            server_ip: Ipv4Address::UNSPECIFIED,
            router: None,
            subnet_mask: None,
            relay_agent_ip: Ipv4Address::UNSPECIFIED,
            broadcast: true,
            requested_ip: None,
            client_identifier: None,
            server_identifier: None,
            parameter_request_list: None,
            dns_servers: None,
            max_size: None,
            lease_duration: None,
            renew_duration: None,
            rebind_duration: None,
            additional_options: &[],
        };
        let mut payload = vec![0u8; repr.buffer_len()];
        let mut packet = DhcpPacket::new_unchecked(&mut payload[..]);
        repr.emit(&mut packet).unwrap();
        udp_frame(
            &[255; 6],
            &mac,
            [0; 4],
            [255, 255, 255, 255],
            68,
            67,
            &payload,
        )
    }

    fn dns_query(name: &str) -> Vec<u8> {
        let mut question = Vec::new();
        for label in name.split('.') {
            question.push(label.len() as u8);
            question.extend_from_slice(label.as_bytes());
        }
        question.push(0);
        question.extend_from_slice(&1u16.to_be_bytes());
        question.extend_from_slice(&1u16.to_be_bytes());
        let mut payload = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        payload.extend_from_slice(&question);
        udp_frame(
            &[2, 0, 0, 0, 0, 2],
            &[2, 0, 0, 0, 0, 1],
            [192, 168, 4, 100],
            [192, 168, 4, 1],
            49152,
            53,
            &payload,
        )
    }

    fn ntp_query_to(destination: [u8; 4], lvm: u8, transmit: [u8; 8]) -> Vec<u8> {
        let mut payload = vec![0u8; 48];
        payload[0] = lvm;
        payload[40..48].copy_from_slice(&transmit);
        udp_frame(
            &[2, 0, 0, 0, 0, 2],
            &[2, 0, 0, 0, 0, 1],
            [192, 168, 4, 100],
            destination,
            49153,
            123,
            &payload,
        )
    }

    fn ntp_query(transmit: [u8; 8]) -> Vec<u8> {
        ntp_query_to([192, 168, 4, 1], (4 << 3) | 3, transmit)
    }

    fn arp_request(ip: [u8; 4], mac: [u8; 6]) -> Vec<u8> {
        let mut arp = vec![0u8; 28];
        arp[..2].copy_from_slice(&[0, 1]);
        arp[2..4].copy_from_slice(&[8, 0]);
        arp[4] = 6;
        arp[5] = 4;
        arp[6..8].copy_from_slice(&1u16.to_be_bytes());
        arp[8..14].copy_from_slice(&mac);
        arp[14..18].copy_from_slice(&[192, 168, 4, 100]);
        arp[18..24].copy_from_slice(&[0; 6]);
        arp[24..28].copy_from_slice(&ip);
        ethernet_frame(&[255; 6], &mac, 0x0806, &arp)
    }

    #[test]
    fn arp_replies_for_every_local_address() {
        let ap = ap();
        let mut network = Network::default();
        let mac = [2, 0, 0, 0, 0, 9];
        for address in [ap.gateway, ap.dns_servers[0], ap.dns_servers[1], ap.host] {
            let replies = network.receive(&ap, &arp_request(address, mac), 0);
            assert_eq!(replies.len(), 1);
            let ethernet = EthernetFrame::new_checked(&replies[0]).unwrap();
            assert_eq!(ethernet.ethertype(), smoltcp::wire::EthernetProtocol::Arp);
            assert_eq!(ethernet.dst_addr().0, mac);
            assert_eq!(&ethernet.payload()[8..14], &ap.bssid);
            assert_eq!(&ethernet.payload()[14..18], &address);
            assert_eq!(&ethernet.payload()[24..28], &[192, 168, 4, 100]);
        }
    }

    #[test]
    fn dhcp_offer_and_ack_use_the_configured_lan() {
        let ap = ap();
        let mut network = Network::default();
        let mac = [2, 0, 0, 0, 0, 9];
        let offer = network.receive(&ap, &dhcp_request(DhcpMessageType::Discover, mac), 10);
        assert_eq!(offer.len(), 1);
        let offer = EthernetFrame::new_checked(&offer[0]).unwrap();
        assert_eq!(offer.dst_addr(), EthernetAddress::BROADCAST);
        let ip = Ipv4Packet::new_checked(offer.payload()).unwrap();
        let udp = UdpPacket::new_checked(ip.payload()).unwrap();
        assert_eq!(udp.src_port(), 67);
        assert_eq!(udp.dst_port(), 68);
        let packet = DhcpPacket::new_checked(udp.payload()).unwrap();
        let reply = DhcpRepr::parse(&packet).unwrap();
        assert_eq!(reply.message_type, DhcpMessageType::Offer);
        assert_eq!(reply.your_ip, Ipv4Address::new(192, 168, 4, 100));
        assert_eq!(reply.server_ip, Ipv4Address::new(192, 168, 4, 1));
        assert_eq!(reply.dns_servers.unwrap().len(), 2);
        assert_eq!(network.lease().unwrap().address, [192, 168, 4, 100]);
        let ack = network.receive(&ap, &dhcp_request(DhcpMessageType::Request, mac), 20);
        assert_eq!(ack.len(), 1);
        let ack = EthernetFrame::new_checked(&ack[0]).unwrap();
        let ip = Ipv4Packet::new_checked(ack.payload()).unwrap();
        let udp = UdpPacket::new_checked(ip.payload()).unwrap();
        let packet = DhcpPacket::new_checked(udp.payload()).unwrap();
        let reply = DhcpRepr::parse(&packet).unwrap();
        assert_eq!(reply.message_type, DhcpMessageType::Ack);
    }

    #[test]
    fn dns_answers_the_configured_name() {
        let ap = ap();
        let mut network = Network {
            guest_mac: Some([2, 0, 0, 0, 0, 2]),
            ..Default::default()
        };
        let reply = network.receive(&ap, &dns_query("ota.test"), 0);
        assert_eq!(reply.len(), 1);
        let ethernet = EthernetFrame::new_checked(&reply[0]).unwrap();
        let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        let udp = UdpPacket::new_checked(ip.payload()).unwrap();
        let answer = udp.payload();
        assert_eq!(u16::from_be_bytes([answer[6], answer[7]]), 1);
        assert_eq!(&answer[answer.len() - 4..], &[203, 0, 113, 10]);
        let missing = network.receive(&ap, &dns_query("missing.test"), 0);
        let ethernet = EthernetFrame::new_checked(&missing[0]).unwrap();
        let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        let udp = UdpPacket::new_checked(ip.payload()).unwrap();
        let answer = udp.payload();
        assert_eq!(u16::from_be_bytes([answer[6], answer[7]]), 0);
        assert_eq!(u16::from_be_bytes([answer[2], answer[3]]) & 15, 3);
    }

    #[test]
    fn dns_and_ntp_answer_the_configured_time_server() {
        let ap = ap_with_ntp();
        let mut network = Network {
            guest_mac: Some([2, 0, 0, 0, 0, 2]),
            ..Default::default()
        };
        let lookup = network.receive(&ap, &dns_query("time.test"), 0);
        assert_eq!(lookup.len(), 1);
        let ethernet = EthernetFrame::new_checked(&lookup[0]).unwrap();
        let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        let udp = UdpPacket::new_checked(ip.payload()).unwrap();
        let answer = udp.payload();
        assert_eq!(u16::from_be_bytes([answer[6], answer[7]]), 1);
        assert_eq!(&answer[answer.len() - 4..], &[192, 168, 4, 1]);

        let transmit = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let reply = network.receive(&ap, &ntp_query(transmit), 0);
        assert_eq!(reply.len(), 1);
        let ethernet = EthernetFrame::new_checked(&reply[0]).unwrap();
        let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        let udp = UdpPacket::new_checked(ip.payload()).unwrap();
        assert_eq!(udp.src_port(), 123);
        assert_eq!(udp.dst_port(), 49153);
        let answer = udp.payload();
        assert_eq!(answer.len(), 48);
        assert_eq!(answer[0], (4 << 3) | 4);
        assert_eq!(answer[1], 2);
        assert_eq!(&answer[24..32], &transmit);
        let expected = (1_791_331_200u64 + NTP_UNIX_OFFSET_SECONDS) << 32;
        assert_eq!(&answer[32..40], &expected.to_be_bytes());
        assert_eq!(&answer[40..48], &expected.to_be_bytes());

        let arp = network.receive(&ap, &arp_request([192, 168, 4, 1], [2, 0, 0, 0, 0, 9]), 0);
        assert_eq!(arp.len(), 1);
        let ethernet = EthernetFrame::new_checked(&arp[0]).unwrap();
        assert_eq!(ethernet.ethertype(), smoltcp::wire::EthernetProtocol::Arp);
    }

    #[test]
    fn ntp_ignores_responses_other_addresses_and_missing_servers() {
        let ap = ap_with_ntp();
        let mut network = Network {
            guest_mac: Some([2, 0, 0, 0, 0, 2]),
            ..Default::default()
        };
        let response = ntp_query_to([192, 168, 4, 1], (4 << 3) | 4, [1; 8]);
        assert!(network.receive(&ap, &response, 0).is_empty());
        let elsewhere = ntp_query_to([192, 168, 4, 2], (4 << 3) | 3, [1; 8]);
        assert!(network.receive(&ap, &elsewhere, 0).is_empty());
        let mut missing = ap.clone();
        missing.ntp = None;
        assert!(network.receive(&missing, &ntp_query([1; 8]), 0).is_empty());
    }

    fn tcp_packet(
        source_port: u16,
        destination_port: u16,
        seq: u32,
        ack: u32,
        flags: (bool, bool, bool, bool),
        payload: &[u8],
    ) -> Vec<u8> {
        let mut tcp = vec![0u8; 20 + payload.len()];
        {
            let mut packet = TcpPacket::new_unchecked(&mut tcp[..]);
            packet.set_src_port(source_port);
            packet.set_dst_port(destination_port);
            packet.set_seq_number(smoltcp::wire::TcpSeqNumber(seq as i32));
            packet.set_ack_number(smoltcp::wire::TcpSeqNumber(ack as i32));
            packet.set_header_len(20);
            packet.set_window_len(TCP_WINDOW);
            packet.set_syn(flags.0);
            packet.set_ack(flags.1);
            packet.set_fin(flags.2);
            packet.set_rst(flags.3);
            packet.payload_mut().copy_from_slice(payload);
            packet.fill_checksum(&ipv4([192, 168, 4, 100]), &ipv4([203, 0, 113, 10]));
        }
        let mut ip = vec![0u8; 20 + tcp.len()];
        {
            let total = ip.len() as u16;
            let mut packet = Ipv4Packet::new_unchecked(&mut ip[..]);
            packet.set_version(4);
            packet.set_header_len(20);
            packet.set_total_len(total);
            packet.set_dont_frag(true);
            packet.set_hop_limit(64);
            packet.set_next_header(IpProtocol::Tcp);
            packet.set_src_addr(Ipv4Address::new(192, 168, 4, 100));
            packet.set_dst_addr(Ipv4Address::new(203, 0, 113, 10));
            packet.payload_mut().copy_from_slice(&tcp);
            packet.fill_checksum();
        }
        ethernet_frame(&[2, 0, 0, 0, 0, 1], &[2, 0, 0, 0, 0, 2], 0x0800, &ip)
    }

    #[test]
    fn tcp_relay_reaches_a_local_listener_and_returns_a_segment() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut ap = ap();
        ap.local_port = port;
        let mut network = Network {
            guest_mac: Some([2, 0, 0, 0, 0, 2]),
            ..Default::default()
        };
        let syn = tcp_packet(40000, 443, 1000, 0, (true, false, false, false), &[]);
        let frames = network.receive(&ap, &syn, 0);
        assert_eq!(frames.len(), 1);
        let ethernet = EthernetFrame::new_checked(&frames[0]).unwrap();
        let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
        assert!(tcp.syn() && tcp.ack());
        assert_eq!(tcp.ack_number().0, 1001);
        let server_sequence = tcp.seq_number().0 as u32;
        let ack = tcp_packet(
            40000,
            443,
            1001,
            server_sequence.wrapping_add(1),
            (false, true, false, false),
            &[],
        );
        network.receive(&ap, &ack, 1);
        let (mut host, _) = listener.accept().unwrap();
        host.write_all(b"hello").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let frames = network.poll(&ap, 2);
        assert!(!frames.is_empty());
        let ethernet = EthernetFrame::new_checked(&frames[0]).unwrap();
        let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
        assert_eq!(tcp.payload(), b"hello");
    }

    #[test]
    fn tcp_relay_forwards_guest_payload_to_the_host_listener() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut ap = ap();
        ap.local_port = port;
        let mut network = Network {
            guest_mac: Some([2, 0, 0, 0, 0, 2]),
            ..Default::default()
        };
        let syn = tcp_packet(40001, 443, 2000, 0, (true, false, false, false), &[]);
        let frames = network.receive(&ap, &syn, 0);
        let ethernet = EthernetFrame::new_checked(&frames[0]).unwrap();
        let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
        let server_sequence = tcp.seq_number().0 as u32;
        let ack = tcp_packet(
            40001,
            443,
            2001,
            server_sequence + 1,
            (false, true, false, false),
            &[],
        );
        network.receive(&ap, &ack, 1);
        let (mut host, _) = listener.accept().unwrap();
        let request = b"GET / HTTP/1.0\r\n\r\n";
        let data = tcp_packet(
            40001,
            443,
            2001,
            server_sequence + 1,
            (false, true, false, false),
            request,
        );
        let frames = network.receive(&ap, &data, 2);
        assert!(!frames.is_empty());
        host.set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let mut received = [0u8; 64];
        let count = host.read(&mut received).unwrap();
        assert_eq!(&received[..count], request);
    }
}
