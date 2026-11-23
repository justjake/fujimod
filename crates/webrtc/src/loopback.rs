//! A share room on this machine: the signaling service on a loopback port and
//! an owner that offers only direct candidates. Controllers on the same
//! machine join through [`LoopbackRoom::link`] exactly as they join any share
//! room; no request leaves the machine.
use crate::{CandidatePolicy, Error, HostRoom, Pacing, Result, share::Ticket};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngExt;

pub struct LoopbackRoom {
    link: String,
    room: HostRoom,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    server: Option<std::thread::JoinHandle<()>>,
}

fn error(value: impl std::fmt::Display) -> Error {
    Error(value.to_string())
}

impl LoopbackRoom {
    /// Serve signaling on 127.0.0.1 and join a new room there as its owner.
    /// `site` is the page a controller link opens, for example the desktop
    /// window's own origin.
    pub fn start(site: &str) -> Result<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(error)?;
        listener.set_nonblocking(true).map_err(error)?;
        let address = listener.local_addr().map_err(error)?;
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = std::thread::Builder::new()
            .name("fujimod-loopback-signal".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    let _ = axum::serve(listener, crate::signaling::router())
                        .with_graceful_shutdown(async {
                            let _ = stopped.await;
                        })
                        .await;
                });
            })
            .map_err(error)?;
        let mut rng = rand::rng();
        let ticket = Ticket {
            v: 1,
            signal: format!("ws://{address}/usb-proxy/signal"),
            room: URL_SAFE_NO_PAD.encode(rng.random::<[u8; 32]>()),
            key: URL_SAFE_NO_PAD.encode(rng.random::<[u8; 32]>()),
            stun: Vec::new(),
        };
        let owner_key = URL_SAFE_NO_PAD.encode(rng.random::<[u8; 32]>());
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ticket).map_err(error)?);
        let link = format!(
            "{}/#?remotecontrol={encoded}",
            site.trim_end_matches('/')
        );
        let room = futures_lite::future::block_on(HostRoom::join(
            &link,
            &owner_key,
            CandidatePolicy::DirectOnly,
            Pacing::Browser,
        ))?;
        Ok(Self {
            link,
            room,
            stop: Some(stop),
            server: Some(server),
        })
    }

    /// The share link a controller on this machine opens.
    pub fn link(&self) -> &str {
        &self.link
    }

    pub fn room(&self) -> &HostRoom {
        &self.room
    }
}

impl Drop for LoopbackRoom {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HostEvent, PeerSocket};
    use std::sync::Arc;

    #[test]
    fn a_controller_on_this_machine_joins_through_the_loopback_link() {
        let room = LoopbackRoom::start("http://127.0.0.1:1420").unwrap();
        assert!(room.link().starts_with("http://127.0.0.1:1420/#?remotecontrol="));
        let link = room.link().to_owned();
        let controller = std::thread::spawn(move || {
            futures_lite::future::block_on(async {
                let mut socket = PeerSocket::connect(
                    &link,
                    &(Arc::new(|_: &str| {}) as crate::Progress),
                    CandidatePolicy::DirectOnly,
                    Pacing::Unpaced,
                )
                .await
                .unwrap();
                socket.send(b"hello".to_vec()).await.unwrap();
                socket.receive().await.unwrap()
            })
        });
        let reply = futures_lite::future::block_on(async {
            let HostEvent::Controller(_, mut owner) = room.room().next_event().await.unwrap() else {
                panic!("controller expected");
            };
            assert_eq!(owner.receive().await.unwrap(), b"hello");
            // The native controller asked for no video.
            assert!(owner.video().is_none());
            owner.send(b"world".to_vec()).await.unwrap();
            owner
        });
        assert_eq!(controller.join().unwrap(), b"world");
        drop(reply);
    }
}
