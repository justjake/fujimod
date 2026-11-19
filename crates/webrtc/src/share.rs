//! Portable capability ticket embedded only in the URL fragment.
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    pub v: u32,
    pub signal: String,
    pub room: String,
    pub key: String,
    #[serde(default)]
    pub stun: Vec<String>,
}

/// Persistent owner credentials stored locally, with a controller link that
/// contains only the room and link key. Other fields belong to the program
/// that keeps the ticket and are ignored.
#[derive(Clone, Serialize, Deserialize)]
pub struct OwnerTicket {
    pub link: String,
    pub owner_key: String,
}

impl OwnerTicket {
    /// Create a persistent room ticket and its Dev Tools share link.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn create(signal: impl Into<String>, site: impl AsRef<str>) -> Result<Self> {
        use rand::RngExt;

        let mut rng = rand::rng();
        let room = URL_SAFE_NO_PAD.encode(rng.random::<[u8; 32]>());
        let key = URL_SAFE_NO_PAD.encode(rng.random::<[u8; 32]>());
        let owner_key = URL_SAFE_NO_PAD.encode(rng.random::<[u8; 32]>());
        let ticket = Ticket {
            v: 1,
            signal: signal.into(),
            room,
            key,
            stun: vec!["stun:stun.cloudflare.com:3478".into()],
        };
        let encoded = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&ticket).map_err(|error| Error(error.to_string()))?);
        let site = site.as_ref().trim_end_matches('/');
        let owner_ticket = Self {
            link: format!("{site}/dev-tools/#?remotecontrol={encoded}"),
            owner_key,
        };
        owner_ticket.ticket()?;
        Ok(owner_ticket)
    }

    /// Read an owner ticket from its JSON file.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|error| Error(error.to_string()))?;
        let ticket: Self =
            serde_json::from_slice(&bytes).map_err(|error| Error(error.to_string()))?;
        ticket.ticket()?;
        Ok(ticket)
    }

    /// Write the owner ticket as JSON, restricting the file to mode 0600 on
    /// Unix systems.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        use std::io::Write;

        self.ticket()?;
        let bytes = serde_json::to_vec(self).map_err(|error| Error(error.to_string()))?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .map_err(|error| Error(error.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|error| Error(error.to_string()))?;
        }
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| Error(error.to_string()))
    }

    /// Parse and return the controller ticket embedded in `link`.
    pub fn ticket(&self) -> Result<Ticket> {
        let ticket = Ticket::from_url(&self.link)?
            .ok_or_else(|| Error("owner ticket has no share link".into()))?;
        if URL_SAFE_NO_PAD
            .decode(self.owner_key.as_bytes())
            .map(|key| key.len())
            .ok()
            != Some(32)
        {
            return Err(Error("invalid owner key".into()));
        }
        Ok(ticket)
    }
}

impl Ticket {
    pub fn from_url(value: &str) -> Result<Option<Self>> {
        if !value.contains('#') {
            return Ok(None);
        }
        let url = url::Url::parse(value).map_err(|_| Error("invalid share URL".into()))?;
        let Some(fragment) = url.fragment() else {
            return Ok(None);
        };
        let values: Vec<_> =
            url::form_urlencoded::parse(fragment.trim_start_matches('?').as_bytes())
                .filter(|(key, _)| key == "remotecontrol")
                .collect();
        if values.is_empty() {
            return Ok(None);
        }
        if values.len() != 1 || values[0].1.len() > 4096 {
            return Err(Error("invalid share ticket".into()));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(values[0].1.as_bytes())
            .map_err(|_| Error("invalid share ticket".into()))?;
        let ticket: Self =
            serde_json::from_slice(&bytes).map_err(|_| Error("invalid share ticket".into()))?;
        let signal =
            url::Url::parse(&ticket.signal).map_err(|_| Error("invalid signaling URL".into()))?;
        if ticket.v != 1
            || !["ws", "wss"].contains(&signal.scheme())
            || !signal.username().is_empty()
            || signal.password().is_some()
            || URL_SAFE_NO_PAD.decode(&ticket.room).map(|v| v.len()).ok() != Some(32)
            || URL_SAFE_NO_PAD.decode(&ticket.key).map(|v| v.len()).ok() != Some(32)
            || ticket.stun.len() > 4
            || ticket
                .stun
                .iter()
                .any(|s| !s.starts_with("stun:") || s.len() > 256)
        {
            return Err(Error("unsupported share ticket".into()));
        }
        Ok(Some(ticket))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn owner_ticket_creates_and_round_trips_a_private_file() {
        let owner = OwnerTicket::create(
            "wss://signal.example/usb-proxy/signal",
            "https://camera.example/",
        )
        .unwrap();
        assert!(
            owner
                .link
                .starts_with("https://camera.example/dev-tools/#?remotecontrol=")
        );
        let ticket = owner.ticket().unwrap();
        let link_ticket = Ticket::from_url(&owner.link).unwrap().unwrap();
        assert_eq!(ticket.room, link_ticket.room);
        assert_eq!(ticket.key, link_ticket.key);
        assert_eq!(ticket.signal, "wss://signal.example/usb-proxy/signal");
        assert_eq!(URL_SAFE_NO_PAD.decode(&ticket.room).unwrap().len(), 32);
        assert_eq!(URL_SAFE_NO_PAD.decode(&ticket.key).unwrap().len(), 32);
        assert_eq!(URL_SAFE_NO_PAD.decode(&owner.owner_key).unwrap().len(), 32);

        let another = OwnerTicket::create(
            "wss://signal.example/usb-proxy/signal",
            "https://camera.example",
        )
        .unwrap();
        let another_ticket = another.ticket().unwrap();
        assert_ne!(ticket.room, another_ticket.room);
        assert_ne!(ticket.key, another_ticket.key);
        assert_ne!(owner.owner_key, another.owner_key);

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "fujimod-owner-ticket-{}-{stamp}.json",
            std::process::id()
        ));
        owner.write(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let loaded = OwnerTicket::read(&path).unwrap();
        assert_eq!(loaded.link, owner.link);
        assert_eq!(loaded.owner_key, owner.owner_key);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn full_browser_url_is_a_native_controller_capability() {
        let ticket = Ticket {
            v: 1,
            signal: "wss://signal.example/usb-proxy/signal".into(),
            room: URL_SAFE_NO_PAD.encode([1; 32]),
            key: URL_SAFE_NO_PAD.encode([2; 32]),
            stun: vec!["stun:stun.cloudflare.com:3478".into()],
        };
        let link = format!(
            "https://camera.example/dev-tools/?firmware=body#?remotecontrol={}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ticket).unwrap())
        );
        let parsed = Ticket::from_url(&link).unwrap().unwrap();
        assert_eq!(parsed.signal, ticket.signal);
        assert_eq!(parsed.key, ticket.key);
        assert!(
            Ticket::from_url("wss://camera.example/usb-proxy/ws")
                .unwrap()
                .is_none()
        );
        assert!(Ticket::from_url("https://camera.example/#?remotecontrol=e30").is_err());
        assert!(Ticket::from_url(&(link.clone() + "&remotecontrol=e30")).is_err());
    }
}
