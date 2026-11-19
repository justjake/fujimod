//! Signaling connections with Happy Eyeballs (RFC 8305): every resolved
//! address is tried in turn, alternating IPv6 and IPv4, with the next attempt
//! starting `STAGGER` after the previous one so a black-holed IPv6 route costs
//! a fraction of a second instead of the operating system's TCP timeout.
use crate::{Error, Result};
use async_io::{Async, Timer};
use async_tungstenite::{
    WebSocketStream,
    smol::{ClientStream, client_async_tls},
};
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    time::Duration,
};

const STAGGER: Duration = Duration::from_millis(250);
const ATTEMPT: Duration = Duration::from_secs(10);

fn error(message: impl ToString) -> Error {
    Error(message.to_string())
}

/// Addresses alternating between the families, IPv6 first.
fn interleave(addresses: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let (mut v6, mut v4): (Vec<_>, Vec<_>) = addresses.into_iter().partition(SocketAddr::is_ipv6);
    v6.reverse();
    v4.reverse();
    let mut order = Vec::new();
    while !(v6.is_empty() && v4.is_empty()) {
        order.extend(v6.pop());
        order.extend(v4.pop());
    }
    order
}

async fn resolve(host: String, port: u16) -> Result<Vec<SocketAddr>> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(
            (host.as_str(), port)
                .to_socket_addrs()
                .map(Iterator::collect),
        );
    });
    let addresses: Vec<SocketAddr> = rx.recv().await.map_err(error)?.map_err(error)?;
    Ok(interleave(addresses))
}

async fn happy_eyeballs(addresses: Vec<SocketAddr>) -> Result<Async<TcpStream>> {
    let mut attempts: FuturesUnordered<_> = addresses
        .into_iter()
        .enumerate()
        .map(|(index, address)| async move {
            Timer::after(STAGGER * index as u32).await;
            futures_lite::future::or(
                async { Async::<TcpStream>::connect(address).await },
                async {
                    Timer::after(ATTEMPT).await;
                    Err(std::io::ErrorKind::TimedOut.into())
                },
            )
            .await
            .map_err(|failure| format!("{address}: {failure}"))
        })
        .collect();
    let mut failures = Vec::new();
    while let Some(result) = attempts.next().await {
        match result {
            Ok(stream) => return Ok(stream),
            Err(failure) => failures.push(failure),
        }
    }
    Err(error(format!(
        "signaling connection failed: {}",
        failures.join("; ")
    )))
}

pub(crate) type Signal = WebSocketStream<ClientStream<Async<TcpStream>>>;

/// Open the signaling WebSocket at `url`.
pub(crate) async fn connect_signaling(url: &str) -> Result<Signal> {
    let parsed = url::Url::parse(url).map_err(error)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| error("signaling URL has no host"))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| error("signaling URL has no port"))?;
    let stream = happy_eyeballs(resolve(host.to_owned(), port).await?).await?;
    let (socket, _) = client_async_tls(url, stream).await.map_err(error)?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_alternate_with_ipv6_first() {
        let a = |text: &str| text.parse::<SocketAddr>().unwrap();
        let order = interleave(vec![
            a("1.1.1.1:1"),
            a("1.1.1.2:1"),
            a("[::1]:1"),
            a("[::2]:1"),
            a("[::3]:1"),
        ]);
        assert_eq!(
            order,
            [
                a("[::1]:1"),
                a("1.1.1.1:1"),
                a("[::2]:1"),
                a("1.1.1.2:1"),
                a("[::3]:1")
            ]
        );
    }

    #[test]
    fn dead_first_address_does_not_block_a_working_one() {
        // 192.0.2.1 is reserved documentation space: connecting hangs or fails.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let good = listener.local_addr().unwrap();
        let started = std::time::Instant::now();
        let stream = futures_lite::future::block_on(happy_eyeballs(vec![
            "192.0.2.1:9".parse().unwrap(),
            good,
        ]))
        .unwrap();
        assert_eq!(stream.get_ref().peer_addr().unwrap(), good);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
