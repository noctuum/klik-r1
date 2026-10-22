use std::io;

use crate::error::ConfigError;
use crate::platform::{Injector, Process};
use crate::press::PressKind;

/// What a parameter of an action is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// A key name, or several joined with `+`, for the injector to declare.
    Key,
    Text,
    Integer,
    Number,
    Flag,
    OneOf(&'static [&'static str]),
    /// The name of a profile of the configuration, which the layout checks.
    Profile,
    /// A whole number of milliseconds above 0.
    Millis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
}

/// One action a module declares, with its parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionSpec {
    pub module: &'static str,
    pub name: &'static str,
    pub params: &'static [ParamSpec],
    /// Of these parameters exactly one is given.
    pub exactly_one_of: &'static [&'static str],
    /// The action binds a hold only: it acts on the start and on the end.
    pub hold_only: bool,
}

impl ActionSpec {
    /// The fields a literal leaves alone, as `..ActionSpec::PLAIN`.
    pub const PLAIN: ActionSpec = ActionSpec {
        module: "",
        name: "",
        params: &[],
        exactly_one_of: &[],
        hold_only: false,
    };

    /// Whether these parameters fit the action: every required one present,
    /// none unknown, each of its kind. The reason names the parameter.
    pub fn check(&self, params: &toml::Table) -> Result<(), String> {
        if !self.exactly_one_of.is_empty() {
            let given = self
                .exactly_one_of
                .iter()
                .filter(|name| params.contains_key(**name))
                .count();
            if given != 1 {
                return Err(format!(
                    "the action {}.{} takes exactly one of {}",
                    self.module,
                    self.name,
                    self.exactly_one_of.join(", ")
                ));
            }
        }
        for spec in self.params {
            match params.get(spec.name) {
                None if spec.required => {
                    return Err(format!("the parameter {} is required", spec.name));
                }
                Some(value) if !spec.kind.accepts(value) => {
                    return Err(format!(
                        "the parameter {} is not {}",
                        spec.name,
                        spec.kind.describe()
                    ));
                }
                _ => {}
            }
        }
        if let Some(unknown) = params
            .keys()
            .find(|key| !self.params.iter().any(|spec| spec.name == key.as_str()))
        {
            return Err(format!(
                "the action {}.{} does not take {unknown}",
                self.module, self.name
            ));
        }
        Ok(())
    }
}

impl ParamKind {
    pub fn accepts(&self, value: &toml::Value) -> bool {
        match self {
            Self::Key => value.as_str().is_some_and(|text| {
                !text.is_empty()
                    && text
                        .split('+')
                        .all(|name| name.starts_with("KEY_") || name.starts_with("BTN_"))
            }),
            Self::Text => value.is_str(),
            Self::Integer => value.is_integer(),
            Self::Number => value.is_integer() || value.is_float(),
            Self::Flag => value.is_bool(),
            Self::OneOf(choices) => value.as_str().is_some_and(|text| choices.contains(&text)),
            Self::Profile => value.is_str(),
            Self::Millis => value.as_integer().is_some_and(|ms| ms > 0),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Key => "a key name such as KEY_1, or several joined with +".to_string(),
            Self::Text => "text".to_string(),
            Self::Integer => "a whole number".to_string(),
            Self::Number => "a number".to_string(),
            Self::Flag => "true or false".to_string(),
            Self::OneOf(choices) => format!("one of {}", choices.join(", ")),
            Self::Profile => "the name of a profile".to_string(),
            Self::Millis => "a whole number of milliseconds above 0".to_string(),
        }
    }
}

/// Where a press is inside its gesture, from the module's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressEvent {
    /// The press itself. A tap is this and nothing more.
    Start(PressKind),
    /// The `nth` report of a held button, counting the press as 1.
    Tick { nth: u32 },
    /// The hold ended.
    End,
}

/// What an action receives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Press<'a> {
    pub device: &'a str,
    pub button: &'a str,
    pub event: PressEvent,
    /// The time of the event clock, seconds since the epoch.
    pub at: f64,
    /// The profile the press was looked up in, which its ticks and its end
    /// keep.
    pub profile: &'a str,
}

/// A rectangle of the screen in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A window the desktop knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: String,
    pub class: String,
    pub title: String,
    pub rect: Rect,
}

/// Which window an action means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowMatch {
    Class(String),
    Title(String),
}

/// A step between virtual desktops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
}

/// What a desktop module gives. Without one, `Services::desktop` is `None`
/// and an action that needs a window says so.
pub trait Desktop {
    fn front(&mut self) -> io::Result<Option<Window>>;
    fn screen(&mut self) -> io::Result<Rect>;
    fn switch(&mut self, step: Step) -> io::Result<()>;
    fn activate(&mut self, which: &WindowMatch) -> io::Result<()>;
    /// The name of the desktop in view.
    fn current_desktop(&mut self) -> io::Result<String>;
}

pub trait Journal {
    fn line(&mut self, text: &str);
    fn debug(&mut self, text: &str);
}

