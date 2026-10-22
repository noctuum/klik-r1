/// The presets built into the binary, by name.
pub const PRESETS: [(&str, &str); 3] = [
    ("reading", include_str!("../presets/reading.toml")),
    ("media", include_str!("../presets/media.toml")),
    ("presentation", include_str!("../presets/presentation.toml")),
];

/// What a preset is for: its first line, a comment.
pub fn describe(text: &str) -> &str {
    let first = text.lines().next().unwrap_or_default();
    first.strip_prefix("# ").unwrap_or(first)
}

/// The configuration with the profile of the preset `name` added at its end,
/// every other line of it as it was, comments and layout included. The
/// presets bind a device named `beauty-r1`, the name the example gives it.
pub fn apply(config: &str, name: &str) -> anyhow::Result<String> {
    let Some((_, preset)) = PRESETS.iter().find(|(known, _)| *known == name) else {
        let names: Vec<&str> = PRESETS.iter().map(|(known, _)| *known).collect();
        anyhow::bail!(
            "no preset named {name}; the presets are {}",
            names.join(", ")
        );
    };
    let table: toml::Table = config.parse()?;
    let has = |section: &str, key: &str| {
        table
            .get(section)
            .and_then(toml::Value::as_table)
            .is_some_and(|inner| inner.contains_key(key))
    };
    if !has("devices", "beauty-r1") {
        anyhow::bail!(
            "the presets bind a device named beauty-r1, and the configuration has no [devices.beauty-r1]"
        );
    }
    if has("profiles", name) {
        anyhow::bail!("the configuration already has a profile {name}");
    }
    let mut result = config.to_string();
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result.push('\n');
    result.push_str(preset);
    result.parse::<toml::Table>()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "[devices.beauty-r1]\nuniq = \"aa\"\n";

    /// `reading` switches desktops through `kwin`, so the presets are held
    /// to the default build.
    #[cfg(feature = "kwin")]
    #[test]
    fn each_preset_is_accepted_with_a_device() {
        for (name, text) in PRESETS {
            crate::daemon::assemble(&format!("{DEVICE}{text}"))
                .unwrap_or_else(|error| panic!("{name}: {error:#}"));
        }
    }

    #[test]
    fn applying_adds_the_profile_and_keeps_the_comments() {
        let mine = format!(
            "# mine\n{DEVICE}\n[profiles.one.bindings]\n\"beauty-r1.center:tap\" = {{ action = \"input.key\", key = \"KEY_1\" }}\n"
        );
        let result = apply(&mine, "media").unwrap();
        assert!(result.starts_with(&mine), "{result}");
        assert_eq!(
            klik_core::config::Config::from_toml(&result).unwrap().order,
            ["one", "media"]
        );
    }

    #[test]
    fn a_preset_whose_profile_exists_is_refused() {
        let text = format!("{DEVICE}[profiles.media]\n");
        let error = apply(&text, "media").unwrap_err().to_string();
        assert!(error.contains("already has a profile media"), "{error}");
    }

    #[test]
    fn a_configuration_without_the_device_is_refused() {
        let error = apply("[devices.other]\nuniq = \"aa\"\n", "media")
            .unwrap_err()
            .to_string();
        assert!(error.contains("[devices.beauty-r1]"), "{error}");
    }

    #[test]
    fn an_unknown_preset_names_the_presets() {
        let error = apply(DEVICE, "nope").unwrap_err().to_string();
        assert!(error.contains("reading, media, presentation"), "{error}");
    }

    #[test]
    fn a_preset_describes_itself_by_its_first_line() {
        assert_eq!(
            describe(PRESETS[0].1),
            "Scroll with Up and Down, switch desktops with Left and Right."
        );
    }
}
