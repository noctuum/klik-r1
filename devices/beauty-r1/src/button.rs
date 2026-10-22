#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    Center,
    Camera,
}

impl Button {
    pub fn from_config_name(name: &str) -> Option<Self> {
        match name {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "center" => Some(Self::Center),
            "camera" => Some(Self::Camera),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::Center => "center",
            Self::Camera => "camera",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_round_trip_all_variants() {
        let buttons = [
            Button::Up,
            Button::Down,
            Button::Left,
            Button::Right,
            Button::Center,
            Button::Camera,
        ];
        for button in &buttons {
            assert_eq!(
                Button::from_config_name(button.as_str()),
                Some(*button),
                "Round-trip failed for {}",
                button.as_str()
            );
        }
    }

    #[test]
    fn button_from_config_name_rejects_unknown() {
        assert_eq!(Button::from_config_name("middle"), None);
        assert_eq!(Button::from_config_name(""), None);
        assert_eq!(Button::from_config_name("CENTER"), None);
    }
}
