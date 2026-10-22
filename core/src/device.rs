use crate::error::ConfigError;
use crate::press::{ButtonName, Recognized};
use crate::raw::Event;

/// What the platform knows of an input device before anything opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    pub bus: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    pub name: String,
    /// The unique address, such as a Bluetooth MAC. Most devices have none.
    pub uniq: Option<String>,
}

/// One button of a device and the kinds of press it produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonSpec {
    pub name: ButtonName,
    pub tap: bool,
    pub hold: bool,
    /// A hold streams ticks, as a held arrow of the Beauty-R1 does.
    pub ticks: bool,
}

impl ButtonSpec {
    pub fn produces(&self, kind: crate::press::PressKind) -> bool {
        match kind {
            crate::press::PressKind::Tap => self.tap,
            crate::press::PressKind::Hold => self.hold,
        }
    }
}

/// The current value of each absolute axis of a device when it is opened.
/// The kernel reports an axis only when it changes, so a profile that keys on
/// a position needs this before the first gesture.
pub type Axes = Vec<(String, i32)>;

/// A device profile: one per model of remote, written in the repository.
pub trait Profile: Send {
    /// The buttons of the remote and what each can produce.
    fn buttons(&self) -> &'static [ButtonSpec];
    /// Whether this descriptor is the remote this profile is configured for.
    fn matches(&self, descriptor: &Descriptor) -> bool;
    /// How often `tick` wants to run while nothing arrives, in milliseconds.
    fn poll_ms(&self) -> u32;
    /// The remote was opened at `at`, with its axes where `axes` says.
    fn connected(&mut self, at: f64, axes: &Axes);
    /// The remote went away. Whatever gesture was in flight is over.
    fn disconnected(&mut self);
    fn feed(&mut self, event: &Event) -> Option<Recognized>;
    /// Nothing arrived; `now` is the clock the events carry.
    fn tick(&mut self, now: f64) -> Option<Recognized>;
}

/// How a profile is built from its section of the configuration.
pub struct ProfileKind {
    /// The name a `[devices.<name>]` section refers to with `profile`, and
    /// the name of the section itself when it has no such key.
    pub id: &'static str,
    pub build: fn(&toml::Table) -> Result<Box<dyn Profile>, ConfigError>,
}

/// The specs of the button named, if the profile has it.
pub fn button_spec(buttons: &[ButtonSpec], name: &str) -> Option<ButtonSpec> {
    buttons.iter().copied().find(|button| button.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::press::PressKind;

    const BUTTONS: [ButtonSpec; 2] = [
        ButtonSpec {
            name: "ok",
            tap: true,
            hold: false,
            ticks: false,
        },
        ButtonSpec {
            name: "up",
            tap: true,
            hold: true,
            ticks: true,
        },
    ];

    #[test]
    fn a_button_is_found_by_name_and_says_what_it_produces() {
        let ok = button_spec(&BUTTONS, "ok").expect("ok exists");
        assert!(ok.produces(PressKind::Tap));
        assert!(!ok.produces(PressKind::Hold));
        assert!(button_spec(&BUTTONS, "left").is_none());
    }
}