/// The four profile actions, as a service an action can call.
pub trait Profiles {
    /// Keeps `name` active until the next switch.
    fn set(&mut self, name: &str);
    /// Keeps the profile after `current`, in the order of the file, active.
    fn next_after(&mut self, current: &str);
    /// Hands the choice back to the rules.
    fn auto(&mut self);
    /// Makes `name` active while this button is held.
    fn hold(&mut self, device: &str, button: &str, name: &str);
    fn release(&mut self, device: &str, button: &str);
}

/// The services of the core, handed to an action for the time it runs.
pub struct Services<'a> {
    pub input: &'a mut dyn Injector,
    pub journal: &'a mut dyn Journal,
    pub desktop: Option<&'a mut dyn Desktop>,
    pub process: &'a mut dyn Process,
    pub profiles: &'a mut dyn Profiles,
}

/// What an action that failed says. Printed with the label of the press.
pub type Fault = Box<dyn std::error::Error + Send + Sync>;

pub trait Module: Send {
    fn name(&self) -> &'static str;
    fn actions(&self) -> &'static [ActionSpec];
    /// The `[modules.<name>]` section, or none. An error here stops the
    /// daemon at startup with the name of the module and the reason.
    fn configure(&mut self, section: Option<&toml::Table>) -> Result<(), ConfigError>;
    /// Runs an action this module declared, with parameters `ActionSpec::check`
    /// accepted.
    fn run(
        &mut self,
        action: &str,
        params: &toml::Table,
        press: &Press,
        services: &mut Services,
    ) -> Result<(), Fault>;
    /// The facts this module can be asked for.
    fn facts(&self) -> &'static [&'static str] {
        &[]
    }
    /// The values of `wanted`, all of them facts this module declared, as
    /// they are now. A fact the module cannot tell is left out.
    fn ask(
        &mut self,
        wanted: &[String],
        services: &mut Services,
    ) -> Result<Vec<(String, String)>, Fault> {
        let _ = (wanted, services);
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: ActionSpec = ActionSpec {
        module: "input",
        name: "key",
        params: &[
            ParamSpec {
                name: "key",
                kind: ParamKind::Key,
                required: true,
            },
            ParamSpec {
                name: "times",
                kind: ParamKind::Integer,
                required: false,
            },
        ],
        ..ActionSpec::PLAIN
    };

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn a_required_parameter_missing_is_named() {
        assert_eq!(
            KEY.check(&table("times = 2")).unwrap_err(),
            "the parameter key is required"
        );
    }

    #[test]
    fn a_parameter_of_the_wrong_kind_is_named_with_what_it_should_be() {
        let reason = KEY.check(&table("key = 7")).unwrap_err();
        assert_eq!(
            reason,
            "the parameter key is not a key name such as KEY_1, or several joined with +"
        );
        assert_eq!(
            ParamKind::OneOf(&["up", "down"]).describe(),
            "one of up, down"
        );
    }

    #[test]
    fn an_unknown_parameter_is_named_with_the_action() {
        let reason = KEY
            .check(&table(
                r#"key = "KEY_1"
speed = 3"#,
            ))
            .unwrap_err();
        assert_eq!(reason, "the action input.key does not take speed");
    }

    #[test]
    fn a_chord_is_a_key_and_an_axis_is_not() {
        assert!(ParamKind::Key.accepts(&toml::Value::String("KEY_LEFTCTRL+KEY_A".into())));
        assert!(!ParamKind::Key.accepts(&toml::Value::String("ABS_X".into())));
        assert!(!ParamKind::Key.accepts(&toml::Value::String(String::new())));
        assert!(ParamKind::OneOf(&["up", "down"]).accepts(&toml::Value::String("up".into())));
        assert!(!ParamKind::OneOf(&["up", "down"]).accepts(&toml::Value::String("left".into())));
    }

    const ACTIVATE: ActionSpec = ActionSpec {
        module: "kwin",
        name: "window.activate",
        params: &[
            ParamSpec {
                name: "class",
                kind: ParamKind::Text,
                required: false,
            },
            ParamSpec {
                name: "title",
                kind: ParamKind::Text,
                required: false,
            },
        ],
        exactly_one_of: &["class", "title"],
        ..ActionSpec::PLAIN
    };

    #[test]
    fn exactly_one_of_a_group_is_given() {
        assert!(ACTIVATE.check(&table("class = \"codium\"")).is_ok());
        let reason = ACTIVATE.check(&table("")).unwrap_err();
        assert!(reason.contains("exactly one of class, title"), "{reason}");
        assert!(
            ACTIVATE
                .check(&table("class = \"a\"\ntitle = \"b\""))
                .is_err()
        );
    }

    #[test]
    fn a_duration_is_a_whole_number_above_zero() {
        let value = |text: &str| table(&format!("x = {text}"))["x"].clone();
        assert!(ParamKind::Millis.accepts(&value("1")));
        assert!(!ParamKind::Millis.accepts(&value("0")));
        assert!(!ParamKind::Millis.accepts(&value("-5")));
        assert!(!ParamKind::Millis.accepts(&value("1.5")));
    }
}
