//! Bound interface sockets preserve the actual ICE source/destination tuple.
//! Receive buffers stay with their socket across cancelable waits.
use futures_util::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

struct BoundSocket {
    local: SocketAddr,
    udp: async_net::UdpSocket,
}
type Socket = Arc<BoundSocket>;
type Pending = BoxFuture<'static, (Socket, Vec<u8>, io::Result<(usize, SocketAddr)>)>;

fn receive(socket: Socket, mut bytes: Vec<u8>) -> Pending {
    async move {
        let result = socket.udp.recv_from(&mut bytes).await;
        (socket, bytes, result)
    }
    .boxed()
}

pub(super) struct Datagram {
    socket: Socket,
    pub local: SocketAddr,
    pub remote: SocketAddr,
    pub bytes: Vec<u8>,
    pub size: usize,
}

pub(super) struct DatagramIo {
    sockets: HashMap<SocketAddr, Socket>,
    pending: FuturesUnordered<Pending>,
    burst_started: Instant,
    burst_bytes: usize,
}
impl DatagramIo {
    pub fn new(sockets: Vec<std::net::UdpSocket>) -> io::Result<Self> {
        let mut result = Self {
            sockets: HashMap::new(),
            pending: FuturesUnordered::new(),
            burst_started: Instant::now(),
            burst_bytes: 0,
        };
        for socket in sockets {
            let local = socket.local_addr()?;
            // SCTP may have a full 1 MiB receive window in flight. The UDP
            // queue must also hold its DTLS overhead and scheduling bursts.
            socket2::SockRef::from(&socket).set_recv_buffer_size(4 * 1024 * 1024)?;
            socket.set_nonblocking(true)?;
            let socket = Arc::new(BoundSocket {
                local,
                udp: async_net::UdpSocket::from(async_io::Async::new(socket)?),
            });
            result.sockets.insert(local, socket.clone());
            result.pending.push(receive(socket, vec![0; 65536]));
        }
        if result.sockets.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "no UDP interfaces",
            ));
        }
        Ok(result)
    }

    pub async fn send(
        &mut self,
        source: SocketAddr,
        destination: SocketAddr,
        bytes: &[u8],
    ) -> io::Result<()> {
        // Bound host bursts as well as SCTP's congestion window. A fast sender
        // can otherwise overflow a browser's UDP socket before its network
        // thread runs. 8 KiB bursts at 50 MB/s leave headroom above the
        // 20 MB/s USB transport target without flooding the receive queue.
        if self.burst_bytes >= 8 * 1024 {
            let deadline = self.burst_started
                + Duration::from_secs_f64(self.burst_bytes as f64 / 50_000_000.0);
            if deadline > Instant::now() {
                async_io::Timer::at(deadline).await;
            }
            self.burst_started = Instant::now();
            self.burst_bytes = 0;
        }
        self.burst_bytes += bytes.len();
        let socket = self.sockets.get(&source).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "ICE selected an unbound UDP source",
            )
        })?;
        let size = socket.udp.send_to(bytes, destination).await?;
        if size != bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short UDP datagram",
            ));
        }
        Ok(())
    }

    pub async fn receive(&mut self) -> io::Result<Datagram> {
        let (socket, bytes, result) = self.pending.next().await.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "no pending UDP receivers")
        })?;
        let (size, remote) = result?;
        Ok(Datagram {
            local: socket.local,
            socket,
            remote,
            bytes,
            size,
        })
    }

    pub fn rearm(&mut self, datagram: Datagram) {
        self.pending.push(receive(datagram.socket, datagram.bytes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interface_tuples_buffers_and_canceled_waits() {
        futures_lite::future::block_on(async {
            let first = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            let second = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            let addresses = [first.local_addr().unwrap(), second.local_addr().unwrap()];
            let mut io = DatagramIo::new(vec![first, second]).unwrap();
            let peer = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let remote = peer.local_addr().unwrap();
            for _ in 0..2 {
                assert!(
                    futures_lite::future::poll_once(io.receive())
                        .await
                        .is_none()
                );
                for (index, address) in addresses.iter().enumerate() {
                    peer.send_to(&[index as u8], address).unwrap();
                }
                let mut seen = Vec::new();
                for _ in 0..2 {
                    let packet = futures_lite::future::or(io.receive(), async {
                        futures_timer::Delay::new(std::time::Duration::from_secs(2)).await;
                        Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "UDP test timed out",
                        ))
                    })
                    .await
                    .unwrap();
                    let index = packet.bytes[0] as usize;
                    assert_eq!(packet.local, addresses[index]);
                    assert_eq!(packet.remote, remote);
                    assert_eq!(packet.size, 1);
                    io.send(packet.local, remote, &[index as u8]).await.unwrap();
                    let mut response = [0; 1];
                    let (size, source) = peer.recv_from(&mut response).unwrap();
                    assert_eq!(source, packet.local);
                    assert_eq!(size, 1);
                    assert_eq!(response[0] as usize, index);
                    seen.push(index);
                    io.rearm(packet);
                }
                seen.sort();
                assert_eq!(seen, [0, 1]);
            }
            assert!(io.send(remote, remote, &[]).await.is_err());
        });
    }
}
