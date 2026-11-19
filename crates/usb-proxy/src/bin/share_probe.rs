//! Check a share link's WebRTC bridge without touching USB.
//!
//! `fujimod-share-probe [--relay-only] URL` (or FUJIMOD_USB_PROXY_URL) prints
//! each connection step with elapsed time, then stops after the proxy
//! handshake. It sends no USB request, not even a device listing.
//! `--relay-only` connects through a TURN relay alone, which shows whether this
//! network can reach the share when no direct path works.
use fujimod_webrtc::CandidatePolicy;
use std::time::Instant;

fn main() {
    let mut policy = CandidatePolicy::Any;
    let mut url = None;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--relay-only" => policy = CandidatePolicy::RelayOnly,
            _ => url = Some(argument),
        }
    }
    let url = url
        .or_else(|| std::env::var("FUJIMOD_USB_PROXY_URL").ok())
        .expect("give the share link as an argument or FUJIMOD_USB_PROXY_URL");
    let started = Instant::now();
    let progress: fujimod_webrtc::Progress = std::sync::Arc::new(move |step: &str| {
        println!("{:6.2}s  {step}", started.elapsed().as_secs_f64());
    });
    match futures_lite::future::block_on(fujimod_usb_proxy::Client::connect_with_policy(
        &url, "", progress, policy,
    )) {
        Ok(_) => println!("BRIDGE READY: WebRTC and proxy handshake work; no USB request sent"),
        Err(error) => {
            println!("BRIDGE FAILED: {error}");
            std::process::exit(1);
        }
    }
}
