use serde::{Deserialize, Serialize};

use crate::error::EventError;

/// One evdev event as it appears in a recorded fixture.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Event {
    pub time: f64,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub code: String,
    pub value: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum EventType {
    #[serde(rename = "EV_KEY")]
    Key,
    #[serde(rename = "EV_ABS")]
    Abs,
    #[serde(rename = "EV_SYN")]
    Syn,
    #[serde(rename = "EV_MSC")]
    Msc,
    #[serde(rename = "EV_REL")]
    Rel,
}

impl Event {
    pub fn parse_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }

    /// Parses a JSON Lines stream, skipping blank lines.
    pub fn parse_stream(text: &str) -> Result<Vec<Self>, EventError> {
        text.lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(line, text)| {
                Self::parse_line(text).map_err(|source| EventError::Json {
                    line: line + 1,
                    source,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The opening packet of a capture, as the recorder writes it.
    const OPENING_PACKET: &str = r#"{"time":1787594305.903279,"type":"EV_KEY","code":"BTN_TOUCH","value":1}
{"time":1787594305.903279,"type":"EV_ABS","code":"ABS_X","value":1904}
{"time":1787594305.903279,"type":"EV_ABS","code":"ABS_Y","value":2036}
{"time":1787594305.903279,"type":"EV_SYN","code":"SYN_REPORT","value":0}"#;

    #[test]
    fn parses_one_line() {
        // Synthesized: the parser's successful-parse path on a single well-formed event.
        // No capture contains an isolated line.
        let e = Event::parse_line(r#"{"time":1.5,"type":"EV_ABS","code":"ABS_Y","value":2036}"#)
            .expect("well formed line must parse");
        assert_eq!(e.time, 1.5);
        assert_eq!(e.event_type, EventType::Abs);
        assert_eq!(e.code, "ABS_Y");
        assert_eq!(e.value, 2036);
    }

    #[test]
    fn parses_a_whole_report() {
        // Fixture from design spec, used to verify multi-event report parsing.
        let events = Event::parse_stream(OPENING_PACKET).expect("fixture packet must parse");
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].code, "BTN_TOUCH");
        assert_eq!(events[3].event_type, EventType::Syn);
        assert!(events.iter().all(|e| e.time == 1787594305.903279));
    }

    #[test]
    fn blank_lines_are_skipped() {
        // Synthesized: blank-line handling. No real capture is empty; this exercises the filter.
        let events = Event::parse_stream("\n\n").expect("blank input is an empty stream");
        assert!(events.is_empty());
    }

    #[test]
    fn malformed_json_names_the_line() {
        // Synthesized: error-path line numbering. Real captures do not contain truly
        // malformed JSON; this exercises that errors report the correct line number.
        let err = Event::parse_stream(
            "{\"time\":1.0,\"type\":\"EV_KEY\",\"code\":\"X\",\"value\":0}\nnot json\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, EventError::Json { line: 2, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn unknown_event_type_is_rejected() {
        // Synthesized: serde rejection of unknown enum variants. Real captures use known types only.
        assert!(
            Event::parse_line(r#"{"time":1.0,"type":"EV_MADE_UP","code":"X","value":0}"#).is_err()
        );
    }
}
