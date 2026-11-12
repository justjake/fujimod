//! Native-to-native data channel throughput on this host.
//!
//! `fujimod-webrtc-bench [MIB] [MESSAGE_KIB] [CONNECTIONS]` starts an in-process signaling
//! service, joins a direct-only room as owner and CONNECTIONS controllers, sends MIB
//! mebibytes split across the controllers to the owner in MESSAGE_KIB messages and
//! prints MB/s (decimal) and the process CPU time. See README.md.
use fujimod_webrtc::{
    CandidatePolicy, HostEvent, HostRoom, Pacing, PeerSocket, share::OwnerTicket,
};
use std::time::Instant;

fn cpu_seconds() -> f64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    let text = String::from_utf8_lossy(&output.stdout);
    let mut seconds = 0.0;
    for part in text.trim().split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().unwrap_or(0.0);
    }
    seconds
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mib: usize = args.next().map_or(1024, |v| v.parse().expect("MIB"));
    let message = args
        .next()
        .map_or(1024, |v| v.parse::<usize>().expect("MESSAGE_KIB"))
        * 1024;
    let connections: usize = args.next().map_or(1, |v| v.parse().expect("CONNECTIONS"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, fujimod_webrtc::signaling::router())
            .await
            .unwrap();
    });
    let owner = OwnerTicket::create(
        format!("ws://{address}/usb-proxy/signal"),
        "https://bench.invalid",
    )
    .unwrap();
    let link = owner.link.clone();
    let total = mib * 1024 * 1024;
    let share = total / connections;
    let (joined, owner_present) = async_channel::bounded(1);
    let receiver = tokio::task::spawn_blocking(move || {
        futures_lite::future::block_on(async move {
            let room = HostRoom::join(
                &owner.link,
                &owner.owner_key,
                CandidatePolicy::DirectOnly,
                Pacing::Unpaced,
            )
            .await
            .unwrap();
            joined.send(()).await.unwrap();
            let mut threads = Vec::new();
            while threads.len() < connections {
                let HostEvent::Controller(_, mut peer) = room.next_event().await.unwrap() else {
                    continue;
                };
                threads.push(std::thread::spawn(move || {
                    futures_lite::future::block_on(async move {
                        let mut received = 0;
                        while received < share {
                            received += peer.receive().await.unwrap().len();
                        }
                        peer.send(vec![1]).await.unwrap();
                        peer
                    })
                }));
            }
            let peers: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            (room, peers)
        })
    });
    owner_present.recv().await.unwrap();
    let mut peers = Vec::new();
    for _ in 0..connections {
        let progress: fujimod_webrtc::Progress = std::sync::Arc::new(|_: &str| {});
        peers.push(
            PeerSocket::connect(
                &link,
                &progress,
                CandidatePolicy::DirectOnly,
                Pacing::Unpaced,
            )
            .await
            .unwrap(),
        );
    }
    let cpu = cpu_seconds();
    let started = Instant::now();
    let senders: Vec<_> = peers
        .into_iter()
        .map(|mut peer| {
            std::thread::spawn(move || {
                futures_lite::future::block_on(async move {
                    let payload = vec![0x5a; message];
                    let mut sent = 0;
                    while sent < share {
                        let length = message.min(share - sent);
                        peer.send(payload[..length].to_vec()).await.unwrap();
                        sent += length;
                    }
                    peer.receive().await.unwrap();
                    peer
                })
            })
        })
        .collect();
    let peers: Vec<_> = senders.into_iter().map(|t| t.join().unwrap()).collect();
    let (elapsed, cpu) = (started.elapsed(), cpu_seconds() - cpu);
    // End the room only after every sender has its acknowledgment.
    let (room, owner_peers) = receiver.await.unwrap();
    room.end().await.unwrap();
    drop((peers, owner_peers));
    let seconds = elapsed.as_secs_f64();
    println!(
        "{mib} MiB over {connections} connection(s) in {} KiB messages: {seconds:.2} s, {:.1} MB/s, {:.2} CPU s ({:.0}% of one core)",
        message / 1024,
        total as f64 / seconds / 1e6,
        cpu,
        cpu / seconds * 100.0
    );
}
