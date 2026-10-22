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
    #[error("malformed TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("the device section lacks {0}")]
    MissingDeviceKey(&'static str),
    #[error("the device key {key} is not {expected}")]
    BadDeviceValue { key: String, expected: &'static str },
    #[error("the device section has a key {key} its profile does not read; it reads {known}")]
    UnknownDeviceKey { key: String, known: String },
    #[error("no [devices.<id>] section, so there is nothing to read")]
    NoDevices,
    #[error("no [profiles.<name>] section, so no button does anything")]
    NoProfiles,
    #[error(
        "several profiles and no [[rules]] to choose between them; add rules and end them with one that has only use"
    )]
    SeveralProfilesNoRules,
    #[error("rule {rule} uses {profile}, which is no profile of the configuration")]
    RuleUnknownProfile { rule: usize, profile: String },
    #[error(
        "the last [[rules]] has a condition, so nothing is active when no rule matches; end the rules with one that has only use"
    )]
    NoDefaultRule,
    #[error("rule {0} has no condition, so the rules after it never apply")]
    RuleAfterDefault(usize),
    #[error("rule {rule}: the condition on {fact} is not text")]
    BadCondition { rule: usize, fact: String },
    #[error("malformed binding key {0}, which has to be device.button:kind")]
    BadBindingKey(String),
    #[error("binding {key}: the action {action} has to be module.action")]
    BadAction { key: String, action: String },
    #[error("binding {key} names a device that has no section")]
    UnknownDevice { key: String },
    #[error("binding {key} names a button the device does not have")]
    UnknownButton { key: String },
    #[error("binding {key} names a kind of press the button does not produce")]
    KindNotProduced { key: String },
    #[error("binding {key}: no module declares the action {action}")]
    UnknownAction { key: String, action: String },
    #[error("binding {key}: {reason}")]
    BadParams { key: String, reason: String },
    #[error("profile {profile} extends {parent}, which does not exist")]
    UnknownParent { profile: String, parent: String },
    #[error("the profiles extend one another in a circle through {0}")]
    ExtendsCycle(String),
    #[error("module {0}: {1}")]
    Module(String, String),
}
