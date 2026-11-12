//! The smol runtime, with UDP socket queues sized for bulk transfer and
//! optional output pacing for browser peers.
use super::Pacing;
use std::{
    future::Future,
    io::{self, IoSliceMut},
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use webrtc::runtime::{
    AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, RecvMeta, Runtime,
    SmolRuntime, Transmit,
};

/// Socket queue size requested for each peer UDP socket. SCTP may have a full
/// 1 MiB receive window in flight; the queue must also hold its DTLS overhead
/// and scheduling bursts, or a burst drops datagrams and stalls on
/// retransmission timeouts. The OS may clamp the request.
const SOCKET_BUFFER: usize = 4 * 1024 * 1024;

type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// `Pacing::Browser` sends at most this many bytes per burst at `PACED_RATE`.
const PACED_BURST: usize = 8 * 1024;
const PACED_RATE: f64 = 50_000_000.0;

#[derive(Debug)]
pub(super) struct PeerRuntime {
    smol: SmolRuntime,
    pacing: Pacing,
}

impl PeerRuntime {
    pub fn shared(pacing: Pacing) -> Arc<dyn Runtime> {
        Arc::new(Self {
            smol: SmolRuntime,
            pacing,
        })
    }
}

struct Burst {
    started: Instant,
    bytes: usize,
    wait: Option<async_io::Timer>,
}

/// A UDP socket that sends in `PACED_BURST` bursts at `PACED_RATE`.
struct PacedSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    burst: Mutex<Burst>,
}

impl std::fmt::Debug for PacedSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PacedSocket").field(&self.inner).finish()
    }
}

impl AsyncUdpSocket for PacedSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn poll_send(&self, cx: &mut Context<'_>, transmit: &Transmit<'_>) -> Poll<io::Result<usize>> {
        let mut burst = self.burst.lock().unwrap();
        if burst.bytes >= PACED_BURST {
            let due = burst.started + Duration::from_secs_f64(burst.bytes as f64 / PACED_RATE);
            let now = Instant::now();
            if due > now {
                let wait = burst.wait.get_or_insert_with(|| async_io::Timer::at(due));
                if Pin::new(wait).poll(cx).is_pending() {
                    return Poll::Pending;
                }
            }
            burst.wait = None;
            burst.started = Instant::now();
            burst.bytes = 0;
        }
        let sent = self.inner.poll_send(cx, transmit);
        if let Poll::Ready(Ok(_)) = sent {
            burst.bytes += transmit.contents.len();
        }
        sent
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.inner.poll_recv(cx, bufs, meta)
    }
    fn max_gro_segments(&self) -> usize {
        self.inner.max_gro_segments()
    }
}

impl Runtime for PeerRuntime {
    fn spawn(&self, future: Boxed<'static, ()>) -> Box<dyn JoinHandle> {
        self.smol.spawn(future)
    }
    fn spawn_reactor(&self, pool_size: usize, future: Boxed<'static, ()>) -> Box<dyn JoinHandle> {
        self.smol.spawn_reactor(pool_size, future)
    }
    fn wrap_udp_socket(&self, socket: std::net::UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        let reference = socket2::SockRef::from(&socket);
        reference.set_recv_buffer_size(SOCKET_BUFFER)?;
        reference.set_send_buffer_size(SOCKET_BUFFER)?;
        let inner = self.smol.wrap_udp_socket(socket)?;
        Ok(match self.pacing {
            Pacing::Unpaced => inner,
            Pacing::Browser => Arc::new(PacedSocket {
                inner,
                burst: Mutex::new(Burst {
                    started: Instant::now(),
                    bytes: 0,
                    wait: None,
                }),
            }),
        })
    }
    fn wrap_tcp_listener(
        &self,
        listener: std::net::TcpListener,
    ) -> io::Result<Arc<dyn AsyncTcpListener>> {
        self.smol.wrap_tcp_listener(listener)
    }
    fn connect_tcp<'a>(
        &'a self,
        remote_addr: SocketAddr,
    ) -> Boxed<'a, io::Result<Arc<dyn AsyncTcpStream>>> {
        self.smol.connect_tcp(remote_addr)
    }
    fn resolve_host<'a>(&'a self, host: &'a str) -> Boxed<'a, io::Result<Vec<SocketAddr>>> {
        self.smol.resolve_host(host)
    }
    fn sleep(&self, duration: Duration) -> Boxed<'static, ()> {
        self.smol.sleep(duration)
    }
    fn interval(&self, period: Duration) -> Box<dyn AsyncInterval> {
        self.smol.interval(period)
    }
    fn block_on(&self, future: Pin<Box<dyn Future<Output = ()> + '_>>) {
        self.smol.block_on(future)
    }
    fn name(&self) -> &'static str {
        "fujimod-smol"
    }
}
