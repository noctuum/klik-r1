use std::collections::BTreeMap;

use serde::Deserialize;

use crate::device::{ButtonSpec, button_spec};
use crate::error::ConfigError;
use crate::module::{ActionSpec, ParamKind};
use crate::press::PressKind;

#[derive(Debug, Clone, Deserialize)]
struct RawDevice {
    profile: Option<String>,
    #[serde(flatten)]
    settings: toml::Table,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProfile {
    extends: Option<String>,
    #[serde(default)]
    bindings: BTreeMap<String, RawBinding>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawBinding {
    action: String,
    #[serde(flatten)]
    params: toml::Table,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DebugConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    devices: BTreeMap<String, RawDevice>,
    #[serde(default)]
    profiles: BTreeMap<String, RawProfile>,
    #[serde(default)]
    modules: BTreeMap<String, toml::Table>,
    #[serde(default)]
    debug: DebugConfig,
}

/// One `[devices.<id>]` section. `profile` names the `ProfileKind` that
/// builds it and defaults to the id of the section.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceSection {
    pub profile: String,
    pub settings: toml::Table,
}

/// `device.button:kind`, the key of a binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BindingKey {
    pub device: String,
    pub button: String,
    pub kind: PressKind,
}

impl BindingKey {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let bad = || ConfigError::BadBindingKey(text.to_string());
        let (rest, kind) = text.split_once(':').ok_or_else(bad)?;
        let (device, button) = rest.split_once('.').ok_or_else(bad)?;
        if device.is_empty() || button.is_empty() {
            return Err(bad());
        }
        let kind = PressKind::from_config_name(kind).ok_or_else(bad)?;
        Ok(Self {
            device: device.to_string(),
            button: button.to_string(),
            kind,
        })
    }
}

impl std::fmt::Display for BindingKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}:{}", self.device, self.button, self.kind.as_str())
    }
}

/// What a button does: `module.action` and the parameters the action takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub module: String,
    pub action: String,
    pub params: toml::Table,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileSection {
    pub extends: Option<String>,
    pub bindings: BTreeMap<BindingKey, Binding>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub devices: BTreeMap<String, DeviceSection>,
    pub profiles: BTreeMap<String, ProfileSection>,
    pub modules: BTreeMap<String, toml::Table>,
    pub debug: DebugConfig,
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(text)?;
        if raw.devices.is_empty() {
            return Err(ConfigError::NoDevices);
        }
        let devices = raw
            .devices
            .into_iter()
            .map(|(id, device)| {
                let profile = device.profile.unwrap_or_else(|| id.clone());
                (
                    id,
                    DeviceSection {
                        profile,
                        settings: device.settings,
                    },
                )
            })
            .collect();
        let mut profiles = BTreeMap::new();
        for (name, profile) in raw.profiles {
            let mut bindings = BTreeMap::new();
            for (key, binding) in profile.bindings {
                let key = BindingKey::parse(&key)?;
                let (module, action) = binding
                    .action
                    .split_once('.')
                    .filter(|(module, action)| !module.is_empty() && !action.is_empty())
                    .ok_or_else(|| ConfigError::BadAction {
                        key: key.to_string(),
                        action: binding.action.clone(),
                    })?;
                bindings.insert(
                    key,
                    Binding {
                        module: module.to_string(),
                        action: action.to_string(),
                        params: binding.params,
                    },
                );
            }
            profiles.insert(
                name,
                ProfileSection {
                    extends: profile.extends,
                    bindings,
                },
            );
        }
        for (name, profile) in &profiles {
            if let Some(parent) = &profile.extends
                && !profiles.contains_key(parent)
            {
                return Err(ConfigError::UnknownParent {
                    profile: name.clone(),
                    parent: parent.clone(),
                });
            }
        }
        Ok(Self {
            devices,
            profiles,
            modules: raw.modules,
            debug: raw.debug,
        })
    }
}

/// The profiles of bindings, checked against the devices and the actions,
/// with the active one chosen. With no rules yet, the one profile of the
/// configuration is the active one, and two or more are refused.
#[derive(Debug, Clone)]
pub struct Layout {
    profiles: BTreeMap<String, ProfileSection>,
    active: String,
}

