use crate::button::Button;
use crate::direction::{Axis, DirectionTracker, Vertical};
use crate::signature::{Signature, match_motionless};
use klik_core::press::PressKind;
use klik_core::raw::{Event, EventType};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecognizerSettings {
    pub debounce_ms: u32,
    pub connect_grace_ms: u32,
    pub camera_hold_ms: u32,
    /// Sliding window that suppresses the repeats a held Left or Right sends.
    /// Must exceed the measured 1.1 s repeat interval; see `on_volume`.
    pub volume_repeat_ms: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// A gesture resolved to a button.
    Press(Button, PressKind),
    /// A further axis event inside a resolved arrow gesture. Drives scrolling.
    Tick(Button),
    /// BTN_TOUCH went to 0 on a resolved gesture.
    End(Button),
    /// The opening packet matched no signature and the gesture never moved.
    Unmatched { x: Option<i32>, y: Option<i32> },
    /// A motionless press the quiet window refused. Nothing acts on it. It is
    /// reported so that the journal can name it, with the time since the
    /// release before it.
    Refused { button: Button, after_ms: u32 },
}

#[derive(Debug)]
enum State {
    Idle,
    /// Collecting the opening packet, before its SYN_REPORT.
    Opening {
        x: Option<i32>,
        y: Option<i32>,
    },
    /// The opening packet closed but no button is known yet. It carries no
    /// coordinates of its own: the position lives on the `Recognizer` and
    /// outlives every gesture, so duplicating it here could only drift.
    Resolving {
        tracker: DirectionTracker,
    },
    /// A button is known. Further axis events become ticks. Nothing re-reads
    /// direction once resolved, so unlike `Resolving` this carries no tracker.
    Resolved {
        button: Button,
    },
    /// A motionless gesture the quiet window refused. The refusal is reported
    /// when its opening packet closes. Nothing else is, its release included,
    /// and it arms nothing.
    Dropped,
}

#[derive(Debug)]
pub struct Recognizer {
    settings: RecognizerSettings,
    signatures: Vec<Signature>,
    grace_until: f64,
    state: State,
    /// Set while a motionless gesture is open, so its duration can be measured.
    camera_opened_at: Option<f64>,
    camera_hold_fired: bool,
    /// End of the sliding window that suppresses volume repeats. Deliberately
    /// NOT part of `state`: a volume hold produces no `BTN_TOUCH` at all, so it
    /// must not reach the touch path and swallow the next tap for a second and
    /// a half.
    volume_until: f64,
    /// End of the window that follows a gesture, within which a motionless
    /// gesture is refused.
    ///
    /// Measured on 2026-09-24: a deliberate repeated tap arrives 30 ms after
    /// the previous release, and the bounce clusters this window exists for sit
    /// at 30 ms as well, so no length of window tells them apart. Their shapes
    /// do. A bounce carries the opening packet and nothing else, which is the
    /// shape of a motionless press, so that is what the window refuses; an
    /// arrow carries movement, resolves a direction, and is reported whether
    /// the window is open or not. Refusing the motionless half is what matters,
    /// because that is where a doubled press is irreversible: Center sends a
    /// key, and Camera will start a dictation.
    quiet_until: f64,
    /// When `BTN_TOUCH` last went to 0, whichever gesture that ended. A refused
    /// press is reported with the time since then.
    released_at: f64,
    /// The pointer's last known absolute position, carried across gestures.
    ///
    /// The kernel reports an absolute axis only when its value differs from the
    /// one it last reported, and it does not reset that state when a gesture
    /// ends. A motionless button replays to its own coordinates and stops
    /// there, so pressing it twice in a row leaves the second gesture with no
    /// axis event at all. Matching that second gesture against its own packet
    /// finds nothing; matching it against the position the first one left
    /// behind finds the button. Measured in
    /// `tests/fixtures/beauty-r1-repeats.jsonl`.
    ///
    /// `None` means no value has ever been reported for that axis, which is
    /// true only before the first gesture that moves it. A gesture opening in
    /// that state is genuinely unidentifiable and is reported as unmatched.
    position_x: Option<i32>,
    position_y: Option<i32>,
}

