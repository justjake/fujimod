//! Local-development share registry. A browser page that is sharing a camera
//! registers its standard share link here over a WebSocket; while that socket
//! stays open the link is one 0600 JSON file in a private directory, which
//! host tools read directly. Nothing serves the directory back over HTTP.
use crate::share::Ticket;
use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const MAX_REGISTRATION: usize = 16 * 1024;

#[derive(Clone)]
struct Registry {
    directory: Arc<PathBuf>,
    origins: Arc<Vec<String>>,
    counter: Arc<AtomicU64>,
}

/// Mount `/usb-proxy/dev-share`. Clears earlier registrations in `directory`.
pub fn router(directory: PathBuf, origins: Vec<String>) -> std::io::Result<Router> {
    std::fs::create_dir_all(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    for entry in std::fs::read_dir(&directory)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            std::fs::remove_file(path)?;
        }
    }
    let registry = Registry {
        directory: Arc::new(directory),
        origins: Arc::new(origins),
        counter: Arc::new(AtomicU64::new(1)),
    };
    Ok(Router::new()
        .route("/usb-proxy/dev-share", get(upgrade))
        .with_state(registry))
}

async fn upgrade(
    State(registry): State<Registry>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    // Only browser pages served by this development site register.
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if origin.is_empty()
        || (origin != format!("https://{host}")
            && origin != format!("http://{host}")
            && !registry.origins.iter().any(|allowed| allowed == origin))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(MAX_REGISTRATION)
        .on_upgrade(|socket| session(registry, socket))
}

fn write_private(path: &Path, value: &Value) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(value.to_string().as_bytes())?;
    file.sync_all()?;
    std::fs::rename(temporary, path)
}

/// The share's camera state as host tools need it: present or not (and why),
/// and the last battery level with its wall-clock time. Unknown fields are dropped.
fn status(value: &Value) -> Value {
    let text = |key: &str| {
        value[key]
            .as_str()
            .map(|s| Value::String(crate::protocol::bounded_reason(s)))
            .unwrap_or(Value::Null)
    };
    json!({
        "online": value["online"].as_bool().unwrap_or(true),
        "reason": text("reason"),
        "battery": text("battery"),
        "battery_at_ms": value["battery_at_ms"].as_u64(),
    })
}

async fn session(registry: Registry, mut socket: WebSocket) {
    let id = registry.counter.fetch_add(1, Ordering::Relaxed);
    let path = registry
        .directory
        .join(format!("{}-{id}.json", std::process::id()));
    while let Some(Ok(message)) = socket.recv().await {
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let reply = match serde_json::from_str::<Value>(&text) {
            Ok(frame) => match frame["url"].as_str().map(Ticket::from_url) {
                Some(Ok(Some(_))) => {
                    let entry = json!({
                        "url": frame["url"],
                        "camera": frame["camera"],
                        "status": status(&frame["status"]),
                    });
                    match write_private(&path, &entry) {
                        Ok(()) => json!({"type": "registered"}),
                        Err(error) => json!({"type": "error", "message": error.to_string()}),
                    }
                }
                _ => json!({"type": "error", "message": "not a share link"}),
            },
            Err(_) => json!({"type": "error", "message": "invalid registration"}),
        };
        if socket
            .send(Message::Text(reply.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
    let _ = std::fs::remove_file(&path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_tungstenite::tungstenite::{Message as Wire, client::IntoClientRequest};
    use futures_util::StreamExt;

    fn link() -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let ticket = json!({"v": 1, "signal": "wss://dev.test/usb-proxy/signal",
            "room": URL_SAFE_NO_PAD.encode([1; 32]), "key": URL_SAFE_NO_PAD.encode([2; 32])});
        format!(
            "https://dev.test/dev-tools/#?remotecontrol={}",
            URL_SAFE_NO_PAD.encode(ticket.to_string())
        )
    }

    fn files(directory: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(directory)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect()
    }

    #[tokio::test]
    async fn registration_lives_exactly_as_long_as_its_socket() {
        let directory = std::env::temp_dir().join(format!("dev-shares-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("stale.json"), "{}").unwrap();
        let app = router(directory.clone(), vec![]).unwrap();
        assert!(
            files(&directory).is_empty(),
            "startup clears stale registrations"
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let url = format!("ws://{address}/usb-proxy/dev-share");

        let mut native = url.clone().into_client_request().unwrap();
        native.headers_mut().remove("origin");
        assert!(
            async_tungstenite::smol::connect_async(native)
                .await
                .is_err()
        );

        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", format!("http://{address}").parse().unwrap());
        let (mut socket, _) = async_tungstenite::smol::connect_async(request)
            .await
            .unwrap();
        socket
            .send(Wire::Text(json!({"url": "https://x/"}).to_string().into()))
            .await
            .unwrap();
        let reply = socket.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(reply.contains("not a share link"));
        assert!(files(&directory).is_empty());

        socket
            .send(Wire::Text(
                json!({"url": link(), "camera": {"model": "X70"}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let reply = socket.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(reply.contains("registered"));
        let [entry] = files(&directory).try_into().unwrap();
        assert_eq!(
            std::fs::metadata(&entry).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let stored: Value = serde_json::from_slice(&std::fs::read(&entry).unwrap()).unwrap();
        assert_eq!(stored["url"], link());
        assert_eq!(stored["camera"]["model"], "X70");
        assert_eq!(stored["status"]["online"], true);

        socket
            .send(Wire::Text(
                json!({"url": link(), "camera": {"model": "X70"},
                       "status": {"online": false, "reason": "battery", "battery": "END",
                                  "battery_at_ms": 5, "extra": "dropped"}})
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        socket.next().await.unwrap().unwrap();
        let stored: Value = serde_json::from_slice(&std::fs::read(&entry).unwrap()).unwrap();
        assert_eq!(
            stored["status"],
            json!({"online": false, "reason": "battery", "battery": "END", "battery_at_ms": 5})
        );

        socket.close(None).await.unwrap();
        for _ in 0..50 {
            if files(&directory).is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            files(&directory).is_empty(),
            "closing the socket withdraws the share"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
