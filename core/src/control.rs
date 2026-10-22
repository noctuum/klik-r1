use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One line from a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    Status,
    Events,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub id: String,
    pub profile: String,
    pub connected: bool,
    /// `taken`, `refused: <why>` or `skipped`, once connected.
    pub grab: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub devices: Vec<DeviceStatus>,
    pub profile: String,
    pub why: String,
}

/// One line to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Status(Status),
    Event { line: String },
    Error { message: String },
}

pub fn socket_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("klik").join("control")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_one_tagged_object() {
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"request":"status"}"#).unwrap(),
            Request::Status
        );
        assert_eq!(
            serde_json::to_string(&Request::Events).unwrap(),
            r#"{"request":"events"}"#
        );
        assert!(serde_json::from_str::<Request>(r#"{"request":"reload"}"#).is_err());
    }

    #[test]
    fn a_reply_carries_its_kind_in_the_tag() {
        let reply = Reply::Event {
            line: "up tap".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&reply).unwrap(),
            r#"{"reply":"event","line":"up tap"}"#
        );
    }

    #[test]
    fn the_socket_is_under_the_runtime_directory() {
        assert_eq!(
            socket_path(Path::new("/run/user/1000")),
            PathBuf::from("/run/user/1000/klik/control")
        );
    }
}
