use klik_beauty_r1::Settings;
use klik_beauty_r1::button::Button;
use klik_beauty_r1::recognizer::{Outcome, Recognizer, RecognizerSettings};
use klik_beauty_r1::signature::Signature;
use klik_core::press::PressKind;
use klik_core::raw::{Event, EventType};

const FIXTURE: &str = include_str!("fixtures/beauty-r1-clean-run.jsonl");

/// Captured at the desk on 2026-09-17: Center, Camera and Up, each pressed
/// twice in a row with nothing in between. No earlier capture contains a button
/// pressed twice, which is why the defect this fixture exposes survived.
const REPEATS: &str = include_str!("fixtures/beauty-r1-repeats.jsonl");

fn recognizer() -> Recognizer {
    let settings = RecognizerSettings {
        debounce_ms: 150,
        connect_grace_ms: 0,
        camera_hold_ms: 200,
        volume_repeat_ms: 1500,
    };
    let signatures = vec![
        Signature {
            button: Button::Center,
            x: 1904,
            y: 1904,
            tolerance: 16,
        },
        Signature {
            button: Button::Camera,
            x: 2048,
            y: 3424,
            tolerance: 16,
        },
    ];
    Recognizer::new(settings, signatures, 0.0)
}

/// `calibrate`'s recognizer: the same settings with no signatures at all.
/// Nothing can match, so every gesture is a candidate signature and has to be
/// able to report itself as unmatched, which is why both axes keep the
/// two-delta rule in this state.
fn without_signatures() -> Recognizer {
    let settings = RecognizerSettings {
        debounce_ms: 150,
        connect_grace_ms: 0,
        camera_hold_ms: 200,
        volume_repeat_ms: 1500,
    };
    Recognizer::new(settings, Vec::new(), 0.0)
}

/// The recognizer a configuration gets when its section names the address
/// and nothing else: the quiet window of 45 ms and the two signatures
/// measured on the one unit.
fn shipped() -> Recognizer {
    let table: toml::Table =
        toml::from_str(r#"uniq = "aa:bb:cc:dd:ee:01""#).expect("the table parses");
    let settings = Settings::from_table(&table).expect("the address is enough");
    Recognizer::new(settings.recognizer, settings.signatures, 0.0)
}

fn presses(outcomes: &[Outcome]) -> Vec<(Button, PressKind)> {
    outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Press(b, k) => Some((*b, *k)),
            _ => None,
        })
        .collect()
}

fn run(source: &str) -> Vec<Outcome> {
    let events = Event::parse_stream(source).expect("fixture parses");
    let mut r = recognizer();
    events.iter().filter_map(|e| r.feed(e)).collect()
}

fn run_fixture() -> Vec<Outcome> {
    run(FIXTURE)
}

#[test]
fn the_clean_run_resolves_all_six_buttons_in_order() {
    assert_eq!(
        presses(&run_fixture()),
        vec![
            (Button::Up, PressKind::Tap),
            (Button::Down, PressKind::Tap),
            (Button::Left, PressKind::Tap),
            (Button::Right, PressKind::Tap),
            (Button::Center, PressKind::Tap),
            (Button::Camera, PressKind::Tap),
        ]
    );
}

#[test]
fn nothing_is_left_unmatched() {
    let unmatched: Vec<_> = run_fixture()
        .into_iter()
        .filter(|o| matches!(o, Outcome::Unmatched { .. }))
        .collect();
    assert!(unmatched.is_empty(), "unmatched gestures: {unmatched:?}");
}

/// Five, not six. The camera reports its tap on release instead of an End,
/// because a camera gesture is not known to be a tap until it ends.
#[test]
fn every_gesture_except_the_camera_reports_its_end() {
    let outcomes = run_fixture();
    let ends: Vec<_> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::End(b) => Some(*b),
            _ => None,
        })
        .collect();
    assert_eq!(
        ends,
        vec![
            Button::Up,
            Button::Down,
            Button::Left,
            Button::Right,
            Button::Center
        ]
    );
}

/// The guarantee the whole camera design rests on: holding it must never first
/// perform the tap action.
#[test]
fn a_camera_gesture_reports_nothing_before_it_ends() {
    let events = Event::parse_stream(FIXTURE).expect("fixture parses");
    let open = events
        .iter()
        .position(|e| e.code == "ABS_Y" && e.value == 3424)
        .expect("the fixture contains the camera gesture");
    let mut r = recognizer();
    for e in &events[..=open] {
        r.feed(e);
    }
    let report = &events[open + 1];
    assert_eq!(report.code, "SYN_REPORT", "the camera packet closes here");
    assert_eq!(r.feed(report), None, "the opening report says nothing");
}

#[test]
fn arrows_emit_ticks_after_they_resolve() {
    let ticks = run_fixture()
        .iter()
        .filter(|o| matches!(o, Outcome::Tick(Button::Up)))
        .count();
    assert!(
        ticks > 0,
        "a held or tapped Up must keep producing scroll ticks"
    );
}

#[test]
fn the_grace_period_after_connect_reports_nothing() {
    let events = Event::parse_stream(FIXTURE).expect("fixture parses");
    let settings = RecognizerSettings {
        debounce_ms: 150,
        connect_grace_ms: 60_000,
        camera_hold_ms: 200,
        volume_repeat_ms: 1500,
    };
    let mut r = Recognizer::new(settings, vec![], events[0].time);
    let outcomes: Vec<_> = events.iter().filter_map(|e| r.feed(e)).collect();
    assert!(
        outcomes.is_empty(),
        "the grace period suppresses the whole run"
    );
}

