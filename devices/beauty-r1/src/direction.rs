use crate::button::Button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

/// Whether one delta on `ABS_Y` is enough to name a vertical arrow.
///
/// `OnFirstDelta` is what the daemon runs with a signature table loaded. It
/// takes one axis event off the wait between a press and the first scroll,
/// about 30 ms at the measured 33 events a second. Two measurements carry it.
/// Every `ABS_Y` gesture in all four captures opens with its first two deltas
/// sharing a sign, at 88, -88 or 16 units. And Right opens 72 and then -300, so
/// its first delta points at Left, which is why the horizontal axis can never
/// take this.
///
/// `OnTwoDeltas` is what `calibrate` runs with, because it loads no signatures.
/// Nothing can match there, so every gesture is a candidate signature and has
/// to be able to report itself as unmatched.
///
/// Two costs come with `OnFirstDelta`, and neither appears in any capture. A
/// first delta that points the wrong way now decides the whole gesture, because
/// nothing re-reads the direction once it resolves, so one tap scrolls about
/// three notches the wrong way rather than moving one event. And a motionless
/// press that misses its signature, which means a configuration that has
/// drifted, scrolls once instead of printing the `unmatched` line that carries
/// the numbers a `[[signature]]` section needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vertical {
    OnFirstDelta,
    OnTwoDeltas,
}

/// Resolves an arrow from the direction its axis travels.
///
/// The opening value of a gesture is not usable for identification: a tapped Up
/// opens at 2036 and a held Up opens at 784. The direction is the same in both.
/// The horizontal axis always needs two consecutive deltas of the same sign,
/// because the Right button opens with one step the other way. The vertical axis
/// needs one or two according to `Vertical`.
#[derive(Debug)]
pub struct DirectionTracker {
    last_x: Option<i32>,
    last_y: Option<i32>,
    agreed_axis: Option<Axis>,
    agreed_sign: i32,
    vertical: Vertical,
}

impl DirectionTracker {
    pub fn new(open_x: Option<i32>, open_y: Option<i32>, vertical: Vertical) -> Self {
        Self {
            last_x: open_x,
            last_y: open_y,
            agreed_axis: None,
            agreed_sign: 0,
            vertical,
        }
    }

