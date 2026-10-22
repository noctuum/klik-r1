use crate::press::{ButtonName, PressKind, Recognized};

/// How far into its gesture each report is, counting the press as 1.
///
/// The scroll rate rises with this number, so it has to restart on every
/// press of the button it is counting. Two taps in a row are two gestures:
/// without the restart the second one would inherit the speed the first
/// reached and move a different distance for the same movement of the hand.
///
/// It records which button it is counting, and a press or an end of any other
/// button leaves the count alone. A held Left of the Beauty-R1 reports its
/// press whether or not a swipe is already streaming, and a counter that
/// restarted on that press would drop a held arrow back to the base rate in
/// the middle of the hold.
#[derive(Debug, Default)]
pub struct Gesture {
    button: Option<ButtonName>,
    events: u32,
    /// The kind of the last press of each button, counted or not.
    kinds: Vec<(ButtonName, PressKind)>,
}

impl Gesture {
    pub fn new() -> Self {
        Self::default()
    }

    /// The kind of the last press of `button`, which is the kind its ticks and
    /// its end belong to.
    pub fn kind_of(&self, button: &str) -> Option<PressKind> {
        self.kinds
            .iter()
            .find(|(name, _)| *name == button)
            .map(|(_, kind)| *kind)
    }

    /// The index of this report inside its gesture, or 0 for a report that
    /// is not part of the gesture being counted.
    pub fn note(&mut self, recognized: &Recognized) -> u32 {
        match recognized {
            Recognized::Press(button, kind) => {
                match self.kinds.iter_mut().find(|(name, _)| name == button) {
                    Some(entry) => entry.1 = *kind,
                    None => self.kinds.push((*button, *kind)),
                }
                if self.button.is_some() && self.button != Some(*button) {
                    return 0;
                }
                self.button = Some(*button);
                self.events = 1;
                self.events
            }
            // A tick belongs to a resolved gesture, so one for another
            // button means that gesture is the live one and this counter
            // follows it. That is also what recovers from a press of the
            // Beauty-R1's camera, which never reports an end and would
            // otherwise hold this counter forever.
            Recognized::Tick(button) => {
                if self.button == Some(*button) {
                    self.events = self.events.saturating_add(1);
                } else {
                    self.button = Some(*button);
                    self.events = 1;
                }
                self.events
            }
            Recognized::End(button) => {
                if self.button == Some(*button) {
                    self.button = None;
                    self.events = 0;
                }
                0
            }
            Recognized::Unmatched(_) | Recognized::Refused { .. } => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::press::PressKind;

    /// The press is report 1, and the ticks that follow it count up. This is
    /// what makes the rate a function of time inside one gesture.
    #[test]
    fn a_press_starts_the_count_and_ticks_continue_it() {
        let mut gesture = Gesture::new();
        assert_eq!(gesture.note(&Recognized::Press("up", PressKind::Tap)), 1);
        assert_eq!(gesture.note(&Recognized::Tick("up")), 2);
        assert_eq!(gesture.note(&Recognized::Tick("up")), 3);
    }

    /// Without this a second tap inherits the speed the first one reached, so
    /// two taps in a row scroll different distances for the same gesture.
    #[test]
    fn a_new_press_restarts_the_count() {
        let mut gesture = Gesture::new();
        gesture.note(&Recognized::Press("down", PressKind::Tap));
        for _ in 0..20 {
            gesture.note(&Recognized::Tick("down"));
        }
        assert_eq!(gesture.note(&Recognized::Press("down", PressKind::Tap)), 1);
    }

    /// An end is the gesture finishing. Nothing scrolls on it, and the count
    /// it leaves behind must not be inherited by whatever arrives next.
    #[test]
    fn an_end_clears_the_count() {
        let mut gesture = Gesture::new();
        gesture.note(&Recognized::Press("up", PressKind::Tap));
        gesture.note(&Recognized::Tick("up"));
        assert_eq!(gesture.note(&Recognized::End("up")), 0);
        assert_eq!(gesture.note(&Recognized::Tick("up")), 1);
    }

    /// A held Left of the Beauty-R1 can report its press while an Up swipe is
    /// already streaming. That press must not reset the arrow's count back to
    /// the base rate.
    #[test]
    fn a_press_of_another_button_does_not_restart_an_open_count() {
        let mut gesture = Gesture::new();
        gesture.note(&Recognized::Press("up", PressKind::Tap));
        for _ in 0..14 {
            gesture.note(&Recognized::Tick("up"));
        }
        gesture.note(&Recognized::Press("left", PressKind::Hold));
        assert_eq!(gesture.note(&Recognized::Tick("up")), 16);
    }

    /// The camera of the Beauty-R1 never reports an end, so a tick of another
    /// button is what recovers this counter from a camera press before it.
    #[test]
    fn a_tick_of_another_button_takes_over_the_count() {
        let mut gesture = Gesture::new();
        gesture.note(&Recognized::Press("camera", PressKind::Hold));
        assert_eq!(gesture.note(&Recognized::Tick("up")), 1);
    }

    /// An end belongs to the gesture it names. One for a button this counter
    /// is not counting says nothing about the count in progress.
    #[test]
    fn an_end_of_another_button_leaves_the_count_alone() {
        let mut gesture = Gesture::new();
        gesture.note(&Recognized::Press("up", PressKind::Tap));
        gesture.note(&Recognized::Tick("up"));
        gesture.note(&Recognized::End("left"));
        assert_eq!(gesture.note(&Recognized::Tick("up")), 3);
    }

    /// A tick and an end are looked up under the kind of their own button's
    /// press, which a press of another button in the middle does not change.
    #[test]
    fn the_kind_of_a_button_is_that_of_its_last_press() {
        let mut gesture = Gesture::new();
        assert_eq!(gesture.kind_of("up"), None);
        gesture.note(&Recognized::Press("up", PressKind::Hold));
        gesture.note(&Recognized::Press("left", PressKind::Tap));
        assert_eq!(gesture.kind_of("up"), Some(PressKind::Hold));
        assert_eq!(gesture.kind_of("left"), Some(PressKind::Tap));
        gesture.note(&Recognized::Press("up", PressKind::Tap));
        assert_eq!(gesture.kind_of("up"), Some(PressKind::Tap));
    }

    /// A refused press belongs to no gesture. Counted as a press of its
    /// button it would hold the counter, and the press of an arrow after it
    /// would not start a count.
    #[test]
    fn a_refused_press_is_not_counted() {
        let mut gesture = Gesture::new();
        let refused = Recognized::Refused {
            button: "center",
            after_ms: 30,
        };
        assert_eq!(gesture.note(&refused), 0);
        assert_eq!(gesture.note(&Recognized::Press("up", PressKind::Tap)), 1);
    }
}
