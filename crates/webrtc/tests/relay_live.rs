//! Relay-only share through the live share service and its TURN relays.
//!
//! Ignored by default: it contacts the public service, creates a temporary
//! room and ends it. The owner echoes bytes.
//!
//! `FUJIMOD_WEBRTC_LIVE_SIGNAL=wss://share.fujimod.com/usb-proxy/signal \
//!  cargo test -p fujimod-webrtc --features signaling --test relay_live -- --ignored --nocapture`
use fujimod_webrtc::{
    CandidatePolicy, HostEvent, HostRoom, Pacing, PeerSocket, share::OwnerTicket,
};
use std::{
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

#[test]
#[ignore = "contacts the live share service and its Cloudflare TURN relays"]
fn relay_only_share_through_live_turn() {
    let signal = std::env::var("FUJIMOD_WEBRTC_LIVE_SIGNAL")
        .unwrap_or_else(|_| "wss://share.fujimod.com/usb-proxy/signal".into());
    let site =
        std::env::var("FUJIMOD_WEBRTC_LIVE_SITE").unwrap_or_else(|_| "https://fujimod.com".into());
    futures_lite::future::block_on(async {
        let owner = OwnerTicket::create(signal, site).unwrap();
        let host = HostRoom::join(
            &owner.link,
            &owner.owner_key,
            CandidatePolicy::RelayOnly,
            Pacing::Browser,
        )
        .await
        .unwrap();
        let echo = thread::spawn(move || {
            futures_lite::future::block_on(async move {
                let HostEvent::Controller(_, mut peer) = host.next_event().await.unwrap() else {
                    panic!("expected a controller");
                };
                let data = peer.receive().await.unwrap();
                peer.send(data).await.unwrap();
                (host, peer)
            })
        });

        let started = Instant::now();
        let steps = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = steps.clone();
        let progress: fujimod_webrtc::Progress = Arc::new(move |step: &str| {
            println!("{:6.2}s  {step}", started.elapsed().as_secs_f64());
            log.lock().unwrap().push(step.to_owned());
        });
        let mut controller = PeerSocket::connect(
            &owner.link,
            &progress,
            CandidatePolicy::RelayOnly,
            Pacing::Browser,
        )
        .await
        .unwrap();
        let payload: Vec<u8> = (0..256 * 1024).map(|index| (index * 31) as u8).collect();
        controller.send(payload.clone()).await.unwrap();
        assert_eq!(controller.receive().await.unwrap(), payload);
        println!(
            "ECHO OK: {} bytes round-tripped through relays only",
            payload.len()
        );
        assert!(
            steps
                .lock()
                .unwrap()
                .iter()
                .any(|step| step == "TURN relay candidate ready")
        );

        drop(controller);
        let (host, peer) = echo.join().unwrap();
        drop(peer);
        host.end().await.unwrap();
        println!("room ended");
    });
}