/// Synthesized from the measured intervals in beauty-r1-hold-map.log: the
/// remote sends a complete press and release pair about every 1.1 s, never a
/// kernel autorepeat with value 2.
fn volume_event(time: f64, code: &str, value: i32) -> Event {
    Event {
        time,
        event_type: EventType::Key,
        code: code.to_string(),
        value,
    }
}

#[test]
fn a_volume_key_is_a_hold_of_that_arrow() {
    let mut r = recognizer();
    assert_eq!(
        r.feed(&volume_event(10.0, "KEY_VOLUMEUP", 1)),
        Some(Outcome::Press(Button::Right, PressKind::Hold))
    );
}

/// The whole point of the separate volume window. The intervals are the
/// measured ones: `beauty-r1-hold-map.log` has five pairs about 1.1 s apart
/// across a single hold. A bounce-sized debounce would let every one of them
/// through, and a held Right would submit the input field once a second.
#[test]
fn a_five_second_hold_fires_exactly_once() {
    let mut r = recognizer();
    let fired: Vec<_> = [0.0, 1.10, 2.21, 3.31, 4.42]
        .iter()
        .filter_map(|t| r.feed(&volume_event(10.0 + t, "KEY_VOLUMEDOWN", 1)))
        .collect();
    assert_eq!(fired, vec![Outcome::Press(Button::Left, PressKind::Hold)]);
}

/// A deliberate second press after the hold ended must not be swallowed.
#[test]
fn a_fresh_press_after_the_window_fires_again() {
    let mut r = recognizer();
    r.feed(&volume_event(10.0, "KEY_VOLUMEDOWN", 1));
    let later = r.feed(&volume_event(12.0, "KEY_VOLUMEDOWN", 1));
    assert_eq!(later, Some(Outcome::Press(Button::Left, PressKind::Hold)));
}

/// The volume window must not reach into the touch path. Holding Right and then
/// immediately tapping it is an ordinary thing to do.
#[test]
fn a_volume_hold_does_not_suppress_the_next_tap() {
    let events = Event::parse_stream(FIXTURE).expect("fixture parses");
    let first_open = events
        .iter()
        .position(|e| e.code == "BTN_TOUCH" && e.value == 1)
        .expect("the fixture opens a gesture");
    let mut r = recognizer();
    r.feed(&volume_event(
        events[first_open].time - 0.3,
        "KEY_VOLUMEUP",
        1,
    ));
    let outcomes: Vec<_> = events.iter().filter_map(|e| r.feed(e)).collect();
    assert!(
        outcomes
            .iter()
            .any(|o| matches!(o, Outcome::Press(Button::Up, PressKind::Tap))),
        "the Up tap 300 ms after a volume event must still resolve"
    );
}

/// A held Left sends a press and release pair about every 1.1 s, and the
/// remote is gone about 450 ms when it drops, so one repeat of a hold that
/// spans a reconnect lands inside the re-armed settling window. That repeat
/// reports nothing, by design, but it is still a volume event and the window
/// that suppresses the repeats slides on every one of them. Left unarmed, the
/// next repeat of the same hold fires a second time.
#[test]
fn a_volume_repeat_inside_the_settling_window_slides_the_window() {
    let settings = RecognizerSettings {
        debounce_ms: 150,
        connect_grace_ms: 500,
        camera_hold_ms: 200,
        volume_repeat_ms: 1500,
    };
    let mut r = Recognizer::new(settings, Vec::new(), 0.0);
    assert_eq!(
        r.feed(&volume_event(10.0, "KEY_VOLUMEDOWN", 1)),
        Some(Outcome::Press(Button::Left, PressKind::Hold))
    );
    assert_eq!(r.feed(&volume_event(11.1, "KEY_VOLUMEDOWN", 1)), None);

    // What `daemon::step` does for a disconnect and the reconnect after it.
    r.reset();
    r.seed_position(None, None);
    r.rearm_grace(11.95);

    assert_eq!(r.feed(&volume_event(12.2, "KEY_VOLUMEDOWN", 1)), None);
    assert_eq!(
        r.feed(&volume_event(13.3, "KEY_VOLUMEDOWN", 1)),
        None,
        "the repeat inside the window re-armed the suppression window"
    );
}

/// Outside the gate only the press half of a repeat arms the window, because
/// `feed` routes `value == 1` to `on_volume` and drops the release. The gate
/// has to agree with it: a release that slid the window would suppress the
/// first real press after the window closes.
#[test]
fn a_volume_release_inside_the_settling_window_does_not_slide_it() {
    let settings = RecognizerSettings {
        debounce_ms: 150,
        connect_grace_ms: 500,
        camera_hold_ms: 200,
        volume_repeat_ms: 1500,
    };
    let mut r = Recognizer::new(settings, Vec::new(), 0.0);
    r.rearm_grace(10.0);

    assert_eq!(r.feed(&volume_event(10.1, "KEY_VOLUMEDOWN", 0)), None);
    assert_eq!(
        r.feed(&volume_event(10.6, "KEY_VOLUMEDOWN", 1)),
        Some(Outcome::Press(Button::Left, PressKind::Hold)),
        "the release must not have armed the suppression window"
    );
}

/// A complete gesture at the Center's motionless signature: press, both axes,
/// the report that resolves it, and the release. Used to prove debounce with
/// an outcome the recognizer can actually withhold, unlike a lone `BTN_TOUCH`
/// which reports nothing whether or not it was suppressed. Synthesized: the
/// bounce capture runs were not kept, so `open_at` and `release_at` below are
/// chosen relative to `debounce_ms` rather than measured.
fn center_gesture(open_at: f64, release_at: f64) -> Vec<Event> {
    let touch = |time, value| Event {
        time,
        event_type: EventType::Key,
        code: "BTN_TOUCH".to_string(),
        value,
    };
    let abs = |code: &str, value| Event {
        time: open_at,
        event_type: EventType::Abs,
        code: code.to_string(),
        value,
    };
    vec![
        touch(open_at, 1),
        abs("ABS_X", 1904),
        abs("ABS_Y", 1904),
        Event {
            time: open_at,
            event_type: EventType::Syn,
            code: "SYN_REPORT".to_string(),
            value: 0,
        },
        touch(release_at, 0),
    ]
}

