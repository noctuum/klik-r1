use std::collections::BTreeMap;
use std::fmt;

use crate::config::Rule;
use crate::module::Profiles;

/// What the program knows about the world at a press, by dotted name.
pub type Facts = BTreeMap<String, String>;

/// Whether `text` matches `pattern`, where `*` stands for any run of
/// characters and `?` for one. Case counts. A pattern with neither is an
/// equality.
pub fn glob(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    // The last star seen, and the position in the text it was tried from.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if let Some((at, from)) = star {
            p = at + 1;
            t = from + 1;
            star = Some((at, from + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// Why a profile is the active one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// The configuration has one profile and no rules.
    Only,
    /// Rule `number`, counted from 1, matched; no conditions is the default.
    Rule {
        number: usize,
        when: Vec<(String, String)>,
    },
    Manual,
    Held {
        device: String,
        button: String,
    },
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Why::Only => write!(f, "the one profile of the configuration"),
            Why::Rule { number, when } if when.is_empty() => {
                write!(f, "rule {number}, the default")
            }
            Why::Rule { number, when } => {
                let conditions: Vec<String> = when
                    .iter()
                    .map(|(fact, pattern)| format!("{fact} = {pattern}"))
                    .collect();
                write!(f, "rule {number}, {}", conditions.join(", "))
            }
            Why::Manual => write!(f, "set by hand"),
            Why::Held { device, button } => write!(f, "held on {device}.{button}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    device: String,
    button: String,
    profile: String,
}

/// Which profile is active and why: the latest hold, then a profile set by
/// hand, then the first rule that matches, then, with no rules, the first
/// profile of the file.
#[derive(Debug, Clone)]
pub struct Selection {
    order: Vec<String>,
    rules: Vec<Rule>,
    manual: Option<String>,
    held: Vec<Held>,
}

impl Selection {
    pub fn new(order: Vec<String>, rules: Vec<Rule>) -> Self {
        Self {
            order,
            rules,
            manual: None,
            held: Vec::new(),
        }
    }

    pub fn order(&self) -> &[String] {
        &self.order
    }

    pub fn knows(&self, name: &str) -> bool {
        self.order.iter().any(|known| known == name)
    }

    /// Every fact the rules read, each once, in the order of first use.
    /// Empty while a hold or a profile set by hand decides, since the rules
    /// are not consulted then and asking the desktop would be wasted.
    pub fn needs_facts(&self) -> Vec<String> {
        if !self.held.is_empty() || self.manual.is_some() {
            return Vec::new();
        }
        let mut facts: Vec<String> = Vec::new();
        for (fact, _) in self.rules.iter().flat_map(|rule| &rule.when) {
            if !facts.contains(fact) {
                facts.push(fact.clone());
            }
        }
        facts
    }

    pub fn active(&self, facts: &Facts) -> (String, Why) {
        if let Some(held) = self.held.last() {
            return (
                held.profile.clone(),
                Why::Held {
                    device: held.device.clone(),
                    button: held.button.clone(),
                },
            );
        }
        if let Some(name) = &self.manual {
            return (name.clone(), Why::Manual);
        }
        for (index, rule) in self.rules.iter().enumerate() {
            let holds = rule
                .when
                .iter()
                .all(|(fact, pattern)| facts.get(fact).is_some_and(|value| glob(pattern, value)));
            if holds {
                return (
                    rule.profile.clone(),
                    Why::Rule {
                        number: index + 1,
                        when: rule.when.clone(),
                    },
                );
            }
        }
        (self.order.first().cloned().unwrap_or_default(), Why::Only)
    }

    /// The holds of a device that went away, whose ends will never come.
    pub fn release_device(&mut self, device: &str) {
        self.held.retain(|held| held.device != device);
    }
}

impl Profiles for Selection {
    fn set(&mut self, name: &str) {
        self.manual = Some(name.to_string());
    }

    fn next_after(&mut self, current: &str) {
        let next = match self.order.iter().position(|name| name == current) {
            Some(index) => self.order[(index + 1) % self.order.len()].clone(),
            None => self.order.first().cloned().unwrap_or_default(),
        };
        self.manual = Some(next);
    }

    fn auto(&mut self) {
        self.manual = None;
    }

    fn hold(&mut self, device: &str, button: &str, name: &str) {
        self.release(device, button);
        self.held.push(Held {
            device: device.to_string(),
            button: button.to_string(),
            profile: name.to_string(),
        });
    }

    fn release(&mut self, device: &str, button: &str) {
        self.held
            .retain(|held| held.device != device || held.button != button);
    }
}

/// The profile each gesture in flight was looked up in, so that its ticks
/// and its end go to the same bindings as its press, whatever came to the
/// front meanwhile.
#[derive(Debug, Default)]
pub struct Pins {
    pins: Vec<(String, String, String)>,
}

impl Pins {
    pub fn pin(&mut self, device: &str, button: &str, profile: &str) {
        self.unpin(device, button);
        self.pins
            .push((device.to_string(), button.to_string(), profile.to_string()));
    }

    pub fn of(&self, device: &str, button: &str) -> Option<&str> {
        self.pins
            .iter()
            .find(|(d, b, _)| d == device && b == button)
            .map(|(_, _, profile)| profile.as_str())
    }

    pub fn unpin(&mut self, device: &str, button: &str) {
        self.pins.retain(|(d, b, _)| d != device || b != button);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Selection {
        Selection::new(
            vec![
                "reading".to_string(),
                "media".to_string(),
                "presentation".to_string(),
            ],
            vec![
                Rule {
                    when: vec![("window.class".to_string(), "codium".to_string())],
                    profile: "reading".to_string(),
                },
                Rule {
                    when: vec![],
                    profile: "media".to_string(),
                },
            ],
        )
    }

    fn facts(pairs: &[(&str, &str)]) -> Facts {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_glob_matches_runs_and_single_characters_with_case() {
        assert!(glob("codium", "codium"));
        assert!(!glob("codium", "codium2"));
        assert!(glob("*klik*", "VSCodium - klik"));
        assert!(glob("?odium", "codium"));
        assert!(!glob("?odium", "odium"));
        assert!(glob("*", ""));
        assert!(!glob("Codium", "codium"));
        assert!(glob("a*b*c", "aXbYc"));
        assert!(!glob("a*b", "acb_"));
    }

    #[test]
    fn the_first_rule_that_matches_decides() {
        let s = rules();
        assert_eq!(s.active(&facts(&[("window.class", "codium")])).0, "reading");
        assert_eq!(s.active(&facts(&[("window.class", "firefox")])).0, "media");
    }

    /// A window the desktop could not name leaves its facts absent, and the
    /// press goes to the default rather than nowhere.
    #[test]
    fn a_fact_with_no_value_matches_nothing() {
        let any_title = Selection::new(
            vec!["titled".to_string(), "rest".to_string()],
            vec![
                Rule {
                    when: vec![("window.title".to_string(), "*".to_string())],
                    profile: "titled".to_string(),
                },
                Rule {
                    when: vec![],
                    profile: "rest".to_string(),
                },
            ],
        );
        assert_eq!(any_title.active(&Facts::new()).0, "rest");
        assert_eq!(
            rules().active(&Facts::new()),
            (
                "media".to_string(),
                Why::Rule {
                    number: 2,
                    when: vec![]
                }
            )
        );
    }

    #[test]
    fn with_no_rules_the_first_profile_is_active() {
        let s = Selection::new(vec!["one".to_string()], vec![]);
        assert_eq!(s.active(&Facts::new()), ("one".to_string(), Why::Only));
    }

    #[test]
    fn a_profile_set_by_hand_beats_the_rules_until_auto() {
        let mut s = rules();
        s.set("presentation");
        assert_eq!(
            s.active(&facts(&[("window.class", "codium")])),
            ("presentation".to_string(), Why::Manual)
        );
        s.auto();
        assert_eq!(s.active(&facts(&[("window.class", "codium")])).0, "reading");
    }

    #[test]
    fn the_latest_hold_wins_and_its_release_uncovers_the_one_before() {
        let mut s = rules();
        s.set("presentation");
        s.hold("beauty-r1", "camera", "media");
        s.hold("beauty-r1", "left", "reading");
        assert_eq!(s.active(&Facts::new()).0, "reading");
        s.release("beauty-r1", "left");
        assert_eq!(
            s.active(&Facts::new()),
            (
                "media".to_string(),
                Why::Held {
                    device: "beauty-r1".to_string(),
                    button: "camera".to_string()
                }
            )
        );
        s.release("beauty-r1", "camera");
        assert_eq!(s.active(&Facts::new()).0, "presentation");
    }

    #[test]
    fn next_steps_around_the_end_of_the_order() {
        let mut s = rules();
        s.next_after("presentation");
        assert_eq!(
            s.active(&Facts::new()),
            ("reading".to_string(), Why::Manual)
        );
        s.next_after("reading");
        assert_eq!(s.active(&Facts::new()).0, "media");
    }

    /// A remote that goes away in the middle of a hold never sends the end
    /// that would release it.
    #[test]
    fn the_device_going_away_releases_its_holds() {
        let mut s = rules();
        s.hold("beauty-r1", "camera", "presentation");
        s.release_device("beauty-r1");
        assert_eq!(s.active(&Facts::new()).0, "media");
    }

    #[test]
    fn facts_are_needed_only_while_the_rules_decide() {
        let mut s = rules();
        assert_eq!(s.needs_facts(), ["window.class"]);
        s.set("media");
        assert!(s.needs_facts().is_empty());
        s.auto();
        s.hold("beauty-r1", "camera", "media");
        assert!(s.needs_facts().is_empty());
    }

    #[test]
    fn why_says_which_rule_and_what_it_read() {
        let s = rules();
        assert_eq!(
            s.active(&facts(&[("window.class", "codium")]))
                .1
                .to_string(),
            "rule 1, window.class = codium"
        );
        assert_eq!(s.active(&Facts::new()).1.to_string(), "rule 2, the default");
        assert_eq!(
            Why::Only.to_string(),
            "the one profile of the configuration"
        );
        assert_eq!(Why::Manual.to_string(), "set by hand");
        assert_eq!(
            Why::Held {
                device: "beauty-r1".to_string(),
                button: "camera".to_string()
            }
            .to_string(),
            "held on beauty-r1.camera"
        );
    }

    #[test]
    fn a_gesture_keeps_the_profile_of_its_press() {
        let mut pins = Pins::default();
        pins.pin("beauty-r1", "up", "reading");
        pins.pin("beauty-r1", "camera", "media");
        assert_eq!(pins.of("beauty-r1", "up"), Some("reading"));
        pins.unpin("beauty-r1", "up");
        assert_eq!(pins.of("beauty-r1", "up"), None);
        assert_eq!(pins.of("beauty-r1", "camera"), Some("media"));
    }
}
