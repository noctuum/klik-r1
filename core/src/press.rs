use serde::{Deserialize, Serialize};

/// The name a device profile gives one of its buttons. Static because a
/// profile is code, and its buttons are known when it is written.
pub type ButtonName = &'static str;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PressKind {
    Tap,
    Hold,
}

impl PressKind {
    pub fn from_config_name(name: &str) -> Option<Self> {
        match name {
            "tap" => Some(Self::Tap),
            "hold" => Some(Self::Hold),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tap => "tap",
            Self::Hold => "hold",
        }
    }
}

/// What a device profile makes of the raw events, one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognized {
    /// A gesture resolved to a button.
    Press(ButtonName, PressKind),
    /// A further event inside a resolved hold. Scrolling runs on these.
    Tick(ButtonName),
    /// The hold ended.
    End(ButtonName),
    /// A gesture that matched nothing, described for the journal: the
    /// numbers a configuration needs are in the text.
    Unmatched(String),
    /// A press the profile refused, with the time since the last release.
    Refused { button: ButtonName, after_ms: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_round_trips_through_its_name() {
        for kind in [PressKind::Tap, PressKind::Hold] {
            assert_eq!(PressKind::from_config_name(kind.as_str()), Some(kind));
        }
        assert_eq!(PressKind::from_config_name("double"), None);
    }
}