impl Recognizer {
    pub fn new(settings: RecognizerSettings, signatures: Vec<Signature>, started_at: f64) -> Self {
        let grace_until = started_at + f64::from(settings.connect_grace_ms) / 1000.0;
        Self {
            settings,
            signatures,
            grace_until,
            state: State::Idle,
            camera_opened_at: None,
            camera_hold_fired: false,
            volume_until: f64::NEG_INFINITY,
            quiet_until: f64::NEG_INFINITY,
            released_at: f64::NEG_INFINITY,
            position_x: None,
            position_y: None,
        }
    }

    /// Tells the recognizer where the pointer already is.
    ///
    /// The kernel reports an absolute axis only when it changes, so a recognizer
    /// that has just started learns nothing until something moves. A motionless
    /// button pressed first is invisible to it. `EVIOCGABS` answers
    /// this at any time, so the daemon reads it when it opens the device and
    /// again after every reconnect. `None` leaves an axis unknown rather than
    /// guessing a value for it.
    ///
    /// Only valid with no gesture in flight. The position fields are read by
    /// the gesture currently resolving, so seeding mid-gesture changes that
    /// gesture's outcome rather than the next one's. A caller handling a
    /// reconnect calls `reset` first.
    pub fn seed_position(&mut self, x: Option<i32>, y: Option<i32>) {
        debug_assert!(
            matches!(self.state, State::Idle),
            "seed_position with a gesture in flight would change that gesture's outcome"
        );
        self.position_x = x;
        self.position_y = y;
    }

