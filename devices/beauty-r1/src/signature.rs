use crate::button::Button;

/// A motionless gesture, identified by where it lands rather than how it moves.
/// Only the center and the camera buttons have one.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub button: Button,
    pub x: i32,
    pub y: i32,
    pub tolerance: i32,
}

/// Matches an opening packet against the motionless signatures.
///
/// Every axis present in the packet must fall within tolerance, and at least
/// one axis must be present. evdev reports only axes that changed, so a packet
/// carrying one axis is normal and must still match.
pub fn match_motionless(
    signatures: &[Signature],
    x: Option<i32>,
    y: Option<i32>,
) -> Option<Button> {
    if x.is_none() && y.is_none() {
        return None;
    }
    signatures
        .iter()
        .find(|s| {
            let near = |v: i32, target: i32| (v - target).abs() <= s.tolerance;
            x.is_none_or(|v| near(v, s.x)) && y.is_none_or(|v| near(v, s.y))
        })
        .map(|s| s.button)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::Button;

    fn signatures() -> Vec<Signature> {
        vec![
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
        ]
    }

    #[test]
    fn center_matches_on_both_axes() {
        assert_eq!(
            match_motionless(&signatures(), Some(1904), Some(1904)),
            Some(Button::Center)
        );
    }

    #[test]
    fn camera_matches_on_both_axes() {
        assert_eq!(
            match_motionless(&signatures(), Some(2048), Some(3424)),
            Some(Button::Camera)
        );
    }

    /// In `beauty-r1-hold-first.log` the center tap reported only ABS_Y,
    /// because ABS_X had not changed since the previous gesture.
    #[test]
    fn center_matches_with_only_the_y_axis_present() {
        assert_eq!(
            match_motionless(&signatures(), None, Some(1904)),
            Some(Button::Center)
        );
    }

    /// The Up tap opens at ABS_X 1904, which is the center's X. Only Y separates them.
    #[test]
    fn up_is_not_mistaken_for_center() {
        assert_eq!(
            match_motionless(&signatures(), Some(1904), Some(2036)),
            None
        );
    }

    #[test]
    fn held_arrows_match_nothing() {
        assert_eq!(
            match_motionless(&signatures(), None, Some(784)),
            None,
            "held Up"
        );
        assert_eq!(
            match_motionless(&signatures(), None, Some(2912)),
            None,
            "held Down"
        );
    }

    #[test]
    fn a_packet_with_no_axes_matches_nothing() {
        assert_eq!(match_motionless(&signatures(), None, None), None);
    }

    /// Tolerance boundary on Y axis: difference equal to tolerance must match.
    /// Synthesized value: Center at Y=1904, tolerance=16, so Y=1920 is exactly at the boundary.
    #[test]
    fn center_matches_at_y_tolerance_boundary() {
        assert_eq!(
            match_motionless(&signatures(), Some(1904), Some(1920)),
            Some(Button::Center),
            "Y difference of 16 should match tolerance=16"
        );
    }

    /// Tolerance boundary on Y axis: difference of one more than tolerance must not match.
    /// Synthesized value: Center at Y=1904, tolerance=16, so Y=1921 is one past the boundary.
    #[test]
    fn center_rejects_just_past_y_tolerance_boundary() {
        assert_eq!(
            match_motionless(&signatures(), Some(1904), Some(1921)),
            None,
            "Y difference of 17 should exceed tolerance=16"
        );
    }

    /// Tolerance boundary on X axis: difference equal to tolerance must match.
    /// Synthesized value: Center at X=1904, tolerance=16, so X=1920 is exactly at the boundary.
    #[test]
    fn center_matches_at_x_tolerance_boundary() {
        assert_eq!(
            match_motionless(&signatures(), Some(1920), Some(1904)),
            Some(Button::Center),
            "X difference of 16 should match tolerance=16"
        );
    }

    /// Tolerance boundary on X axis: difference of one more than tolerance must not match.
    /// Synthesized value: Center at X=1904, tolerance=16, so X=1921 is one past the boundary.
    #[test]
    fn center_rejects_just_past_x_tolerance_boundary() {
        assert_eq!(
            match_motionless(&signatures(), Some(1921), Some(1904)),
            None,
            "X difference of 17 should exceed tolerance=16"
        );
    }
}
