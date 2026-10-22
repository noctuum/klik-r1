use std::io;

use crate::error::ConfigError;
use crate::platform::Injector;
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
}

impl ActionSpec {
    /// Whether these parameters fit the action: every required one present,
    /// none unknown, each of its kind. The reason names the parameter.
    pub fn check(&self, params: &toml::Table) -> Result<(), String> {
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

/// What a desktop module gives. None of it exists in this plan; the type
/// is here so that `input` can ask and be told there is no desktop.
pub trait Desktop {
    fn front(&mut self) -> io::Result<Option<Window>>;
    fn screen(&mut self) -> io::Result<Rect>;
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
}