    /// Drops whatever gesture was in flight.
    ///
    /// A disconnect invalidates every gesture in progress: the `BTN_TOUCH = 0`
    /// that would have ended it is never coming. Without this the machine keeps
    /// a dead gesture across the gap. `camera_opened_at` in particular
    /// survives, so `tick` fires a camera hold, after the reconnect, for a press
    /// that ended when the remote went away.
    ///
    /// The position is deliberately left alone. It is a property of the device
    /// rather than of the gesture, and the caller replaces it with
    /// `seed_position` when the device comes back.
    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.camera_opened_at = None;
        self.camera_hold_fired = false;
    }

    /// Re-arms the settling window that follows a connect.
    ///
    /// `reset` cannot do this: it has no clock. The window matters after every
    /// connect and not merely the first, because a phantom gesture cluster
    /// appeared at the start of the first run after connecting and nothing says
    /// a reconnect is different.
    pub fn rearm_grace(&mut self, now: f64) {
        self.grace_until = now + f64::from(self.settings.connect_grace_ms) / 1000.0;
    }

    pub fn feed(&mut self, event: &Event) -> Option<Outcome> {
        // Inside the settling window no event changes the machine's state and
        // nothing is reported. An axis event still reaches the remembered
        // position, and a volume press still slides the window that suppresses
        // the repeats, exactly as it does outside the gate, where the release
        // arms nothing either. Both mirror state the device holds and have to
        // see every change.
        if event.time < self.grace_until {
            match event.event_type {
                EventType::Abs => {
                    self.remember(event);
                }
                EventType::Key
                    if event.value == 1
                        && (event.code == "KEY_VOLUMEUP" || event.code == "KEY_VOLUMEDOWN") =>
                {
                    self.arm_volume(event.time);
                }
                _ => {}
            }
            return None;
        }
        match event.event_type {
            EventType::Key if event.code == "BTN_TOUCH" => self.on_touch(event),
            EventType::Abs => self.on_axis(event),
            EventType::Syn => self.on_report(event),
            EventType::Key if event.code == "KEY_VOLUMEUP" && event.value == 1 => {
                self.on_volume(Button::Right, event)
            }
            EventType::Key if event.code == "KEY_VOLUMEDOWN" && event.value == 1 => {
                self.on_volume(Button::Left, event)
            }
            _ => None,
        }
    }

    /// Called by the daemon loop when no event has arrived. A held camera sends
    /// nothing at all between its edges, so its hold cannot be seen any other way.
    pub fn tick(&mut self, now: f64) -> Option<Outcome> {
        let opened_at = self.camera_opened_at?;
        if self.camera_hold_fired {
            return None;
        }
        if now - opened_at >= f64::from(self.settings.camera_hold_ms) / 1000.0 {
            self.camera_hold_fired = true;
            return Some(Outcome::Press(Button::Camera, PressKind::Hold));
        }
        None
    }

    fn on_touch(&mut self, event: &Event) -> Option<Outcome> {
        if event.value == 1 {
            // Every gesture opens, inside the quiet window or not. What the
            // window refuses is decided on the report, where a motionless
            // gesture is told from an arrow; see `quiet_until`.
            self.state = State::Opening { x: None, y: None };
            self.camera_opened_at = None;
            self.camera_hold_fired = false;
            return None;
        }

        // BTN_TOUCH = 0 is decided on this key event rather than on the
        // SYN_REPORT that closes it. The report-boundary rule exists because an
        // opening packet's axis values can still be arriving later in the same
        // report; here there is none of that left to wait for. Every branch
        // below reads only state a previous report already settled, never axis
        // data from this report, so the hazard the rule guards against does not
        // apply here.
        self.released_at = event.time;
        let outcome = match &self.state {
            State::Resolved { button } => {
                let button = *button;
                let camera_was_short = button == Button::Camera && !self.camera_hold_fired;
                // The camera is the one button whose tap cannot be reported when
                // the gesture opens: a hold looks identical until the timer says
                // otherwise, and reporting the tap first would jump to a waiting
                // session on every dictation. Its tap is therefore delivered on
                // release instead.
                if camera_was_short {
                    Some(Outcome::Press(Button::Camera, PressKind::Tap))
                } else {
                    Some(Outcome::End(button))
                }
            }
            // The opening packet matched no signature and the direction tracker
            // never agreed on one before release. Report what was seen instead
            // of letting the gesture disappear with no outcome at all; this is
            // the line a miscalibrated remote is diagnosed from.
            State::Resolving { .. } => Some(Outcome::Unmatched {
                x: self.position_x,
                y: self.position_y,
            }),
            // Nothing was in flight: this release belongs to a gesture the
            // settling window already swallowed, to one the quiet window
            // refused, or to no gesture at all. Nothing ended here, so there is
            // nothing to report, and a refused gesture cannot push the quiet
            // window out with its own release.
            State::Idle | State::Dropped => return None,
            // Opened and never closed its packet. Nothing to report, but a
            // gesture did end here.
            State::Opening { .. } => None,
        };
        self.state = State::Idle;
        self.quiet_until = event.time + f64::from(self.settings.debounce_ms) / 1000.0;
        self.camera_opened_at = None;
        outcome
    }

    /// Copies an `ABS_X` or `ABS_Y` value into the remembered position, and
    /// says which axis it was. Any other absolute axis is ignored.
    ///
    /// Every axis event goes through here, whatever the state, inside the
    /// settling window too, and whether or not its gesture is ever identified.
    /// The kernel keeps this state for us and only tells us about changes, so
    /// dropping an update would desynchronize us from it permanently.
    fn remember(&mut self, event: &Event) -> Option<Axis> {
        let axis = match event.code.as_str() {
            "ABS_X" => Axis::X,
            "ABS_Y" => Axis::Y,
            _ => return None,
        };
        match axis {
            Axis::X => self.position_x = Some(event.value),
            Axis::Y => self.position_y = Some(event.value),
        }
        Some(axis)
    }

    fn on_axis(&mut self, event: &Event) -> Option<Outcome> {
        let axis = self.remember(event)?;

        // The new state is decided inside the match and applied after it, because
        // assigning `self.state` while the match still borrows it does not compile.
        enum Step {
            Nothing,
            Resolved(Button),
            Tick(Button),
        }

        let step = match &mut self.state {
            State::Opening { x, y } => {
                match axis {
                    Axis::X => *x = Some(event.value),
                    Axis::Y => *y = Some(event.value),
                }
                Step::Nothing
            }
            State::Resolving { tracker } => match tracker.feed(axis, event.value) {
                Some(button) => Step::Resolved(button),
                None => Step::Nothing,
            },
            // Once resolved, no later axis event can change the button: nothing
            // reads a direction again after this point, only whether one more
            // event arrived at all. See `State::Resolved`.
            State::Resolved { button } => Step::Tick(*button),
            _ => Step::Nothing,
        };

        match step {
            Step::Nothing => None,
            Step::Resolved(button) => {
                self.state = State::Resolved { button };
                Some(Outcome::Press(button, PressKind::Tap))
            }
            Step::Tick(button) => Some(Outcome::Tick(button)),
        }
    }

    fn on_report(&mut self, event: &Event) -> Option<Outcome> {
        let (x, y) = match self.state {
            State::Opening { x, y } => (x, y),
            _ => return None,
        };

        // Matched against the remembered position rather than against this
        // packet. A packet carries only the axes that changed, so a repeated
        // motionless press carries none at all; see `position_x`.
        if let Some(button) = match_motionless(&self.signatures, self.position_x, self.position_y) {
            // The one thing the quiet window refuses. A bounce cluster has
            // exactly this shape, the opening packet and no movement, so a
            // repeated motionless press cannot be told from one. An arrow
            // falls through to the tracker below and resolves either way.
            if event.time < self.quiet_until {
                self.state = State::Dropped;
                let after_ms = ((event.time - self.released_at) * 1000.0).round() as u32;
                return Some(Outcome::Refused { button, after_ms });
            }
            self.state = State::Resolved { button };
            if button == Button::Camera {
                // Nothing is reported yet. Whether this is a tap or a hold is
                // decided by `tick`, or by the release arriving first.
                self.camera_opened_at = Some(event.time);
                return None;
            }
            return Some(Outcome::Press(button, PressKind::Tap));
        }

        // The tracker is seeded from this packet rather than from the remembered
        // position, deliberately. It reads deltas within one gesture, and a delta
        // measured against where the previous gesture ended is not one of them.
        self.state = State::Resolving {
            tracker: DirectionTracker::new(x, y, self.vertical()),
        };
        None
    }

    /// Which vertical rule a gesture of this recognizer resolves under.
    ///
    /// An empty signature table means `calibrate`: nothing can match, so every
    /// gesture is a candidate signature and has to be able to report itself as
    /// unmatched rather than being claimed as an arrow. With a table loaded a
    /// vertical gesture resolves one event sooner; see `Vertical`.
    fn vertical(&self) -> Vertical {
        if self.signatures.is_empty() {
            Vertical::OnTwoDeltas
        } else {
            Vertical::OnFirstDelta
        }
    }

    /// A held Left or Right sends a complete press and release pair about every
    /// 1.1 s, so the bounce debounce is far too short to make the hold fire once.
    /// The window here is its own, longer than the repeat interval, and sliding:
    /// every volume event re-arms it whether or not that event fired. A fixed
    /// window would expire in the middle of a long hold and fire again.
    fn on_volume(&mut self, button: Button, event: &Event) -> Option<Outcome> {
        let suppressed = event.time < self.volume_until;
        self.arm_volume(event.time);
        if suppressed {
            return None;
        }
        Some(Outcome::Press(button, PressKind::Hold))
    }

    /// Arms the sliding window that suppresses volume repeats.
    ///
    /// Shared with the settling gate, because a repeat dropped there is still a
    /// volume press. The window slides on every one of them or a hold that
    /// spans a reconnect fires a second time.
    fn arm_volume(&mut self, now: f64) {
        self.volume_until = now + f64::from(self.settings.volume_repeat_ms) / 1000.0;
    }
}
