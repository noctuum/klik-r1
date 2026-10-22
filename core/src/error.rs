#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("line {line}: {source}")]
    Json {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("the device section lacks {0}")]
    MissingDeviceKey(&'static str),
    #[error("the device key {key} is not {expected}")]
    BadDeviceValue { key: String, expected: &'static str },
}
