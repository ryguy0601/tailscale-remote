use serde::{Deserialize, Serialize};

/// Messages sent from the phone remote UI to the media PC host over `/ws/remote`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    Move {
        dx: i32,
        dy: i32,
    },
    Click {
        btn: String,
        #[serde(default)]
        down: Option<bool>,
    },
    Scroll {
        dx: i32,
        dy: i32,
    },
    Key {
        code: String,
        #[serde(default)]
        down: Option<bool>,
        #[serde(default)]
        mods: Option<Vec<String>>,
    },
    Text {
        text: String,
        #[serde(default)]
        enter: Option<bool>,
    },
    Clipboard {
        text: String,
        #[serde(default)]
        paste: Option<bool>,
    },
    Media {
        action: String,
    },
    Ping {
        ts: u64,
    },
}

/// Messages sent from the media PC host back to the phone remote UI over `/ws/remote`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Pong {
        ts: u64,
    },
    Hello {
        hostname: String,
        os: String,
        tailscale_ip: String,
        dry_run: bool,
    },
    Error {
        message: String,
    },
}

/// Metadata for a currently connected phone client shown on `/qr` and `/api/status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectedClient {
    pub id: u64,
    pub ip: String,
    pub label: String,
}

/// Live state snapshots pushed to the loopback `/qr` display over `/ws/host`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HostEvent {
    Snapshot {
        connected: Vec<ConnectedClient>,
        pairing_url: String,
        magic_dns_url: Option<String>,
        qr_svg: String,
        auth_enabled: bool,
        hostname: String,
        tailscale_ip: String,
        os: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_msg_round_trip() {
        let msgs = vec![
            ClientMsg::Move { dx: 12, dy: -7 },
            ClientMsg::Click {
                btn: "left".into(),
                down: Some(true),
            },
            ClientMsg::Scroll { dx: 0, dy: -2 },
            ClientMsg::Key {
                code: "KeyW".into(),
                down: None,
                mods: Some(vec!["ctrl".into()]),
            },
            ClientMsg::Text {
                text: "hello world".into(),
                enter: Some(true),
            },
            ClientMsg::Clipboard {
                text: "https://example.com".into(),
                paste: Some(true),
            },
            ClientMsg::Media {
                action: "play_pause".into(),
            },
            ClientMsg::Ping { ts: 123456789 },
        ];
        for msg in msgs {
            let json = serde_json::to_string(&msg).unwrap();
            let decoded: ClientMsg = serde_json::from_str(&json).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_server_msg_round_trip() {
        let msgs = vec![
            ServerMsg::Pong { ts: 42 },
            ServerMsg::Hello {
                hostname: "media-pc".into(),
                os: "linux".into(),
                tailscale_ip: "100.64.0.10".into(),
                dry_run: false,
            },
            ServerMsg::Error {
                message: "invalid input".into(),
            },
        ];
        for msg in msgs {
            let json = serde_json::to_string(&msg).unwrap();
            let decoded: ServerMsg = serde_json::from_str(&json).unwrap();
            assert_eq!(msg, decoded);
        }
    }
}
