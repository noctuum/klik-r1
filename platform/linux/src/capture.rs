use std::time::{SystemTime, UNIX_EPOCH};

use evdev::{EventSummary, EventType, InputEvent};

use klik_core::raw::Event;

/// The fixture format's spelling of an evdev event type.
///
/// This table is written out instead of derived, and that is the point of it.
/// evdev names its own variants `KEY`, `ABSOLUTE`, `SYNCHRONIZATION`, so
/// `format!("{:?}")` yields `ABSOLUTE` where `evtest` and every fixture say
/// `EV_ABS`. A recorder built on `Debug` writes a file this project's own
/// parser rejects, and nothing notices until the first replay.
///
/// `None` means the fixture format has no name for this type. The recorder
/// refuses such an event rather than inventing a spelling for it.
pub fn recorded_type(event_type: EventType) -> Option<klik_core::raw::EventType> {
    use klik_core::raw::EventType as Recorded;
    match event_type {
        EventType::SYNCHRONIZATION => Some(Recorded::Syn),
        EventType::KEY => Some(Recorded::Key),
        EventType::RELATIVE => Some(Recorded::Rel),
        EventType::ABSOLUTE => Some(Recorded::Abs),
        EventType::MISC => Some(Recorded::Msc),
        _ => None,
    }
}

/// Whether evdev's `Debug` output is a name rather than a message.
///
/// Two halves, and both decide something. The `all` half rejects
/// `unknown key: 767`, which is the message the crate prints for a code it does
/// not know. The leading-uppercase half is what makes the rule "a name", rather
/// than "a string of characters a name may contain": `_ABC` and `0ABC` pass the
/// first half and are still not names, and neither is the empty string, which
/// passes it vacuously.
///
/// Extracted from `code_name` rather than nested inside it so the rule can be
/// stated against strings. Reaching it through `code_name` needs a real
/// `InputEvent` whose `Debug` prints one of those shapes, and no such event
/// exists to build.
fn is_identifier(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// The kernel's name for an event code, or its number when the crate has none.
///
/// Code names need no table: `evdev_enum!` generates `Debug` as the constant's
/// own identifier, so `KeyCode::BTN_TOUCH` prints `BTN_TOUCH` and the names
/// agree with the fixtures exactly. The one edge is a code the crate does not
/// know, which prints `unknown key: 767`. That is not a name, and writing it
/// into a fixture would produce a line no reader can act on, so such a code is
/// written as its decimal number instead. A number is visible in a diff and
/// cannot be confused with a name, because every real name starts with a letter.
pub fn code_name(event: &InputEvent) -> String {
    fn name_or_number(code: impl std::fmt::Debug, raw: u16) -> String {
        let name = format!("{code:?}");
        if is_identifier(&name) {
            name
        } else {
            raw.to_string()
        }
    }

    let raw = event.code();
    match event.destructure() {
        EventSummary::Synchronization(_, code, _) => name_or_number(code, raw),
        EventSummary::Key(_, code, _) => name_or_number(code, raw),
        EventSummary::RelativeAxis(_, code, _) => name_or_number(code, raw),
        EventSummary::AbsoluteAxis(_, code, _) => name_or_number(code, raw),
        EventSummary::Misc(_, code, _) => name_or_number(code, raw),
        _ => raw.to_string(),
    }
}

/// Seconds since the epoch, the clock the captures already carry.
///
/// evdev timestamps are `CLOCK_REALTIME` unless `EVIOCSCLOCKID` says otherwise,
/// which is why `SystemTime::now()` is the right clock for the loop's own tick
/// to be compared against. A time before the epoch cannot come from the kernel,
/// and is returned negative rather than clamped: a silent zero would move every
/// debounce and grace window instead of failing where it happened.
pub fn seconds(time: SystemTime) -> f64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_secs_f64(),
        Err(before) => -before.duration().as_secs_f64(),
    }
}