impl Layout {
    pub fn new(
        config: &Config,
        buttons: &dyn Fn(&str) -> Option<&'static [ButtonSpec]>,
        actions: &[ActionSpec],
    ) -> Result<Self, ConfigError> {
        let mut names = config.profiles.keys();
        let active = match (names.next(), names.next()) {
            (Some(one), None) => one.clone(),
            (None, _) => return Err(ConfigError::NoProfiles),
            (Some(_), Some(_)) => return Err(ConfigError::SeveralProfilesNoRules),
        };
        for profile in config.profiles.values() {
            check_chain(&config.profiles, profile)?;
            for (key, binding) in &profile.bindings {
                let Some(specs) = buttons(&key.device) else {
                    return Err(ConfigError::UnknownDevice {
                        key: key.to_string(),
                    });
                };
                let Some(button) = button_spec(specs, &key.button) else {
                    return Err(ConfigError::UnknownButton {
                        key: key.to_string(),
                    });
                };
                if !button.produces(key.kind) {
                    return Err(ConfigError::KindNotProduced {
                        key: key.to_string(),
                    });
                }
                let Some(action) = actions
                    .iter()
                    .find(|spec| spec.module == binding.module && spec.name == binding.action)
                else {
                    return Err(ConfigError::UnknownAction {
                        key: key.to_string(),
                        action: format!("{}.{}", binding.module, binding.action),
                    });
                };
                action
                    .check(&binding.params)
                    .map_err(|reason| ConfigError::BadParams {
                        key: key.to_string(),
                        reason,
                    })?;
            }
        }
        Ok(Self {
            profiles: config.profiles.clone(),
            active,
        })
    }

    pub fn active(&self) -> &str {
        &self.active
    }

    /// The binding of the active profile, or of the first parent that has
    /// one, or none.
    pub fn lookup(&self, device: &str, button: &str, kind: PressKind) -> Option<&Binding> {
        let key = BindingKey {
            device: device.to_string(),
            button: button.to_string(),
            kind,
        };
        let mut name = Some(self.active.as_str());
        while let Some(current) = name {
            let profile = self.profiles.get(current)?;
            if let Some(binding) = profile.bindings.get(&key) {
                return Some(binding);
            }
            name = profile.extends.as_deref();
        }
        None
    }