/// Bounce, the thing `debounce_ms` exists for. Two capture runs contained
/// clusters 30 ms apart; synthesized here because those runs were not kept.
/// A complete second gesture (press, axes, report, release) replayed 30 ms
/// after the previous gesture's release must yield no `Press` at all; the
/// identical gesture 200 ms later, past the 150 ms debounce window, must
/// yield one. The two halves differ only in that interval.
#[test]
fn a_bounced_gesture_yields_no_press_but_a_later_one_does() {
    let events = Event::parse_stream(FIXTURE).expect("fixture parses");
    let last_release = events.last().expect("fixture is not empty").time;

    let mut bounced = recognizer();
    for e in &events {
        bounced.feed(e);
    }
    let bounce_outcomes: Vec<_> = center_gesture(last_release + 0.030, last_release + 0.060)
        .iter()
        .filter_map(|e| bounced.feed(e))
        .collect();
    assert!(
        !bounce_outcomes
            .iter()
            .any(|o| matches!(o, Outcome::Press(..))),
        "a gesture 30 ms after the previous release is bounce: {bounce_outcomes:?}"
    );

    let mut later = recognizer();
    for e in &events {
        later.feed(e);
    }
    let later_outcomes: Vec<_> = center_gesture(last_release + 0.200, last_release + 0.230)
        .iter()
        .filter_map(|e| later.feed(e))
        .collect();
    assert!(
        later_outcomes
            .iter()
            .any(|o| matches!(o, Outcome::Press(Button::Center, PressKind::Tap))),
        "a gesture 200 ms after the previous release must resolve: {later_outcomes:?}"
    );
}

#[test]
fn the_camera_hold_fires_from_the_timer() {
    let mut r = recognizer();
    let events = Event::parse_stream(FIXTURE).expect("fixture parses");
    let camera_open = events
        .iter()
        .position(|e| e.code == "ABS_Y" && e.value == 3424)
        .expect("the fixture contains the camera gesture");
    for e in &events[..=camera_open + 1] {
        r.feed(e);
    }
    let opened_at = events[camera_open].time;
    assert_eq!(r.tick(opened_at + 0.1), None, "100 ms is still a tap");
    assert_eq!(
        r.tick(opened_at + 0.25),
        Some(Outcome::Press(Button::Camera, PressKind::Hold))
    );
    assert_eq!(r.tick(opened_at + 0.30), None, "the hold fires once");
}

/// Synthesized: no capture contains a gesture that opens away from both
/// signatures and then never agrees on a direction. The axis reverses sign on
/// every step so `DirectionTracker` never confirms one, exercising the one
/// path a real capture has never produced but the calibration story depends on.
///
/// Built on `calibrate`'s empty table, which is the state this path belongs to:
/// with signatures loaded the first delta would resolve the gesture as an arrow
/// instead, which is the cost `Vertical` records.
#[test]
fn an_unresolved_gesture_reports_what_it_saw_on_release() {
    let mut r = without_signatures();
    let open = Event {
        time: 20.0,
        event_type: EventType::Key,
        code: "BTN_TOUCH".to_string(),
        value: 1,
    };
    assert_eq!(r.feed(&open), None);

    // Opens at Y=2500, far from both the center and the camera signatures.
    assert_eq!(
        r.feed(&Event {
            time: 20.0,
            event_type: EventType::Abs,
            code: "ABS_Y".to_string(),
            value: 2500,
        }),
        None
    );
    assert_eq!(
        r.feed(&Event {
            time: 20.0,
            event_type: EventType::Syn,
            code: "SYN_REPORT".to_string(),
            value: 0,
        }),
        None,
        "2500 matches no signature"
    );

    // The sign flips on every delta, so two consecutive deltas never agree.
    for (time, value) in [(20.1, 2600), (20.2, 2550), (20.3, 2600)] {
        assert_eq!(
            r.feed(&Event {
                time,
                event_type: EventType::Abs,
                code: "ABS_Y".to_string(),
                value,
            }),
            None
        );
    }

    let release = Event {
        time: 20.5,
        event_type: EventType::Key,
        code: "BTN_TOUCH".to_string(),
        value: 0,
    };
    assert_eq!(
        r.feed(&release),
        Some(Outcome::Unmatched {
            x: None,
            y: Some(2600)
        })
    );
}

/// The kernel reports an absolute axis only when its value differs from the one
/// it last reported, and that state outlives the gesture that set it. A
/// motionless button replays to its own coordinates and stops there, so pressing
/// it twice in a row leaves the second gesture carrying no axis event at all.
/// Identifying it means remembering where the pointer already is.
#[test]
fn a_motionless_button_pressed_twice_is_recognized_both_times() {
    let cameras = presses(&run(REPEATS))
        .into_iter()
        .filter(|(button, _)| *button == Button::Camera)
        .count();
    assert_eq!(
        cameras, 2,
        "both camera presses must resolve, and the second one carries no coordinates"
    );
}

