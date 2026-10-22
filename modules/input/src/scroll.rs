use serde::Deserialize;

/// How long a scroll series may go quiet before the cursor is aimed again.
///
/// A held Up or Down delivers about 33 events per second, measured on
/// 2026-09-16, so roughly 30 ms apart. The threshold has to clear that with
/// room, or a hold would re-aim in the middle of itself and jerk the cursor
/// back to the same point.
pub const SERIES_GAP_MS: u32 = 200;

const _: () = assert!(SERIES_GAP_MS > 30, "a held button sends every 30 ms");

/// Which way the wheel turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// Tracks whether the wheel events still arriving belong to the same series.
///
/// The cursor is placed once per series rather than once per event, because a
/// wheel event goes to the window under the pointer and aiming is the expensive
/// part: every series asks the desktop for the window in front and moves the
/// cursor once.
#[derive(Debug, Default)]
pub struct Series {
    last: Option<f64>,
}

impl Series {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the cursor has to be aimed before this event is emitted.
    ///
    /// An event dated before the last one also starts a new series. evdev
    /// timestamps are `CLOCK_REALTIME`, which NTP steps and resume from suspend
    /// move in both directions, and treating a backward step as "still the same
    /// series" would leave the cursor unaimed until the clock caught up.
    pub fn needs_aim(&self, now: f64) -> bool {
        match self.last {
            None => true,
            Some(last) => {
                // Compared as `now` against `last + threshold` rather than
                // `now - last` against `threshold`. The two are not
                // numerically identical: floating point addition and
                // subtraction do not round-trip, so a timestamp built as
                // `last + threshold` can subtract back to slightly more than
                // `threshold`, wrongly starting a new series at the boundary.
                let boundary = last + f64::from(SERIES_GAP_MS) / 1000.0;
                now < last || now > boundary
            }
        }
    }

    /// Records that an event of the current series was emitted at this time.
    pub fn note(&mut self, now: f64) {
        self.last = Some(now);
    }
}

/// The section `[modules.input]`: the speed of the wheel and where the cursor
/// is aimed. Every key has a default, so the section may be left out.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default)]
pub struct ScrollSettings {
    pub hi_res_per_event: u32,
    /// Added to the rate after every `events_per_step` events of one gesture.
    /// Zero turns acceleration off and is deliberately accepted.
    pub hi_res_step: u32,
    pub events_per_step: u32,
    pub hi_res_max: u32,
    pub aim_x: f64,
    pub aim_y: f64,
}

/// The rate and the curve measured on 2026-09-16 and 2026-09-24, and the aim
/// point of 2026-10-12: 15 px from the right edge of a window 1920 px wide, in
/// its upper third, where the cursor rests on no text. Another width needs
/// another fraction.
impl Default for ScrollSettings {
    fn default() -> Self {
        Self {
            hi_res_per_event: 40,
            hi_res_step: 40,
            events_per_step: 10,
            hi_res_max: 120,
            aim_x: 0.992,
            aim_y: 0.3,
        }
    }
}

impl ScrollSettings {
    pub fn from_table(table: &toml::Table) -> Result<Self, String> {
        toml::Value::Table(table.clone())
            .try_into::<Self>()
            .map_err(|error| error.to_string())
    }

    /// The settings that would leave the wheel dead or aimed outside the
    /// window, refused with the key in the reason.
    pub fn check(&self) -> Result<(), String> {
        if self.hi_res_per_event == 0 {
            return Err("hi_res_per_event is 0, so every scroll would emit nothing".to_string());
        }
        if self.events_per_step == 0 {
            return Err(
                "events_per_step is 0, so no event could be counted into a step".to_string(),
            );
        }
        if self.hi_res_max < self.hi_res_per_event {
            return Err(format!(
                "hi_res_max is {} and hi_res_per_event is {}, so the ceiling is below the rate it caps",
                self.hi_res_max, self.hi_res_per_event
            ));
        }
        // 0.0 is the window's own first pixel and 1.0 is the first pixel past
        // its far edge, so the range is closed below and open above. A value
        // outside it aims at a point clamped to a screen edge, and the wheel
        // then turns whatever window sits there. `contains` refuses a NaN as
        // well, which would otherwise aim at the window's origin.
        for (field, value) in [("aim_x", self.aim_x), ("aim_y", self.aim_y)] {
            if !(0.0..1.0).contains(&value) {
                return Err(format!(
                    "{field} is {value}, and an aim fraction runs from 0.0 up to but not including 1.0"
                ));
            }
        }
        Ok(())
    }
}

