pub mod button;
pub mod direction;
pub mod recognizer;
pub mod signature;

use klik_core::device::{Axes, ButtonSpec, Descriptor, Profile, ProfileKind};
use klik_core::error::ConfigError;
use klik_core::press::Recognized;
use klik_core::raw::Event;

use button::Button;
use recognizer::{Outcome, Recognizer, RecognizerSettings};
use signature::Signature;

/// The six buttons of the remote. The center has no hold, measured on
/// 2026-09-16; the arrows stream while held and the camera holds `BTN_TOUCH`.
pub const BUTTONS: [ButtonSpec; 6] = [
    ButtonSpec {
        name: "up",
        tap: true,
        hold: true,
        ticks: true,
    },
    ButtonSpec {
        name: "down",
        tap: true,
        hold: true,
        ticks: true,
    },
    ButtonSpec {
        name: "left",
        tap: true,
        hold: true,
        ticks: false,
    },
    ButtonSpec {
        name: "right",
        tap: true,
        hold: true,
        ticks: false,
    },
    ButtonSpec {
        name: "center",
        tap: true,
        hold: false,
        ticks: false,
    },
    ButtonSpec {
        name: "camera",
        tap: true,
        hold: true,
        ticks: false,
    },
];

/// The section `[devices.<name>]` of a Beauty-R1. Everything but `uniq` has
/// the value measured on the one unit, so a configuration needs the address
/// and nothing else.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub uniq: String,
    pub recognizer: RecognizerSettings,
    pub signatures: Vec<Signature>,
}

const DEFAULT_RECOGNIZER: RecognizerSettings = RecognizerSettings {
    debounce_ms: 45,
    connect_grace_ms: 500,
    camera_hold_ms: 200,
    volume_repeat_ms: 1500,
};

fn default_signatures() -> Vec<Signature> {
    vec![
        Signature {
            button: Button::Center,
            x: 1904,
            y: 1904,
            tolerance: 16,
        },
        Signature {
            button: Button::Camera,
            x: 2048,
            y: 3424,
            tolerance: 16,
        },
    ]
}

fn integer(table: &toml::Table, key: &str, default: u32) -> Result<u32, ConfigError> {
    match table.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_integer()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| ConfigError::BadDeviceValue {
                key: key.to_string(),
                expected: "a whole number of milliseconds, 0 or more",
            }),
    }
}

fn point(table: &toml::Table, key: &str, default: Signature) -> Result<Signature, ConfigError> {
    let Some(value) = table.get(key) else {
        return Ok(default);
    };
    let bad = || ConfigError::BadDeviceValue {
        key: key.to_string(),
        expected: "a table { x = .., y = .. } of two whole numbers",
    };
    let table = value.as_table().ok_or_else(bad)?;
    let axis = |name: &str| {
        table
            .get(name)
            .and_then(toml::Value::as_integer)
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(bad)
    };
    Ok(Signature {
        x: axis("x")?,
        y: axis("y")?,
        ..default
    })
}

impl Settings {
    pub fn from_table(table: &toml::Table) -> Result<Self, ConfigError> {
        let uniq = table
            .get("uniq")
            .and_then(toml::Value::as_str)
            .filter(|uniq| !uniq.is_empty())
            .ok_or(ConfigError::MissingDeviceKey("uniq"))?
            .to_string();
        let recognizer = RecognizerSettings {
            debounce_ms: integer(table, "debounce_ms", DEFAULT_RECOGNIZER.debounce_ms)?,
            connect_grace_ms: integer(
                table,
                "connect_grace_ms",
                DEFAULT_RECOGNIZER.connect_grace_ms,
            )?,
            camera_hold_ms: integer(table, "camera_hold_ms", DEFAULT_RECOGNIZER.camera_hold_ms)?,
            volume_repeat_ms: integer(
                table,
                "volume_repeat_ms",
                DEFAULT_RECOGNIZER.volume_repeat_ms,
            )?,
        };
        let tolerance = integer(table, "tolerance", 16)?;
        let tolerance = i32::try_from(tolerance).unwrap_or(i32::MAX);
        let defaults = default_signatures();
        let signatures = vec![
            point(
                table,
                "center",
                Signature {
                    tolerance,
                    ..defaults[0].clone()
                },
            )?,
            point(
                table,
                "camera",
                Signature {
                    tolerance,
                    ..defaults[1].clone()
                },
            )?,
        ];
        Ok(Self {
            uniq,
            recognizer,
            signatures,
        })
    }
}