/// The unmatched line is what a miscalibrated remote is diagnosed from, and the
/// design promises the numbers it prints are the ones that belong in a
/// `[[signature]]` section. A signature needs both axes. A gesture only mentions
/// the axes that changed, so reporting the packet alone yields half a signature
/// and no way to tell a missing axis from an unknown one.
///
/// On `calibrate`'s empty table, for the same reason as the test above: this is
/// the state where an unmatched report is what the caller is there to collect.
#[test]
fn an_unmatched_gesture_reports_the_position_not_just_the_axes_it_mentioned() {
    let abs = |time: f64, code: &str, value: i32| Event {
        time,
        event_type: EventType::Abs,
        code: code.to_string(),
        value,
    };
    let touch = |time: f64, value: i32| Event {
        time,
        event_type: EventType::Key,
        code: "BTN_TOUCH".to_string(),
        value,
    };
    let syn = |time: f64| Event {
        time,
        event_type: EventType::Syn,
        code: "SYN_REPORT".to_string(),
        value: 0,
    };

    let mut r = without_signatures();

    // A gesture at what would be the camera's spot, which leaves the pointer at
    // (2048, 3424). With no signatures loaded nothing matches it, and its own
    // report is not what this test is about.
    for event in [
        touch(10.0, 1),
        abs(10.0, "ABS_X", 2048),
        abs(10.0, "ABS_Y", 3424),
        syn(10.0),
        touch(10.1, 0),
    ] {
        r.feed(&event);
    }

    // A gesture that moves on Y alone and matches nothing. X is not mentioned
    // because it has not changed, not because it is unknown.
    for event in [
        touch(20.0, 1),
        abs(20.0, "ABS_Y", 2500),
        syn(20.0),
        abs(20.1, "ABS_Y", 2600),
        abs(20.2, "ABS_Y", 2550),
        abs(20.3, "ABS_Y", 2600),
    ] {
        r.feed(&event);
    }

    assert_eq!(
        r.feed(&touch(20.5, 0)),
        Some(Outcome::Unmatched {
            x: Some(2048),
            y: Some(2600)
        }),
        "X is still 2048 from the camera press and belongs in the diagnostic"
    );
}

/// A recognizer that has just started has never seen a coordinate, so a
/// motionless press arriving before anything moves is unidentifiable. The two
/// leading `unmatched` lines in the repeats fixture are exactly that. The
/// kernel knows the position the whole time, so the daemon reads it with
/// EVIOCGABS when it opens the device and hands it over here.
#[test]
fn a_seeded_recognizer_resolves_a_press_that_carries_no_coordinates() {
    let touch = |time: f64, value: i32| Event {
        time,
        event_type: EventType::Key,
        code: "BTN_TOUCH".to_string(),
        value,
    };
    let syn = |time: f64| Event {
        time,
        event_type: EventType::Syn,
        code: "SYN_REPORT".to_string(),
        value: 0,
    };

    let mut r = recognizer();
    r.seed_position(Some(1904), Some(1904));

    assert_eq!(r.feed(&touch(1.0, 1)), None);
    assert_eq!(
        r.feed(&syn(1.0)),
        Some(Outcome::Press(Button::Center, PressKind::Tap)),
        "the pointer was already on the center and the gesture says nothing new"
    );
}

/// The same stream without the seed. Kept beside the test above so the
/// difference is the seed and nothing else.
#[test]
fn an_unseeded_recognizer_cannot_resolve_that_press_and_says_so() {
    let leading_unmatched = run(REPEATS)
        .into_iter()
        .take_while(|o| matches!(o, Outcome::Unmatched { .. }))
        .count();
    assert_eq!(
        leading_unmatched, 2,
        "both opening center presses are unidentifiable without a seed"
    );
}

/// A camera gesture that is cut short by a disconnect leaves camera_opened_at
/// still set. After the reconnect, tick fires a Hold for a gesture that no
/// longer exists. reset must drop it. Kept beside the unreset test so the
/// difference is the reset and nothing else.
#[test]
fn a_reset_drops_a_camera_hold_that_the_disconnect_cut_short() {
    let touch = |time: f64, value: i32| Event {
        time,
        event_type: EventType::Key,
        code: "BTN_TOUCH".to_string(),
        value,
    };
    let abs = |time: f64, code: &str, value: i32| Event {
        time,
        event_type: EventType::Abs,
        code: code.to_string(),
        value,
    };
    let syn = |time: f64| Event {
        time,
        event_type: EventType::Syn,
        code: "SYN_REPORT".to_string(),
        value: 0,
    };

    // Open a camera gesture at time 10.0: BTN_TOUCH, ABS_X, ABS_Y, SYN_REPORT.
    // Do not close it with BTN_TOUCH = 0, simulating a disconnect.
    let mut r_unreset = recognizer();
    r_unreset.feed(&touch(10.0, 1));
    r_unreset.feed(&abs(10.0, "ABS_X", 2048));
    r_unreset.feed(&abs(10.0, "ABS_Y", 3424));
    r_unreset.feed(&syn(10.0));

    // Without a reset, tick fires the hold that the disconnect cut short.
    assert_eq!(
        r_unreset.tick(10.5),
        Some(Outcome::Press(Button::Camera, PressKind::Hold)),
        "the unreset recognizer fires a ghost hold after the disconnect"
    );

    // Now reset and run the same opening again.
    let mut r_reset = recognizer();
    r_reset.feed(&touch(10.0, 1));
    r_reset.feed(&abs(10.0, "ABS_X", 2048));
    r_reset.feed(&abs(10.0, "ABS_Y", 3424));
    r_reset.feed(&syn(10.0));

    // Reset the gesture state.
    r_reset.reset();

    // Now tick says nothing: the gesture was dropped.
    assert_eq!(
        r_reset.tick(10.5),
        None,
        "the reset recognizer drops the incomplete camera gesture"
    );
}