/// How many high resolution wheel units the `event`th event of one gesture is
/// worth, counting the press as 1.
///
/// The rate rises by `hi_res_step` after every `events_per_step` events and
/// stops at `hi_res_max`. A tap delivers about ten events over 270 to 300 ms,
/// so with the shipped defaults the whole of a tap sits in the first block and
/// a tap is unchanged; a hold delivers about 33 events a second and reaches the
/// ceiling after 0.9 s. Measured on 2026-09-16 and 2026-09-24.
///
/// `events_per_step` is never 0 here: `ScrollSettings::check` refuses it. The
/// `max(1)` keeps the function total rather than guarding a case that can
/// arrive.
pub fn rate(event: u32, scroll: &ScrollSettings) -> u32 {
    let steps = event.saturating_sub(1) / scroll.events_per_step.max(1);
    scroll
        .hi_res_per_event
        .saturating_add(scroll.hi_res_step.saturating_mul(steps))
        .min(scroll.hi_res_max)
}

/// How many high resolution wheel units one received axis event is worth.
///
/// `rate` is what `rate` computed for this event. 120 units are one notch, so
/// it is usually a fraction of a notch, and the pointer keeps the remainder.
pub fn units(direction: Direction, rate: u32) -> i32 {
    let rate = i32::try_from(rate).unwrap_or(i32::MAX);
    match direction {
        Direction::Up => rate,
        Direction::Down => -rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_event_of_a_session_needs_the_cursor_aimed() {
        let series = Series::new();
        assert!(series.needs_aim(100.0));
    }

    /// The cursor is placed once per series, not once per event. A held Up
    /// delivers about 33 events per second, and aiming on each one would jerk
    /// the cursor back to the same point a hundred times in three seconds.
    #[test]
    fn an_event_inside_the_series_does_not_aim_again() {
        let mut series = Series::new();
        series.note(100.0);
        assert!(!series.needs_aim(100.030));
    }

    /// A pause of a second is a new series. Someone who stops for that long has
    /// stopped, and what they scroll next may be a different window, so the
    /// cursor has to be aimed again rather than left where the last series put it.
    #[test]
    fn a_pause_of_a_second_starts_a_new_series() {
        let mut series = Series::new();
        series.note(100.0);
        assert!(series.needs_aim(101.0));
    }

    #[test]
    fn a_gap_longer_than_the_threshold_starts_a_new_series() {
        let mut series = Series::new();
        series.note(100.0);
        let after = 100.0 + f64::from(SERIES_GAP_MS) / 1000.0 + 0.001;
        assert!(series.needs_aim(after));
    }

    /// A gap exactly at the threshold is still the same series, because the
    /// comparison is strict. The test builds its input with the same
    /// expression the code compares against, so it pins the operator rather
    /// than the threshold.
    #[test]
    fn a_gap_exactly_at_the_threshold_is_the_same_series() {
        let mut series = Series::new();
        series.note(100.0);
        assert!(!series.needs_aim(100.0 + f64::from(SERIES_GAP_MS) / 1000.0));
    }

    /// Time going backwards must not strand a series forever. evdev timestamps
    /// are `CLOCK_REALTIME`, which NTP and resume from suspend both move in
    /// either direction.
    #[test]
    fn an_event_dated_before_the_last_one_starts_a_new_series() {
        let mut series = Series::new();
        series.note(100.0);
        assert!(series.needs_aim(99.0));
    }

    #[test]
    fn a_repeated_timestamp_does_not_start_a_new_series() {
        let mut series = Series::new();
        series.note(100.0);
        assert!(!series.needs_aim(100.0));
    }

    #[test]
    fn up_scrolls_positive_and_down_scrolls_negative() {
        assert_eq!(units(Direction::Up, 40), 40);
        assert_eq!(units(Direction::Down, 40), -40);
    }

    #[test]
    fn the_configured_rate_is_the_units_of_one_event() {
        assert_eq!(units(Direction::Up, 120), 120);
        assert_eq!(units(Direction::Down, 120), -120);
    }

    #[test]
    fn a_rate_of_zero_scrolls_nothing() {
        assert_eq!(units(Direction::Up, 0), 0);
    }

    /// A configured rate is a small number in practice, but the parameter is
    /// `u32` and the result is `i32`. Saturating the conversion rather than
    /// letting it wrap keeps an unrealistic rate from turning into a negative
    /// click count instead of a very large positive one.
    #[test]
    fn a_rate_past_i32_range_saturates_rather_than_wraps() {
        assert_eq!(units(Direction::Up, u32::MAX), i32::MAX);
    }

    /// The shipped defaults, so the numbers in these tests are the numbers a
    /// user gets.
    fn scroll() -> ScrollSettings {
        ScrollSettings::default()
    }

    /// A tap delivers about ten events, so the whole of a tap sits in the
    /// first block and scrolls exactly as it did before acceleration existed.
    #[test]
    fn every_event_of_a_tap_scrolls_at_the_base_rate() {
        for event in 1..=10 {
            assert_eq!(rate(event, &scroll()), 40, "event {event}");
        }
    }

    /// The first event past the tap is the first faster one. This is the
    /// boundary the whole feature turns on.
    #[test]
    fn the_eleventh_event_is_the_first_faster_one() {
        assert_eq!(rate(11, &scroll()), 80);
        assert_eq!(rate(20, &scroll()), 80);
        assert_eq!(rate(21, &scroll()), 120);
    }

    /// Without the ceiling a long hold reaches 400 units an event, which is
    /// three notches per event and moves the page faster than it can be read.
    #[test]
    fn the_rate_stops_at_the_ceiling() {
        assert_eq!(rate(31, &scroll()), 120);
        assert_eq!(rate(1000, &scroll()), 120);
    }

    /// A step of zero is how a user turns acceleration off, and it has to keep
    /// working rather than being refused.
    #[test]
    fn a_step_of_zero_never_accelerates() {
        let flat = ScrollSettings {
            hi_res_step: 0,
            ..scroll()
        };
        assert_eq!(rate(1, &flat), 40);
        assert_eq!(rate(1000, &flat), 40);
    }

    /// Event 0 cannot arrive, because `Gesture::note` numbers a press 1. If it
    /// ever does, the base rate is the answer that scrolls rather than the one
    /// that panics on a subtraction below zero.
    #[test]
    fn event_zero_falls_back_to_the_base_rate() {
        assert_eq!(rate(0, &scroll()), 40);
    }

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).expect("the test table parses")
    }

    fn checked(text: &str) -> Result<ScrollSettings, String> {
        let settings = ScrollSettings::from_table(&table(text))?;
        settings.check()?;
        Ok(settings)
    }

    /// A rate of zero runs the whole scroll path, cursor placement included,
    /// and then emits nothing and says nothing, which leaves the most used
    /// button on the remote dead and the daemon reporting no trouble.
    #[test]
    fn a_zero_scroll_rate_is_rejected() {
        assert_eq!(
            checked("hi_res_per_event = 0").unwrap_err(),
            "hi_res_per_event is 0, so every scroll would emit nothing"
        );
    }

    #[test]
    fn zero_events_per_step_is_rejected() {
        assert_eq!(
            checked("events_per_step = 0").unwrap_err(),
            "events_per_step is 0, so no event could be counted into a step"
        );
    }

    /// A ceiling below the rate it caps would slow scrolling down instead of
    /// speeding it up, which is the opposite of what the key is for.
    #[test]
    fn a_ceiling_below_the_base_rate_is_rejected() {
        assert_eq!(
            checked("hi_res_max = 39").unwrap_err(),
            "hi_res_max is 39 and hi_res_per_event is 40, so the ceiling is below the rate it caps"
        );
    }

    /// Equal is allowed: it means the rate never rises, which is a valid way
    /// to turn acceleration off.
    #[test]
    fn a_ceiling_equal_to_the_base_rate_is_accepted() {
        assert!(checked("hi_res_max = 40").is_ok());
    }

    /// A section that names only the rate still loads, and gets the shipped
    /// curve and aim point rather than a parse error.
    #[test]
    fn a_section_without_the_acceleration_keys_gets_the_defaults() {
        let settings = checked("hi_res_per_event = 40").expect("parses");
        assert_eq!(settings, ScrollSettings::default());
    }

    /// The bounds are 0.0 inclusive and 1.0 exclusive, because that is the set
    /// of fractions that land inside the window: 0.0 is the window's first
    /// pixel and 1.0 is the first pixel past its far edge, which belongs to
    /// whatever sits there. A value outside the range is clamped to a screen
    /// edge, so the wheel goes to another window entirely.
    #[test]
    fn an_aim_fraction_outside_the_window_is_rejected() {
        for text in [
            "aim_x = 1.5",
            "aim_x = 1.0",
            "aim_x = -0.1",
            "aim_y = 1.0",
            "aim_y = -0.5",
        ] {
            let reason = checked(text).unwrap_err();
            assert!(
                reason.contains("an aim fraction runs from 0.0"),
                "{text} must be refused: {reason}"
            );
        }
    }

    /// The lower bound is inclusive: 0.0 aims at the window's own first pixel,
    /// which is inside it. Only the upper bound leaves the window.
    #[test]
    fn an_aim_fraction_of_zero_is_accepted() {
        let settings = checked("aim_x = 0.0").expect("0.0 aims at the window's first pixel");
        assert_eq!(settings.aim_x, 0.0);
    }
}
