use std::io;

use klik_core::module::Rect;
use klik_core::platform::{Process, Run};
use serde::Deserialize;

use crate::HELPER_LIMIT;

#[derive(Deserialize)]
struct Outputs {
    outputs: Vec<Output>,
}

#[derive(Deserialize)]
struct Output {
    enabled: bool,
    pos: Position,
    size: Size,
}

#[derive(Deserialize)]
struct Position {
    x: i32,
    y: i32,
}

#[derive(Deserialize)]
struct Size {
    width: i32,
    height: i32,
}

/// The first enabled output, from `kscreen-doctor -j`.
///
/// One output is what this machine has, and the first enabled one is a stated
/// rule rather than a guess at what a multi-monitor layout should do. Returns
/// `None` rather than a default rectangle: a wrong screen size places the cursor
/// in the wrong place silently, which is worse than refusing to place it.
pub fn parse(json: &str) -> Option<Rect> {
    let parsed: Outputs = serde_json::from_str(json).ok()?;
    parsed
        .outputs
        .into_iter()
        .find(|o| o.enabled)
        .map(|o| Rect {
            x: o.pos.x,
            y: o.pos.y,
            width: o.size.width,
            height: o.size.height,
        })
}

/// Asks the compositor, with the limit of every helper. The call measured
/// 0.06 s on 2026-09-27, more than the rest of a scroll series put together,
/// so the desktop asks once and keeps the answer: a daemon that outlives a
/// resolution change aims at the old geometry until it is restarted.
pub fn current(process: &mut dyn Process) -> io::Result<Option<Rect>> {
    let run = Run {
        program: "kscreen-doctor".to_string(),
        args: vec!["-j".to_string()],
        env: Vec::new(),
        limit: HELPER_LIMIT,
    };
    let out = process.output(&run)?;
    Ok(parse(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `kscreen-doctor -j` printed on 2026-09-19.
    const SAMPLE: &str = r#"{
        "outputs": [
            {
                "name": "eDP-1",
                "enabled": true,
                "pos": { "x": 0, "y": 0 },
                "size": { "height": 1080, "width": 1920 },
                "scale": 1
            }
        ]
    }"#;

    #[test]
    fn the_enabled_output_is_the_screen() {
        assert_eq!(
            parse(SAMPLE),
            Some(Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            })
        );
    }

    /// A disabled output has no geometry worth aiming into.
    #[test]
    fn a_disabled_output_is_skipped() {
        let json = SAMPLE.replace("\"enabled\": true", "\"enabled\": false");
        assert_eq!(parse(&json), None);
    }

    #[test]
    fn the_first_enabled_output_wins() {
        let json = r#"{
            "outputs": [
                { "name": "a", "enabled": false, "pos": { "x": 0, "y": 0 },
                  "size": { "width": 100, "height": 100 }, "scale": 1 },
                { "name": "b", "enabled": true, "pos": { "x": 7, "y": 9 },
                  "size": { "width": 200, "height": 300 }, "scale": 1 }
            ]
        }"#;
        assert_eq!(
            parse(json),
            Some(Rect {
                x: 7,
                y: 9,
                width: 200,
                height: 300
            })
        );
    }

    /// The test above alternates a disabled entry with an enabled one, which
    /// proves a disabled entry is skipped but not that the first of two
    /// enabled entries is the one chosen over the second.
    #[test]
    fn the_first_of_two_enabled_outputs_wins() {
        let json = r#"{
            "outputs": [
                { "name": "a", "enabled": true, "pos": { "x": 1, "y": 2 },
                  "size": { "width": 300, "height": 400 }, "scale": 1 },
                { "name": "b", "enabled": true, "pos": { "x": 7, "y": 9 },
                  "size": { "width": 200, "height": 300 }, "scale": 1 }
            ]
        }"#;
        assert_eq!(
            parse(json),
            Some(Rect {
                x: 1,
                y: 2,
                width: 300,
                height: 400
            })
        );
    }

    #[test]
    fn output_that_is_not_json_is_refused_rather_than_guessed() {
        assert_eq!(parse("kscreen-doctor: command not found"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn json_without_the_fields_this_needs_is_refused() {
        assert_eq!(parse(r#"{"outputs": [{"name": "eDP-1"}]}"#), None);
    }

    /// Each numeric field is required on its own: a `#[serde(default)]` on
    /// any one of them would let a reply missing only that field deserialize
    /// with a zero in its place.
    #[test]
    fn an_output_missing_only_x_is_refused() {
        let json = SAMPLE.replace("\"x\": 0, ", "");
        assert_eq!(parse(&json), None);
    }

    #[test]
    fn an_output_missing_only_y_is_refused() {
        let json = SAMPLE.replace(", \"y\": 0", "");
        assert_eq!(parse(&json), None);
    }

    #[test]
    fn an_output_missing_only_width_is_refused() {
        let json = SAMPLE.replace(", \"width\": 1920", "");
        assert_eq!(parse(&json), None);
    }

    #[test]
    fn an_output_missing_only_height_is_refused() {
        let json = SAMPLE.replace("\"height\": 1080, ", "");
        assert_eq!(parse(&json), None);
    }
}