/// reset must drop the gesture state but leave the position alone. A caller
/// handling a reconnect calls reset and then seed_position; if reset cleared
/// the position the seed would be useless.
#[test]
fn a_reset_leaves_the_position_alone() {
    let touch = |time: f64, value: i32| Event {
        time,
        event_type: EventType::Key,
        code: "BTN_TOUCH".to_string(),
        value,
    };
    let syn = |time: f64| Event {
        time,
        event_type: EventType::Syn,
        code: "SYN_REPORT".to_string(),
        value: 0,
    };

    let mut r = recognizer();

    // Seed the position to the center.
    r.seed_position(Some(1904), Some(1904));

    // Reset the gesture state.
    r.reset();

    // Feed a motionless center press with no coordinate events. If reset had
    // cleared the position, this would be unmatched. Instead, it resolves
    // because the position is still there.
    assert_eq!(r.feed(&touch(1.0, 1)), None);
    assert_eq!(
        r.feed(&syn(1.0)),
        Some(Outcome::Press(Button::Center, PressKind::Tap)),
        "reset left the position alone, so the motionless press still resolves"
    );
}

/// `reset` cannot re-arm the settling window that follows a connect: it has no
/// clock. `rearm_grace` is what a caller uses instead, on every connect and not
/// merely the first. A phantom gesture cluster appeared at the start of the
/// first run after connecting, and nothing says a reconnect is different. A
/// gesture landing inside the re-armed window must be dropped exactly as one
/// arriving right after `Recognizer::new` would be; the same gesture once the
/// window has passed must resolve.
#[test]
fn rearm_grace_drops_inside_the_window_and_resolves_past_it() {
    let touch = |time: f64, value: i32| Event {
        time,
        event_type: EventType::Key,
        code: "BTN_TOUCH".to_string(),
        value,
    };
    let syn = |time: f64| Event {
        time,
        event_type: EventType::Syn,
        code: "SYN_REPORT".to_string(),
        value: 0,
    };

    let settings = RecognizerSettings {
        debounce_ms: 150,
        connect_grace_ms: 500,
        camera_hold_ms: 200,
        volume_repeat_ms: 1500,
    };
    let signatures = vec![Signature {
        button: Button::Center,
        x: 1904,
        y: 1904,
        tolerance: 16,
    }];
    let mut r = Recognizer::new(settings, signatures, 0.0);
    r.seed_position(Some(1904), Some(1904));

    // Re-armed at t=100.0 with a 500 ms window: it now runs to 100.5.
    r.rearm_grace(100.0);

    // Inside the re-armed window: dropped, the same as right after `new`.
    assert_eq!(r.feed(&touch(100.1, 1)), None);
    assert_eq!(
        r.feed(&syn(100.1)),
        None,
        "a gesture inside the re-armed window is dropped"
    );

    // Past the window: the identical gesture resolves normally.
    assert_eq!(r.feed(&touch(100.6, 1)), None);
    assert_eq!(
        r.feed(&syn(100.6)),
        Some(Outcome::Press(Button::Center, PressKind::Tap)),
        "past the window the same gesture resolves"
    );
}

/// Center and Camera at the measured signatures, a 500 ms settling window
/// re-armed at 100.0 so it runs to 100.5, and the position seeded at `x`, `y`.
fn settling(x: i32, y: i32) -> Recognizer {
    let settings = RecognizerSettings {
        debounce_ms: 150,
        connect_grace_ms: 500,
        camera_hold_ms: 200,
        volume_repeat_ms: 1500,
    };
    let signatures = vec![
        Signature {
            button: Button::Center,
            x: 1904,
            y: 1904,
            tolerance: 16,
        },
        Signature {
            button: Button::Camera,
            x: 2048,
            y: 3424,
            tolerance: 16,
        },
    ];
    let mut r = Recognizer::new(settings, signatures, 0.0);
    r.seed_position(Some(x), Some(y));
    r.rearm_grace(100.0);
    r
}

fn at(time: f64, event_type: EventType, code: &str, value: i32) -> Event {
    Event {
        time,
        event_type,
        code: code.to_string(),
        value,
    }
}

/// A whole gesture opened at `x`, `y` and released, inside the window.
fn gesture_inside_the_window(r: &mut Recognizer, x: i32, y: i32) {
    for event in [
        at(100.1, EventType::Key, "BTN_TOUCH", 1),
        at(100.1, EventType::Abs, "ABS_X", x),
        at(100.1, EventType::Abs, "ABS_Y", y),
        at(100.1, EventType::Syn, "SYN_REPORT", 0),
        at(100.2, EventType::Key, "BTN_TOUCH", 0),
        at(100.2, EventType::Syn, "SYN_REPORT", 0),
    ] {
        assert_eq!(r.feed(&event), None, "inside the window: {event:?}");
    }
}

/// The kernel reports an axis only when it changes, so a press on the same
/// spot after a gesture inside the window carries no axis event at all. The
/// window reports nothing, but that gesture's axes still have to reach the
/// remembered position, or the next press is matched against the old one.
#[test]
fn the_settling_window_reports_nothing_but_keeps_the_position() {
    let mut r = settling(0, 0);
    gesture_inside_the_window(&mut r, 1904, 1904);

    assert_eq!(r.feed(&at(101.0, EventType::Key, "BTN_TOUCH", 1)), None);
    assert_eq!(
        r.feed(&at(101.0, EventType::Syn, "SYN_REPORT", 0)),
        Some(Outcome::Press(Button::Center, PressKind::Tap))
    );
}

