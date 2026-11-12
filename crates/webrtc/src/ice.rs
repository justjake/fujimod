//! The STUN and TURN servers a signaling join reply offers a room.
use serde_json::Value;

/// Long-term TURN credentials issued for one room.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// One `ice_servers` entry from a signaling join reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub credentials: Option<Credentials>,
}

/// The `ice_servers` of a join reply. Entries of an unexpected shape are skipped.
pub fn ice_servers(value: &Value) -> Vec<IceServer> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let urls = match &entry["urls"] {
                Value::String(url) => vec![url.clone()],
                Value::Array(list) => list
                    .iter()
                    .filter_map(|url| url.as_str().map(str::to_owned))
                    .collect(),
                _ => return None,
            };
            let credentials = match (entry["username"].as_str(), entry["credential"].as_str()) {
                (Some(username), Some(password)) => Some(Credentials {
                    username: username.to_owned(),
                    password: password.to_owned(),
                }),
                _ => None,
            };
            Some(IceServer { urls, credentials })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ice_servers_keep_credentials_and_skip_malformed_entries() {
        let reply = serde_json::json!([
            {"urls": ["stun:stun.example.test:3478"]},
            {
                "urls": "turn:turn.example.test:3478?transport=udp",
                "username": "1700000000:room",
                "credential": "shared-secret"
            },
            {"urls": 7},
        ]);
        let servers = ice_servers(&reply);
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].credentials, None);
        assert_eq!(
            servers[1].urls,
            ["turn:turn.example.test:3478?transport=udp"]
        );
        assert_eq!(
            servers[1].credentials,
            Some(Credentials {
                username: "1700000000:room".into(),
                password: "shared-secret".into(),
            })
        );
        assert!(ice_servers(&serde_json::json!({"not": "a list"})).is_empty());
    }
}