    /// Every key name a binding of any profile carries in a parameter of
    /// kind `Key`, split at `+`, for the injector to declare once. A name is
    /// listed once.
    pub fn key_names(&self, actions: &[ActionSpec]) -> Vec<String> {
        let mut names = Vec::new();
        for profile in self.profiles.values() {
            for binding in profile.bindings.values() {
                let Some(spec) = actions
                    .iter()
                    .find(|spec| spec.module == binding.module && spec.name == binding.action)
                else {
                    continue;
                };
                for param in spec.params.iter().filter(|p| p.kind == ParamKind::Key) {
                    if let Some(value) =
                        binding.params.get(param.name).and_then(toml::Value::as_str)
                    {
                        for name in value.split('+') {
                            if !names.iter().any(|known| known == name) {
                                names.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
        names
    }
}

/// A chain of `extends` that comes back to a profile it passed is refused:
/// a lookup would walk it forever.
fn check_chain(
    profiles: &BTreeMap<String, ProfileSection>,
    start: &ProfileSection,
) -> Result<(), ConfigError> {
    let mut seen = Vec::new();
    let mut next = start.extends.as_deref();
    while let Some(name) = next {
        if seen.contains(&name) {
            return Err(ConfigError::ExtendsCycle(name.to_string()));
        }
        seen.push(name);
        next = profiles
            .get(name)
            .and_then(|profile| profile.extends.as_deref());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::ParamSpec;

    const BUTTONS: [ButtonSpec; 2] = [
        ButtonSpec {
            name: "up",
            tap: true,
            hold: true,
            ticks: true,
        },
        ButtonSpec {
            name: "center",
            tap: true,
            hold: false,
            ticks: false,
        },
    ];

    fn buttons(device: &str) -> Option<&'static [ButtonSpec]> {
        (device == "beauty-r1").then_some(&BUTTONS[..])
    }

    const ACTIONS: [ActionSpec; 2] = [
        ActionSpec {
            module: "input",
            name: "key",
            params: &[ParamSpec {
                name: "key",
                kind: ParamKind::Key,
                required: true,
            }],
        },
        ActionSpec {
            module: "input",
            name: "wheel",
            params: &[ParamSpec {
                name: "direction",
                kind: ParamKind::OneOf(&["up", "down"]),
                required: true,
            }],
        },
    ];

    const SAMPLE: &str = r#"
        [devices.beauty-r1]
        uniq = "aa:bb:cc:dd:ee:01"

        [profiles.reading.bindings]
        "beauty-r1.up:tap" = { action = "input.wheel", direction = "up" }
        "beauty-r1.up:hold" = { action = "input.wheel", direction = "up" }
        "beauty-r1.center:tap" = { action = "input.key", key = "KEY_LEFTCTRL+KEY_A" }

        [modules.input]
        hi_res_per_event = 40
    "#;

    fn layout(text: &str) -> Result<Layout, ConfigError> {
        Layout::new(&Config::from_toml(text)?, &buttons, &ACTIONS)
    }

    #[test]
    fn a_binding_key_is_device_button_and_kind() {
        let key = BindingKey::parse("beauty-r1.up:hold").unwrap();
        assert_eq!(key.device, "beauty-r1");
        assert_eq!(key.button, "up");
        assert_eq!(key.kind, PressKind::Hold);
        assert_eq!(key.to_string(), "beauty-r1.up:hold");
        for bad in [
            "up:tap",
            "beauty-r1.up",
            "beauty-r1.up:double",
            ".up:tap",
            "beauty-r1.:tap",
        ] {
            assert!(
                matches!(BindingKey::parse(bad), Err(ConfigError::BadBindingKey(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_sample_loads_with_the_device_named_after_its_section() {
        let config = Config::from_toml(SAMPLE).unwrap();
        assert_eq!(config.devices["beauty-r1"].profile, "beauty-r1");
        assert_eq!(
            config.devices["beauty-r1"].settings["uniq"].as_str(),
            Some("aa:bb:cc:dd:ee:01")
        );
        assert_eq!(
            config.modules["input"]["hi_res_per_event"].as_integer(),
            Some(40)
        );
        assert!(!config.debug.enabled);
    }

    #[test]
    fn a_device_may_name_another_profile_than_its_section() {
        let config = Config::from_toml(
            r#"
            [devices.left]
            profile = "beauty-r1"
            uniq = "aa"
            [devices.right]
            uniq = "bb"
            "#,
        )
        .unwrap();
        assert_eq!(config.devices["left"].profile, "beauty-r1");
        assert!(config.devices["left"].settings.get("profile").is_none());
        assert_eq!(
            config.devices["right"].profile, "right",
            "no key is the name of the section"
        );
    }

    #[test]
    fn a_configuration_without_a_device_is_refused() {
        assert!(matches!(
            Config::from_toml("[profiles.a]"),
            Err(ConfigError::NoDevices)
        ));
    }

    #[test]
    fn an_action_without_a_module_is_refused_by_key() {
        let text = r#"
            [devices.beauty-r1]
            uniq = "aa"
            [profiles.a.bindings]
            "beauty-r1.up:tap" = { action = "wheel" }
        "#;
        let error = Config::from_toml(text).unwrap_err();
        assert!(
            matches!(error, ConfigError::BadAction { ref key, .. } if key == "beauty-r1.up:tap")
        );
    }

    #[test]
    fn the_one_profile_is_active_and_its_bindings_resolve() {
        let layout = layout(SAMPLE).unwrap();
        assert_eq!(layout.active(), "reading");
        let binding = layout.lookup("beauty-r1", "up", PressKind::Hold).unwrap();
        assert_eq!(binding.module, "input");
        assert_eq!(binding.action, "wheel");
        assert_eq!(binding.params["direction"].as_str(), Some("up"));
        assert!(
            layout
                .lookup("beauty-r1", "center", PressKind::Hold)
                .is_none()
        );
    }

    /// Two profiles are legal only once rules choose between them, so the
    /// layout here is built directly, with `reading` active over `base`.
    #[test]
    fn a_lookup_walks_the_parents_and_stops_at_the_first_binding() {
        let config = Config::from_toml(
            r#"
            [devices.beauty-r1]
            uniq = "aa"
            [profiles.base.bindings]
            "beauty-r1.up:tap" = { action = "input.wheel", direction = "up" }
            "beauty-r1.center:tap" = { action = "input.key", key = "KEY_1" }
            [profiles.reading]
            extends = "base"
            [profiles.reading.bindings]
            "beauty-r1.center:tap" = { action = "input.key", key = "KEY_2" }
            "#,
        )
        .unwrap();
        let layout = Layout {
            profiles: config.profiles,
            active: "reading".to_string(),
        };
        let center = layout
            .lookup("beauty-r1", "center", PressKind::Tap)
            .unwrap();
        assert_eq!(
            center.params["key"].as_str(),
            Some("KEY_2"),
            "the active profile first"
        );
        let up = layout.lookup("beauty-r1", "up", PressKind::Tap).unwrap();
        assert_eq!(up.action, "wheel", "then its parent");
        assert!(layout.lookup("beauty-r1", "up", PressKind::Hold).is_none());
    }

    /// The error names the rule to write.
    #[test]
    fn two_profiles_without_rules_are_refused() {
        let layout = Layout::new(
            &Config::from_toml(
                r#"
                [devices.beauty-r1]
                uniq = "aa"
                [profiles.base.bindings]
                "beauty-r1.up:tap" = { action = "input.wheel", direction = "up" }
                [profiles.reading]
                extends = "base"
                "#,
            )
            .unwrap(),
            &buttons,
            &ACTIONS,
        );
        assert!(matches!(layout, Err(ConfigError::SeveralProfilesNoRules)));
    }

    #[test]
    fn a_binding_to_a_button_the_device_lacks_is_refused_by_key() {
        let text = SAMPLE.replace("beauty-r1.center:tap", "beauty-r1.ok:tap");
        let error = layout(&text).unwrap_err();
        assert!(
            matches!(error, ConfigError::UnknownButton { ref key } if key == "beauty-r1.ok:tap")
        );
    }

    #[test]
    fn a_hold_of_a_button_without_one_is_refused_by_key() {
        let text = SAMPLE.replace("beauty-r1.center:tap", "beauty-r1.center:hold");
        let error = layout(&text).unwrap_err();
        assert!(
            matches!(error, ConfigError::KindNotProduced { ref key } if key == "beauty-r1.center:hold")
        );
    }

    #[test]
    fn a_binding_of_an_unknown_device_is_refused_by_key() {
        let text = SAMPLE.replace("beauty-r1.up:tap", "other.up:tap");
        let error = layout(&text).unwrap_err();
        assert!(matches!(error, ConfigError::UnknownDevice { ref key } if key == "other.up:tap"));
    }

    #[test]
    fn an_action_nobody_declares_is_refused_by_key_and_name() {
        let text = SAMPLE.replace("input.wheel", "input.spin");
        let error = layout(&text).unwrap_err();
        assert!(
            matches!(error, ConfigError::UnknownAction { ref action, .. } if action == "input.spin")
        );
    }

    #[test]
    fn a_parameter_the_action_does_not_take_is_refused_by_key() {
        let text = SAMPLE.replace(r#"direction = "up" }"#, r#"direction = "up", speed = 3 }"#);
        let error = layout(&text).unwrap_err();
        assert!(
            matches!(error, ConfigError::BadParams { ref reason, .. } if reason.contains("speed"))
        );
    }

    #[test]
    fn the_key_names_of_a_layout_are_split_at_plus_and_listed_once() {
        let layout = layout(SAMPLE).unwrap();
        assert_eq!(layout.key_names(&ACTIONS), vec!["KEY_LEFTCTRL", "KEY_A"]);
    }

    #[test]
    fn a_cycle_of_parents_is_refused() {
        let mut profiles = BTreeMap::new();
        for (name, parent) in [("a", "b"), ("b", "a")] {
            profiles.insert(
                name.to_string(),
                ProfileSection {
                    extends: Some(parent.to_string()),
                    bindings: BTreeMap::new(),
                },
            );
        }
        assert!(matches!(
            check_chain(&profiles, &profiles["a"]),
            Err(ConfigError::ExtendsCycle(_))
        ));
    }

    #[test]
    fn an_unknown_parent_is_refused_by_both_names() {
        let text = r#"
            [devices.beauty-r1]
            uniq = "aa"
            [profiles.a]
            extends = "missing"
        "#;
        let error = Config::from_toml(text).unwrap_err();
        assert!(
            matches!(error, ConfigError::UnknownParent { ref profile, ref parent } if profile == "a" && parent == "missing")
        );
    }
}