/// The wrong-button path: seeded on Center, a Camera press inside the window,
/// then a Camera press with no axis event after it. With the window's axes
/// dropped, that press resolves as Center, which the example configuration
/// binds to `KEY_1`.
#[test]
fn a_camera_press_inside_the_settling_window_is_not_taken_for_center() {
    let mut r = settling(1904, 1904);
    gesture_inside_the_window(&mut r, 2048, 3424);

    assert_eq!(r.feed(&at(101.0, EventType::Key, "BTN_TOUCH", 1)), None);
    assert_eq!(
        r.feed(&at(101.0, EventType::Syn, "SYN_REPORT", 0)),
        None,
        "a camera press reports on release, not on open"
    );
    assert_eq!(
        r.feed(&at(101.1, EventType::Key, "BTN_TOUCH", 0)),
        Some(Outcome::Press(Button::Camera, PressKind::Tap))
    );
}

/// Both callers in the binary, `daemon::step` and `calibrate`, reset before
/// they re-arm, so the window normally opens on `Idle`. The gate does not lean
/// on that: with an Up gesture open and one delta short of resolving when the
/// window opens, the delta that would resolve it arrives inside the window and
/// reports nothing.
#[test]
fn a_window_opened_mid_gesture_still_reports_nothing() {
    let mut r = settling(0, 0);
    for event in [
        at(101.0, EventType::Key, "BTN_TOUCH", 1),
        at(101.0, EventType::Abs, "ABS_Y", 2036),
        at(101.0, EventType::Syn, "SYN_REPORT", 0),
    ] {
        assert_eq!(r.feed(&event), None, "{event:?}");
    }
    r.rearm_grace(101.05);
    assert_eq!(r.feed(&at(101.06, EventType::Abs, "ABS_Y", 2124)), None);
}

/// A gesture that opens inside the settling window releases after it, with
/// the machine still in `Idle`. Nothing was in flight, so that release is not
/// the end of a gesture and must not arm the debounce: the next real press is
/// not bounce.
#[test]
fn a_release_after_the_settling_window_is_not_a_gesture_ending() {
    let mut r = settling(1904, 1904);
    assert_eq!(r.feed(&at(100.45, EventType::Key, "BTN_TOUCH", 1)), None);
    assert_eq!(r.feed(&at(100.45, EventType::Syn, "SYN_REPORT", 0)), None);
    assert_eq!(r.feed(&at(100.58, EventType::Key, "BTN_TOUCH", 0)), None);

    assert_eq!(r.feed(&at(100.65, EventType::Key, "BTN_TOUCH", 1)), None);
    assert_eq!(
        r.feed(&at(100.65, EventType::Syn, "SYN_REPORT", 0)),
        Some(Outcome::Press(Button::Center, PressKind::Tap)),
        "the press after the window was dropped as bounce"
    );
}

/// The other half of the same rule: a press dropped as bounce must not push
/// the window out with its own release, or tapping faster than `debounce_ms`
/// keeps the window open and swallows every tap. The first gesture's release
/// at 10.05 arms the window to 10.2; the bounce opens inside it and releases
/// inside it; the real gesture opens once it has passed.
#[test]
fn a_bounced_press_does_not_extend_the_debounce_window() {
    let mut r = recognizer();
    for event in center_gesture(10.0, 10.05) {
        r.feed(&event);
    }
    let bounced: Vec<_> = center_gesture(10.1, 10.11)
        .iter()
        .filter_map(|e| r.feed(e))
        .collect();
    assert_eq!(
        bounced,
        vec![Outcome::Refused {
            button: Button::Center,
            after_ms: 50
        }],
        "the bounce is reported as refused, and as nothing else"
    );

    let outcomes: Vec<_> = center_gesture(10.21, 10.26)
        .iter()
        .filter_map(|e| r.feed(e))
        .collect();
    assert_eq!(
        outcomes.first(),
        Some(&Outcome::Press(Button::Center, PressKind::Tap)),
        "the bounce must not have pushed the window past 10.2"
    );
}

/// A refused press is reported with the time since the release before it,
/// whichever press that release ended. The first gesture is released at 10.05
/// and arms the window to 10.2. The second opens 30 ms later, is refused, and
/// is released at 10.09. The third opens at 10.13, which is 40 ms after the
/// release of the second and 80 ms after the release of the first.
#[test]
fn a_refused_press_names_the_time_since_the_last_release() {
    let mut r = recognizer();
    for event in center_gesture(10.0, 10.05) {
        r.feed(&event);
    }
    let refused = |after_ms| Outcome::Refused {
        button: Button::Center,
        after_ms,
    };

    let second: Vec<_> = center_gesture(10.08, 10.09)
        .iter()
        .filter_map(|e| r.feed(e))
        .collect();
    assert_eq!(second, vec![refused(30)]);

    let third: Vec<_> = center_gesture(10.13, 10.14)
        .iter()
        .filter_map(|e| r.feed(e))
        .collect();
    assert_eq!(third, vec![refused(40)]);
}

/// `docs/logs/beauty-r1-capabilities.log` shows the remote declaring
/// `ABS_VOLUME` and `ABS_MISC` beside the two axes. Neither carries a
/// position, so neither may reach the remembered one.
#[test]
fn another_absolute_axis_does_not_move_the_remembered_position() {
    let mut r = settling(1904, 1904);
    assert_eq!(r.feed(&at(101.0, EventType::Abs, "ABS_VOLUME", 448)), None);
    assert_eq!(r.feed(&at(101.1, EventType::Key, "BTN_TOUCH", 1)), None);
    assert_eq!(
        r.feed(&at(101.1, EventType::Syn, "SYN_REPORT", 0)),
        Some(Outcome::Press(Button::Center, PressKind::Tap)),
        "a volume axis must not have moved the position off Center"
    );
}