    pub fn feed(&mut self, axis: Axis, value: i32) -> Option<Button> {
        let last = match axis {
            Axis::X => self.last_x.replace(value),
            Axis::Y => self.last_y.replace(value),
        };
        let delta = value - last?;
        let sign = delta.signum();
        if sign == 0 {
            return None;
        }

        let agrees = self.agreed_axis == Some(axis) && self.agreed_sign == sign;
        self.agreed_axis = Some(axis);
        self.agreed_sign = sign;
        let enough = agrees || (axis == Axis::Y && self.vertical == Vertical::OnFirstDelta);
        if !enough {
            return None;
        }

        Some(match (axis, sign) {
            (Axis::Y, s) if s > 0 => Button::Up,
            (Axis::Y, _) => Button::Down,
            (Axis::X, s) if s > 0 => Button::Left,
            (Axis::X, _) => Button::Right,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds a series of absolute values on one axis and returns the button the
    /// tracker resolves, plus how many values it needed. The rule under test is
    /// the first argument, because it is what these cases are about.
    fn resolve(
        vertical: Vertical,
        axis: Axis,
        open: i32,
        values: &[i32],
    ) -> Option<(Button, usize)> {
        let (x, y) = match axis {
            Axis::X => (Some(open), None),
            Axis::Y => (None, Some(open)),
        };
        let mut tracker = DirectionTracker::new(x, y, vertical);
        for (i, v) in values.iter().enumerate() {
            if let Some(button) = tracker.feed(axis, *v) {
                return Some((button, i + 1));
            }
        }
        None
    }

    #[test]
    fn up_tap_resolves_on_its_first_delta() {
        // Measured from docs/logs/beauty-r1-clean-run.log
        assert_eq!(
            resolve(Vertical::OnFirstDelta, Axis::Y, 2036, &[2124, 2168, 2212]),
            Some((Button::Up, 1))
        );
    }

    /// `calibrate`'s rule, and the reason the fast one is conditional: with no
    /// signatures loaded the same tap waits for the second delta.
    #[test]
    fn up_tap_resolves_on_the_second_delta_without_signatures() {
        assert_eq!(
            resolve(Vertical::OnTwoDeltas, Axis::Y, 2036, &[2124, 2168, 2212]),
            Some((Button::Up, 2))
        );
    }

    #[test]
    fn down_tap_resolves_on_its_first_delta() {
        // Measured from docs/logs/beauty-r1-clean-run.log
        assert_eq!(
            resolve(Vertical::OnFirstDelta, Axis::Y, 2176, &[2088, 2048, 2000]),
            Some((Button::Down, 1))
        );
    }

    #[test]
    fn down_tap_resolves_on_the_second_delta_without_signatures() {
        assert_eq!(
            resolve(Vertical::OnTwoDeltas, Axis::Y, 2176, &[2088, 2048, 2000]),
            Some((Button::Down, 2))
        );
    }

    /// The horizontal axis waits for two deltas under either rule.
    #[test]
    fn left_tap_resolves_on_the_second_delta() {
        // Measured from docs/logs/beauty-r1-clean-run.log
        assert_eq!(
            resolve(Vertical::OnFirstDelta, Axis::X, 512, &[650, 948, 1250]),
            Some((Button::Left, 2))
        );
    }

    /// The whole reason the horizontal axis cannot take the fast rule: Right
    /// steps +72 first, so its first delta points at Left.
    #[test]
    fn right_survives_its_opening_step_in_the_wrong_direction() {
        // Measured from docs/logs/beauty-r1-clean-run.log
        assert_eq!(
            resolve(Vertical::OnFirstDelta, Axis::X, 3328, &[3400, 3100, 2800]),
            Some((Button::Right, 3))
        );
    }

    #[test]
    fn held_up_resolves_from_its_sixteen_unit_steps() {
        // Measured from docs/logs/beauty-r1-hold-map.log
        assert_eq!(
            resolve(Vertical::OnFirstDelta, Axis::Y, 784, &[800, 816, 832]),
            Some((Button::Up, 1))
        );
    }

    #[test]
    fn held_down_resolves_from_its_sixteen_unit_steps() {
        // Measured from docs/logs/beauty-r1-hold-map.log
        assert_eq!(
            resolve(Vertical::OnFirstDelta, Axis::Y, 2912, &[2896, 2880, 2864]),
            Some((Button::Down, 1))
        );
    }

    #[test]
    fn repeated_values_do_not_resolve() {
        assert_eq!(
            resolve(Vertical::OnFirstDelta, Axis::Y, 2036, &[2036, 2036, 2036]),
            None
        );
    }

    #[test]
    fn a_single_horizontal_delta_is_not_enough() {
        assert_eq!(resolve(Vertical::OnFirstDelta, Axis::X, 512, &[650]), None);
    }

    #[test]
    fn a_single_vertical_delta_is_not_enough_without_signatures() {
        assert_eq!(resolve(Vertical::OnTwoDeltas, Axis::Y, 2036, &[2124]), None);
    }

    #[test]
    fn same_sign_continuation_keeps_returning_button() {
        // Every upward delta of a held Up keeps returning Up, the first one
        // included under the fast rule.
        let mut tracker = DirectionTracker::new(None, Some(784), Vertical::OnFirstDelta);
        assert_eq!(tracker.feed(Axis::Y, 800), Some(Button::Up));
        assert_eq!(tracker.feed(Axis::Y, 816), Some(Button::Up));
        assert_eq!(tracker.feed(Axis::Y, 832), Some(Button::Up));
        assert_eq!(tracker.feed(Axis::Y, 848), Some(Button::Up));
    }

    /// The first cost of the fast rule, stated as a test: one reversing delta
    /// switches the button. `Recognizer` never asks again once a gesture has
    /// resolved, so within a gesture this is reached only before resolution.
    #[test]
    fn sign_reversal_switches_button_at_once() {
        let mut tracker = DirectionTracker::new(None, Some(784), Vertical::OnFirstDelta);
        assert_eq!(tracker.feed(Axis::Y, 800), Some(Button::Up));
        assert_eq!(tracker.feed(Axis::Y, 784), Some(Button::Down));
    }

    /// The same reversal under `calibrate`'s rule: two agreeing deltas, so a
    /// single step the other way names nothing.
    #[test]
    fn sign_reversal_needs_two_deltas_without_signatures() {
        let mut tracker = DirectionTracker::new(None, Some(784), Vertical::OnTwoDeltas);
        assert_eq!(tracker.feed(Axis::Y, 800), None);
        assert_eq!(tracker.feed(Axis::Y, 816), Some(Button::Up));
        assert_eq!(tracker.feed(Axis::Y, 800), None);
        assert_eq!(tracker.feed(Axis::Y, 784), Some(Button::Down));
    }

    #[test]
    fn different_axis_does_not_agree_with_prior_delta() {
        // A delta on one axis must not agree with stored deltas on another axis.
        // Resolve on Y with an upward delta, then feed negative deltas on X.
        let mut tracker = DirectionTracker::new(Some(512), Some(2036), Vertical::OnFirstDelta);
        assert_eq!(tracker.feed(Axis::Y, 2124), Some(Button::Up));
        // First X delta must not resolve: there is no prior X agreement, and the
        // Y agreement was positive.
        assert_eq!(tracker.feed(Axis::X, 400), None);
        assert_eq!(tracker.feed(Axis::X, 300), Some(Button::Right));
    }
}