/// The remote behind the profile trait of the core.
pub struct Beauty {
    settings: Settings,
    recognizer: Recognizer,
}

impl Beauty {
    pub fn new(settings: Settings) -> Self {
        let recognizer = Recognizer::new(settings.recognizer, settings.signatures.clone(), 0.0);
        Self {
            settings,
            recognizer,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }
}

/// The address is compared without case: sysfs writes it in lower case and a
/// hand-written configuration may not. A device with no address is never
/// the remote, whatever else matches.
pub fn uniq_matches(descriptor: &Descriptor, wanted: &str) -> bool {
    descriptor
        .uniq
        .as_deref()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(wanted))
}

/// The axes a connect hands over, read for the two the recognizer keys on.
pub fn position(axes: &Axes) -> (Option<i32>, Option<i32>) {
    let value = |name: &str| axes.iter().find(|(axis, _)| axis == name).map(|(_, v)| *v);
    (value("ABS_X"), value("ABS_Y"))
}

fn named(outcome: Outcome) -> Recognized {
    match outcome {
        Outcome::Press(button, kind) => Recognized::Press(button.as_str(), kind),
        Outcome::Tick(button) => Recognized::Tick(button.as_str()),
        Outcome::End(button) => Recognized::End(button.as_str()),
        Outcome::Unmatched { x, y } => Recognized::Unmatched(format!("x={x:?} y={y:?}")),
        Outcome::Refused { button, after_ms } => Recognized::Refused {
            button: button.as_str(),
            after_ms,
        },
    }
}

impl Profile for Beauty {
    fn buttons(&self) -> &'static [ButtonSpec] {
        &BUTTONS
    }

    fn matches(&self, descriptor: &Descriptor) -> bool {
        uniq_matches(descriptor, &self.settings.uniq)
    }

    /// Half the hold threshold: a poll on the boundary can miss a hold by a
    /// whole period, and halving it bounds the error by half of one. The
    /// floor keeps a hold of zero from turning the loop into a spin.
    fn poll_ms(&self) -> u32 {
        (self.settings.recognizer.camera_hold_ms / 2).max(10)
    }

    fn connected(&mut self, at: f64, axes: &Axes) {
        self.recognizer.reset();
        let (x, y) = position(axes);
        self.recognizer.seed_position(x, y);
        self.recognizer.rearm_grace(at);
    }

    fn disconnected(&mut self) {
        self.recognizer.reset();
    }

    fn feed(&mut self, event: &Event) -> Option<Recognized> {
        self.recognizer.feed(event).map(named)
    }

    fn tick(&mut self, now: f64) -> Option<Recognized> {
        self.recognizer.tick(now).map(named)
    }
}

fn build(table: &toml::Table) -> Result<Box<dyn Profile>, ConfigError> {
    Ok(Box::new(Beauty::new(Settings::from_table(table)?)))
}

pub const KIND: ProfileKind = ProfileKind {
    id: "beauty-r1",
    build,
};