/// The Up tap opens with `ABS_Y` alone (`docs/logs/beauty-r1-clean-run.log`),
/// and with signatures loaded its first delta resolves it. The opening value
/// still has to land on the tracker's own axis, or that delta is spent learning
/// it and the gesture needs another event.
#[test]
fn an_up_tap_resolves_on_its_first_delta() {
    let mut r = settling(0, 0);
    for event in [
        at(101.0, EventType::Key, "BTN_TOUCH", 1),
        at(101.0, EventType::Abs, "ABS_Y", 2036),
        at(101.0, EventType::Syn, "SYN_REPORT", 0),
    ] {
        assert_eq!(r.feed(&event), None, "{event:?}");
    }
    assert_eq!(
        r.feed(&at(101.03, EventType::Abs, "ABS_Y", 2124)),
        Some(Outcome::Press(Button::Up, PressKind::Tap))
    );
}

/// The gate's contract is that inside the window nothing is reported and
/// nothing changes the machine's state. The gesture here is open and has
/// delivered no delta yet, which with signatures loaded is one event short of
/// resolving: the delta inside the window must not reach the tracker, and the
/// first delta past the window is the one that resolves it, as a press rather
/// than as a tick of a gesture resolved behind the gate.
#[test]
fn the_settling_window_does_not_advance_a_gesture_in_flight() {
    let mut r = settling(0, 0);
    for event in [
        at(101.0, EventType::Key, "BTN_TOUCH", 1),
        at(101.0, EventType::Abs, "ABS_Y", 2036),
        at(101.0, EventType::Syn, "SYN_REPORT", 0),
    ] {
        assert_eq!(r.feed(&event), None, "{event:?}");
    }
    r.rearm_grace(101.05);
    assert_eq!(r.feed(&at(101.06, EventType::Abs, "ABS_Y", 2124)), None);
    assert_eq!(
        r.feed(&at(101.6, EventType::Abs, "ABS_Y", 2168)),
        Some(Outcome::Press(Button::Up, PressKind::Tap)),
        "the window must not have resolved the gesture behind the gate"
    );
}

/// The camera is the one button whose tap is reported on release, so the
/// release after a hold has to report an end instead, or a binding of
/// `camera:tap` runs at the end of every hold. The second gesture pins the
/// other half. The flag that remembers the hold is
/// cleared when a gesture opens, or every camera tap for the rest of the
/// session reports an end and does nothing.
#[test]
fn a_camera_hold_ends_and_leaves_the_next_tap_alone() {
    let mut r = recognizer();
    for event in [
        at(10.0, EventType::Key, "BTN_TOUCH", 1),
        at(10.0, EventType::Abs, "ABS_X", 2048),
        at(10.0, EventType::Abs, "ABS_Y", 3424),
        at(10.0, EventType::Syn, "SYN_REPORT", 0),
    ] {
        assert_eq!(r.feed(&event), None, "{event:?}");
    }
    assert_eq!(
        r.tick(10.5),
        Some(Outcome::Press(Button::Camera, PressKind::Hold))
    );
    assert_eq!(
        r.feed(&at(10.9, EventType::Key, "BTN_TOUCH", 0)),
        Some(Outcome::End(Button::Camera)),
        "the release after a hold must not repeat the tap action"
    );

    // A second camera gesture, past the 150 ms debounce, on the position the
    // first one left behind.
    for event in [
        at(11.5, EventType::Key, "BTN_TOUCH", 1),
        at(11.5, EventType::Syn, "SYN_REPORT", 0),
    ] {
        assert_eq!(r.feed(&event), None, "{event:?}");
    }
    assert_eq!(
        r.feed(&at(11.6, EventType::Key, "BTN_TOUCH", 0)),
        Some(Outcome::Press(Button::Camera, PressKind::Tap)),
        "the hold flag must have been cleared when this gesture opened"
    );
}

/// The same as `a_volume_repeat_inside_the_settling_window_slides_the_window`
/// for the other volume key. A held Right spans a reconnect exactly as a held
/// Left does, and `right:hold` can carry a key binding too, so both halves of
/// the gate's volume arming need pinning.
#[test]
fn a_volume_up_repeat_inside_the_settling_window_slides_the_window() {
    let settings = RecognizerSettings {
        debounce_ms: 150,
        connect_grace_ms: 500,
        camera_hold_ms: 200,
        volume_repeat_ms: 1500,
    };
    let mut r = Recognizer::new(settings, Vec::new(), 0.0);
    assert_eq!(
        r.feed(&volume_event(10.0, "KEY_VOLUMEUP", 1)),
        Some(Outcome::Press(Button::Right, PressKind::Hold))
    );
    assert_eq!(r.feed(&volume_event(11.1, "KEY_VOLUMEUP", 1)), None);

    r.reset();
    r.seed_position(None, None);
    r.rearm_grace(11.95);

    assert_eq!(r.feed(&volume_event(12.2, "KEY_VOLUMEUP", 1)), None);
    assert_eq!(
        r.feed(&volume_event(13.3, "KEY_VOLUMEUP", 1)),
        None,
        "the repeat inside the window re-armed the suppression window"
    );
}

/// Recorded from the remote on 2026-09-24, when about half of a series of
/// deliberate taps did nothing. It holds 15 touch cycles: 12 arrow gestures on
/// `ABS_Y` and 3 on `ABS_X`, each 270 to 330 ms long and carrying 9 to 11 axis
/// events, and ten of them open 30 ms after the previous release. Under the old
/// rule, which refused any gesture opening inside the 150 ms window, `replay`
/// resolved 9 of the 15. Every one of them carries movement and resolves a
/// direction, so every one of them is a real press.
#[test]
fn the_rapid_taps_all_resolve() {
    let mut r = shipped();
    let events = Event::parse_stream(include_str!("fixtures/beauty-r1-rapid-taps.jsonl"))
        .expect("the fixture parses");
    let outcomes: Vec<_> = events.iter().filter_map(|e| r.feed(e)).collect();

    let mut expected = vec![(Button::Down, PressKind::Tap); 12];
    expected.extend(vec![(Button::Right, PressKind::Tap); 3]);
    assert_eq!(presses(&outcomes), expected);
}

