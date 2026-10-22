use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use klik_core::config::Binding;
use klik_core::module::Journal;
use klik_core::press::Recognized;

/// Whether the journal carries what every press came to: `[debug] enabled` of
/// the configuration, set once when the daemon starts.
static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn set_debug(enabled: bool) {
    DEBUG.store(enabled, Ordering::Relaxed);
}

/// The journal of the daemon: standard error, which the user service hands to
/// journald. A plain line goes always: the start and the stop, a device coming
/// and going, a fault. A debug line goes in the debug mode alone: what each
/// press came to.
pub struct Stderr;

impl Journal for Stderr {
    fn line(&mut self, text: &str) {
        eprintln!("{text}");
    }

    fn debug(&mut self, text: &str) {
        if DEBUG.load(Ordering::Relaxed) {
            eprintln!("{text}");
        }
    }
}

/// The time of day in UTC, to the millisecond, as the journal dates a press.
/// The date is left out: a line is read within the day it was written.
pub fn clock(epoch_s: f64) -> String {
    let ms = (epoch_s * 1000.0) as u64;
    let s = ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}Z",
        s / 3600 % 24,
        s / 60 % 60,
        s % 60,
        ms % 1000
    )
}

/// What was pressed, as the journal names it. A refused press never became a
/// tap or a hold, so it is named a press.
pub fn pressed(recognized: &Recognized) -> String {
    match recognized {
        Recognized::Press(button, kind) => format!("{button} {}", kind.as_str()),
        Recognized::Tick(button) => format!("{button} tick"),
        Recognized::End(button) => format!("{button} end"),
        Recognized::Refused { button, .. } => format!("{button} press"),
        Recognized::Unmatched(_) => "unmatched".to_string(),
    }
}

/// What a line of the journal about one press begins with: when it was
/// pressed, on which device, and what.
pub fn label(now: f64, device: &str, recognized: &Recognized) -> String {
    format!("{} {device} {}", clock(now), pressed(recognized))
}

/// What the journal says about a press once it is done: the action it came
/// to, and how long the daemon took.
pub fn came_to_line(binding: Option<&Binding>, took: Duration) -> String {
    let what = binding.map_or_else(
        || "nothing".to_string(),
        |binding| format!("{}.{}", binding.module, binding.action),
    );
    format!("came to {what}, {} ms", took.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use klik_core::press::PressKind;

    #[test]
    fn the_clock_is_the_time_of_day_in_utc() {
        assert_eq!(clock(1790684820.976), "12:27:00.976Z");
    }

    #[test]
    fn the_clock_starts_a_day_at_midnight() {
        assert_eq!(clock(0.0), "00:00:00.000Z");
        assert_eq!(clock(86399.9994), "23:59:59.999Z");
        assert_eq!(clock(86400.0), "00:00:00.000Z");
    }

    #[test]
    fn a_press_is_named_by_its_button_and_its_kind() {
        assert_eq!(
            pressed(&Recognized::Press("center", PressKind::Tap)),
            "center tap"
        );
        assert_eq!(
            pressed(&Recognized::Press("camera", PressKind::Hold)),
            "camera hold"
        );
    }

    #[test]
    fn a_tick_an_end_and_an_unmatched_gesture_have_a_name_too() {
        assert_eq!(pressed(&Recognized::Tick("up")), "up tick");
        assert_eq!(pressed(&Recognized::End("up")), "up end");
        assert_eq!(
            pressed(&Recognized::Unmatched("x=None y=None".to_string())),
            "unmatched"
        );
    }

    #[test]
    fn a_refused_press_is_named_a_press() {
        let refused = Recognized::Refused {
            button: "camera",
            after_ms: 60,
        };
        assert_eq!(pressed(&refused), "camera press");
    }

    #[test]
    fn a_line_begins_with_the_time_the_device_and_the_press() {
        let press = Recognized::Press("camera", PressKind::Tap);
        assert_eq!(
            label(1790684820.976, "beauty-r1", &press),
            "12:27:00.976Z beauty-r1 camera tap"
        );
    }

    #[test]
    fn a_press_is_named_by_what_it_came_to() {
        let binding = Binding {
            module: "input".to_string(),
            action: "wheel".to_string(),
            params: toml::Table::new(),
        };
        assert_eq!(
            came_to_line(Some(&binding), Duration::from_millis(3)),
            "came to input.wheel, 3 ms"
        );
        assert_eq!(came_to_line(None, Duration::ZERO), "came to nothing, 0 ms");
    }
}