/// One live event as it would appear in a recorded fixture.
///
/// `None` for a type the fixture format cannot spell; the caller counts those
/// and reports the count rather than dropping them in silence.
pub fn to_event(event: &InputEvent) -> Option<Event> {
    Some(Event {
        time: seconds(event.timestamp()),
        event_type: recorded_type(event.event_type())?,
        code: code_name(event),
        value: event.value(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Numeric codes taken from linux/input-event-codes.h, which is what the
    /// remote actually sends.
    const EV_SYN: u16 = 0x00;
    const EV_KEY: u16 = 0x01;
    const EV_ABS: u16 = 0x03;
    const EV_MSC: u16 = 0x04;

    #[test]
    fn the_six_codes_the_remote_sends_keep_their_kernel_names() {
        let cases = [
            (EV_KEY, 0x14a, "BTN_TOUCH"),
            (EV_KEY, 0x140, "BTN_TOOL_PEN"),
            (EV_ABS, 0x00, "ABS_X"),
            (EV_ABS, 0x01, "ABS_Y"),
            (EV_MSC, 0x04, "MSC_SCAN"),
            (EV_SYN, 0x00, "SYN_REPORT"),
        ];
        for (event_type, code, expected) in cases {
            let event = InputEvent::new(event_type, code, 0);
            assert_eq!(code_name(&event), expected, "type {event_type} code {code}");
        }
    }

    /// evdev spells its own type variants KEY, ABSOLUTE, SYNCHRONIZATION. The
    /// fixture format spells them EV_KEY, EV_ABS, EV_SYN. Deriving one from the
    /// other writes a file this project's parser rejects.
    #[test]
    fn event_types_map_to_the_kernel_spelling() {
        use klik_core::raw::EventType as Recorded;
        assert_eq!(recorded_type(EventType(EV_SYN)), Some(Recorded::Syn));
        assert_eq!(recorded_type(EventType(EV_KEY)), Some(Recorded::Key));
        assert_eq!(recorded_type(EventType(EV_ABS)), Some(Recorded::Abs));
        assert_eq!(recorded_type(EventType(EV_MSC)), Some(Recorded::Msc));
        assert_eq!(recorded_type(EventType(0x02)), Some(Recorded::Rel));
    }

    #[test]
    fn a_type_the_fixture_format_has_no_name_for_is_refused() {
        // EV_LED. Nothing on this path emits it, and inventing a spelling for
        // it would put an unparseable line in a fixture.
        assert_eq!(recorded_type(EventType(0x11)), None);
        assert!(to_event(&InputEvent::new(0x11, 0, 1)).is_none());
    }

    /// Debug on a code the crate does not know prints "unknown key: 767", which
    /// is not a name. Every real name is an uppercase identifier, so the shape
    /// decides, not the wording of a message that may change between versions.
    #[test]
    fn an_unnamed_code_is_written_as_its_number() {
        assert_eq!(code_name(&InputEvent::new(EV_KEY, 767, 1)), "767");
    }

    /// The half of the rule no event can exercise. Deleting the leading-letter
    /// guard left the whole suite green, because `unknown key: 767` is already
    /// refused by the character set alone. The guard was a stated rule that
    /// nothing checked. These are the strings that tell the two halves apart.
    #[test]
    fn a_name_has_to_start_with_an_uppercase_letter() {
        assert!(is_identifier("BTN_TOUCH"));
        assert!(is_identifier("ABS_X"));
        assert!(is_identifier("SYN_REPORT"));
        assert!(!is_identifier("_ABC"), "a leading underscore is not a name");
        assert!(!is_identifier("0ABC"), "a leading digit is not a name");
        assert!(!is_identifier(""), "nothing at all is not a name");
    }

    /// The other half, stated directly rather than only through an event: the
    /// crate's message for an unknown code is made of characters no name has.
    #[test]
    fn a_message_is_not_a_name() {
        assert!(!is_identifier("unknown key: 767"));
        assert!(!is_identifier("BTN touch"), "a space is not part of a name");
        assert!(!is_identifier("btn_touch"), "nor is lower case");
    }

    /// `code_name`'s last arm, for an event whose type it destructures to none
    /// of the five it names. `to_event` can never reach it, because
    /// `recorded_type` refuses such a type first, but `code_name` is `pub` and
    /// answers on its own. Gutting that arm to an empty string left all 127
    /// tests green, which in a module with no I/O in it is a defect rather than
    /// an accepted gap.
    #[test]
    fn a_type_with_no_named_code_still_answers_with_its_number() {
        // EV_LED, which destructures to a summary variant `code_name` does not
        // name. The code survives as its decimal self, like any unknown code.
        assert_eq!(code_name(&InputEvent::new(0x11, 5, 1)), "5");
    }

    #[test]
    fn a_timestamp_becomes_seconds_since_the_epoch() {
        // Compared with a tolerance rather than exactly. The value has sixteen
        // significant digits and an f64 carries about fifteen, so an exact
        // literal would pin an artifact of rounding instead of the conversion.
        // A microsecond is orders finer than anything the recognizer measures.
        let t = UNIX_EPOCH + Duration::from_micros(1_787_594_305_903_279);
        assert!((seconds(t) - 1_787_594_305.903_279).abs() < 1e-6);
    }

    /// A clock set before the epoch cannot come from the kernel, but clamping it
    /// to zero would move every window in the recognizer instead of failing.
    #[test]
    fn a_time_before_the_epoch_is_negative_rather_than_zero() {
        let t = UNIX_EPOCH - Duration::from_millis(1500);
        assert_eq!(seconds(t), -1.5);
    }

    #[test]
    fn a_whole_event_converts() {
        let event = InputEvent::new(EV_ABS, 0x01, 2036);
        let converted = to_event(&event).expect("EV_ABS converts");
        assert_eq!(converted.event_type, klik_core::raw::EventType::Abs);
        assert_eq!(converted.code, "ABS_Y");
        assert_eq!(converted.value, 2036);
    }

    /// The recorder's output has to survive this project's own parser, which is
    /// the check that would have caught an EV_ABS written as "ABSOLUTE".
    #[test]
    fn what_the_recorder_writes_is_what_the_parser_reads() {
        let event = to_event(&InputEvent::new(EV_KEY, 0x14a, 1)).expect("converts");
        let line = serde_json::to_string(&event).expect("serializes");
        let parsed = Event::parse_line(&line).expect("round trips");
        assert_eq!(parsed, event);
    }
}