#[cfg(test)]
mod tests {
    use super::*;
    use klik_core::press::PressKind;
    use klik_core::raw::Event;

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).expect("the test table parses")
    }

    fn descriptor(uniq: Option<&str>) -> Descriptor {
        Descriptor {
            bus: 5,
            vendor: 0x05ac,
            product: 0x022c,
            version: 0x011b,
            name: "Beauty-R1".to_string(),
            uniq: uniq.map(str::to_string),
        }
    }

    /// The section needs the address and nothing else; the rest is what was
    /// measured on the one unit.
    #[test]
    fn the_address_alone_makes_a_settings_with_the_measured_defaults() {
        let settings = Settings::from_table(&table(r#"uniq = "aa:bb:cc:dd:ee:01""#)).unwrap();
        assert_eq!(settings.recognizer, DEFAULT_RECOGNIZER);
        assert_eq!(settings.signatures, default_signatures());
    }

    #[test]
    fn a_section_without_an_address_is_refused() {
        assert!(matches!(
            Settings::from_table(&table("debounce_ms = 45")),
            Err(ConfigError::MissingDeviceKey("uniq"))
        ));
        assert!(matches!(
            Settings::from_table(&table(r#"uniq = """#)),
            Err(ConfigError::MissingDeviceKey("uniq"))
        ));
    }

    #[test]
    fn a_coordinate_overrides_the_default_and_keeps_the_tolerance() {
        let settings = Settings::from_table(&table(
            r#"
            uniq = "aa"
            tolerance = 8
            camera = { x = 2000, y = 3400 }
            "#,
        ))
        .unwrap();
        assert_eq!(settings.signatures[0].tolerance, 8);
        assert_eq!(settings.signatures[1].x, 2000);
        assert_eq!(settings.signatures[1].y, 3400);
        assert_eq!(settings.signatures[1].tolerance, 8);
    }

    #[test]
    fn a_coordinate_of_the_wrong_shape_is_refused_by_key() {
        let error = Settings::from_table(&table(
            r#"uniq = "aa"
center = 5"#,
        ))
        .unwrap_err();
        assert!(matches!(error, ConfigError::BadDeviceValue { ref key, .. } if key == "center"));
    }

    #[test]
    fn the_address_decides_the_match_without_case() {
        let beauty =
            Beauty::new(Settings::from_table(&table(r#"uniq = "AA:BB:CC:DD:EE:01""#)).unwrap());
        assert!(beauty.matches(&descriptor(Some("aa:bb:cc:dd:ee:01"))));
        assert!(!beauty.matches(&descriptor(Some("aa:bb:cc:dd:ee:02"))));
        assert!(
            !beauty.matches(&descriptor(None)),
            "no address is never the remote"
        );
    }

    /// The whole clean run through the profile: six taps, named as the core
    /// names them, with the arrows ended.
    #[test]
    fn the_clean_run_resolves_to_six_named_taps() {
        let text = include_str!("../tests/fixtures/beauty-r1-clean-run.jsonl");
        let events = Event::parse_stream(text).unwrap();
        let mut beauty = Beauty::new(Settings::from_table(&table(r#"uniq = "aa""#)).unwrap());
        beauty.connected(events[0].time - 1.0, &Vec::new());
        let presses: Vec<(&str, PressKind)> = events
            .iter()
            .filter_map(|event| beauty.feed(event))
            .filter_map(|recognized| match recognized {
                Recognized::Press(button, kind) => Some((button, kind)),
                _ => None,
            })
            .collect();
        assert_eq!(
            presses,
            vec![
                ("up", PressKind::Tap),
                ("down", PressKind::Tap),
                ("left", PressKind::Tap),
                ("right", PressKind::Tap),
                ("center", PressKind::Tap),
                ("camera", PressKind::Tap),
            ]
        );
    }

    #[test]
    fn the_poll_is_half_the_hold_with_a_floor() {
        let mut beauty = Beauty::new(Settings::from_table(&table(r#"uniq = "aa""#)).unwrap());
        assert_eq!(beauty.poll_ms(), 100);
        beauty.settings.recognizer.camera_hold_ms = 4;
        assert_eq!(beauty.poll_ms(), 10);
    }
}
