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
pub enum ConfigError {}
