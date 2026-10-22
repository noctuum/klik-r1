use std::io;

use evdev::{AttributeSet, KeyCode};

/// A configuration key name into the code the kernel uses. Only `KEY_` and
/// `BTN_` names pass; the kernel keeps both in one code space, and a name of
/// another kind would give this keyboard an axis.
pub fn code(name: &str) -> Option<KeyCode> {
    if !(name.starts_with("KEY_") || name.starts_with("BTN_")) {
        return None;
    }
    name.parse().ok()
}

/// The codes of every name, or the first name that is no key, so that the
/// daemon refuses to start with the name in its message rather than
/// declaring a keyboard without it.
pub fn resolve(names: &[String]) -> io::Result<AttributeSet<KeyCode>> {
    let mut set = AttributeSet::<KeyCode>::new();
    for name in names {
        let key = code(name).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("{name} is not a key"))
        })?;
        set.insert(key);
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kernel_name_resolves_and_an_invented_one_does_not() {
        assert_eq!(code("KEY_1"), Some(KeyCode::KEY_1));
        assert_eq!(code("BTN_LEFT"), Some(KeyCode::BTN_LEFT));
        assert_eq!(code("KEY_NOPE"), None);
        assert_eq!(code("ABS_X"), None, "an axis is not a key");
    }

    #[test]
    fn one_unknown_name_refuses_the_whole_set_by_name() {
        let names = ["KEY_1".to_string(), "KEY_NOPE".to_string()];
        let error = resolve(&names).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("KEY_NOPE"), "{error}");
        assert_eq!(resolve(&names[..1]).unwrap().iter().count(), 1);
    }
}