/// Recorded from the remote on 2026-10-02: four series of three Center presses
/// and five to seven Camera presses, 35 in all. Every Camera press is a tap, 60
/// to 151 ms long. In the second and the third series a button is pressed again
/// 59 to 149 ms after its last release.
const QUICK_REPEATS: &str = include_str!("fixtures/beauty-r1-quick-repeats.jsonl");

/// The taps of one series of that recording: Center, then Camera.
fn series(centers: usize, cameras: usize) -> Vec<(Button, PressKind)> {
    let mut taps = vec![(Button::Center, PressKind::Tap); centers];
    taps.extend(vec![(Button::Camera, PressKind::Tap); cameras]);
    taps
}

/// A quiet window of 150 ms, which `recognizer()` runs and which was the
/// default until 2026-10-10, refuses every other press of a button that is
/// pressed again that quickly: seven of the 35, two of Center and five of
/// Camera. Each is reported with the time since the release before it, and
/// the press after a refused one is a tap again.
#[test]
fn a_quiet_window_of_150_ms_refuses_every_other_quick_repeat() {
    let outcomes = run(QUICK_REPEATS);

    let expected = [series(3, 5), series(2, 4), series(2, 4), series(3, 5)].concat();
    assert_eq!(presses(&outcomes), expected);

    let refused: Vec<(Button, u32)> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Refused { button, after_ms } => Some((*button, *after_ms)),
            _ => None,
        })
        .collect();
    assert_eq!(
        refused,
        vec![
            (Button::Center, 120),
            (Button::Camera, 90),
            (Button::Camera, 90),
            (Button::Center, 149),
            (Button::Camera, 60),
            (Button::Camera, 59),
            (Button::Camera, 91),
        ]
    );
}

/// The same recording under the shipped quiet window, 45 ms since
/// 2026-10-10, which stands between a bounce, 30 ms after a release, and the
/// quickest repeat on record, 59 ms after one. Nothing is refused. Every other
/// test here runs the 150 ms of `recognizer()`, so this is the one that fails
/// when `debounce_ms` is not read.
#[test]
fn the_shipped_quiet_window_refuses_none_of_the_quick_repeats() {
    let mut r = shipped();
    let events = Event::parse_stream(QUICK_REPEATS).expect("the fixture parses");
    let outcomes: Vec<_> = events.iter().filter_map(|e| r.feed(e)).collect();

    let expected = [series(3, 5), series(3, 6), series(3, 7), series(3, 5)].concat();
    assert_eq!(presses(&outcomes), expected);
    assert!(
        !outcomes
            .iter()
            .any(|o| matches!(o, Outcome::Refused { .. })),
        "{outcomes:?}"
    );
}

/// The new rule at its boundary, on the measured numbers: a repeated tap and a
/// bounce cluster both arrive 30 ms after the previous release, so the window
/// cannot separate them by time. It separates them by shape instead. The
/// motionless half is refused, which
/// `a_bounced_gesture_yields_no_press_but_a_later_one_does` pins; an arrow
/// carries movement and resolves inside the window as it does outside it.
#[test]
fn an_arrow_inside_the_quiet_window_still_resolves() {
    let mut r = recognizer();
    for event in center_gesture(10.0, 10.05) {
        r.feed(&event);
    }

    // 30 ms after that release, well inside the 150 ms window.
    for event in [
        at(10.08, EventType::Key, "BTN_TOUCH", 1),
        at(10.08, EventType::Abs, "ABS_Y", 2176),
        at(10.08, EventType::Syn, "SYN_REPORT", 0),
    ] {
        assert_eq!(r.feed(&event), None, "{event:?}");
    }
    assert_eq!(
        r.feed(&at(10.11, EventType::Abs, "ABS_Y", 2088)),
        Some(Outcome::Press(Button::Down, PressKind::Tap))
    );
}

/// The condition itself: the same vertical gesture, one recognizer with the
/// measured signatures and one with none. With a table loaded the first delta
/// resolves it, which is the 30 ms this rule exists for. With no table every
/// gesture is a candidate signature, so the second delta is what resolves it and
/// a gesture that never agrees can still report itself as unmatched, which is
/// what `calibrate` collects.
#[test]
fn a_loaded_signature_table_is_what_makes_a_gesture_resolve_sooner() {
    let opening = [
        at(10.0, EventType::Key, "BTN_TOUCH", 1),
        at(10.0, EventType::Abs, "ABS_Y", 2176),
        at(10.0, EventType::Syn, "SYN_REPORT", 0),
    ];
    let first = at(10.03, EventType::Abs, "ABS_Y", 2088);
    let second = at(10.06, EventType::Abs, "ABS_Y", 2048);

    let mut loaded = recognizer();
    for event in &opening {
        assert_eq!(loaded.feed(event), None, "{event:?}");
    }
    assert_eq!(
        loaded.feed(&first),
        Some(Outcome::Press(Button::Down, PressKind::Tap))
    );

    let mut empty = without_signatures();
    for event in &opening {
        assert_eq!(empty.feed(event), None, "{event:?}");
    }
    assert_eq!(
        empty.feed(&first),
        None,
        "no table, so one delta is not enough"
    );
    assert_eq!(
        empty.feed(&second),
        Some(Outcome::Press(Button::Down, PressKind::Tap))
    );
}
