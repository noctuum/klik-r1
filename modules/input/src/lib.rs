pub mod scroll;

use klik_core::error::ConfigError;
use klik_core::module::{
    ActionSpec, Fault, Module, ParamKind, ParamSpec, Press, PressEvent, Services,
};
use klik_core::press::PressKind;

use scroll::{Direction, ScrollSettings, Series, rate, units};

pub const ACTIONS: [ActionSpec; 3] = [
    ActionSpec {
        module: "input",
        name: "key",
        params: &[ParamSpec {
            name: "key",
            kind: ParamKind::Key,
            required: true,
        }],
        ..ActionSpec::PLAIN
    },
    ActionSpec {
        module: "input",
        name: "wheel",
        params: &[
            ParamSpec {
                name: "direction",
                kind: ParamKind::OneOf(&["up", "down"]),
                required: true,
            },
            ParamSpec {
                name: "aim",
                kind: ParamKind::Flag,
                required: false,
            },
        ],
        ..ActionSpec::PLAIN
    },
    ActionSpec {
        module: "input",
        name: "click",
        ..ActionSpec::PLAIN
    },
];

/// The keys of a chord in the order they go down; the last one is tapped
/// and the others are released in reverse. `KEY_LEFTCTRL+KEY_A` is
/// `Ctrl` down, `A` tapped, `Ctrl` up.
pub fn chord(text: &str) -> (Vec<&str>, &str) {
    let mut names: Vec<&str> = text.split('+').collect();
    let last = names.pop().unwrap_or_default();
    (names, last)
}

/// Keys and the wheel into the system.
pub struct Input {
    scroll: ScrollSettings,
    series: Series,
    /// Whether this series was told that no desktop aims the cursor, so the
    /// journal says it once per series and not per event.
    said_no_desktop: bool,
    /// Whether the journal said why an aim failed, since the last aim that
    /// did not fail. A desktop that keeps failing is said once.
    said_failed_aim: bool,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    pub fn new() -> Self {
        Self {
            scroll: ScrollSettings::default(),
            series: Series::new(),
            said_no_desktop: false,
            said_failed_aim: false,
        }
    }

    fn key(
        &mut self,
        params: &toml::Table,
        press: &Press,
        services: &mut Services,
    ) -> Result<(), Fault> {
        if press.event != PressEvent::Start(PressKind::Tap)
            && press.event != PressEvent::Start(PressKind::Hold)
        {
            return Ok(());
        }
        let text = params["key"].as_str().unwrap_or_default();
        let (modifiers, last) = chord(text);
        for name in &modifiers {
            services.input.press(name)?;
        }
        let tapped = services.input.tap(last);
        for name in modifiers.iter().rev() {
            services.input.release(name)?;
        }
        tapped?;
        services.journal.debug(&format!("{text} sent"));
        Ok(())
    }

    fn wheel(
        &mut self,
        params: &toml::Table,
        press: &Press,
        services: &mut Services,
    ) -> Result<(), Fault> {
        let nth = match press.event {
            PressEvent::Start(_) => 1,
            PressEvent::Tick { nth } => nth,
            PressEvent::End => return Ok(()),
        };
        let direction = match params["direction"].as_str() {
            Some("up") => Direction::Up,
            _ => Direction::Down,
        };
        let aim = params
            .get("aim")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        if self.series.needs_aim(press.at) {
            self.said_no_desktop = false;
            // A failed aim leaves the cursor where it is, and the wheel turns
            // all the same.
            if aim {
                match self.aim(services) {
                    Ok(()) => self.said_failed_aim = false,
                    Err(fault) if !self.said_failed_aim => {
                        self.said_failed_aim = true;
                        services
                            .journal
                            .line(&format!("the wheel goes where the cursor is: {fault}"));
                    }
                    Err(_) => {}
                }
            }
        }
        self.series.note(press.at);
        services
            .input
            .wheel(units(direction, rate(nth, &self.scroll)))?;
        Ok(())
    }

    /// The cursor at the aim point of the window in front, once per series.
    /// With no desktop the wheel goes where the cursor is, and the journal
    /// says so once per series.
    fn aim(&mut self, services: &mut Services) -> Result<(), Fault> {
        let Some(desktop) = services.desktop.as_deref_mut() else {
            if !self.said_no_desktop {
                self.said_no_desktop = true;
                services
                    .journal
                    .debug("no desktop module, the wheel goes where the cursor is");
            }
            return Ok(());
        };
        let Some(window) = desktop.front()? else {
            return Ok(());
        };
        let screen = desktop.screen()?;
        let (x, y) = aim_point(window.rect, screen, self.scroll.aim_x, self.scroll.aim_y);
        services.input.place(x, y)?;
        Ok(())
    }
}

/// A fraction of the window into the normalized range of the pointer.
pub fn aim_point(
    window: klik_core::module::Rect,
    screen: klik_core::module::Rect,
    aim_x: f64,
    aim_y: f64,
) -> (i32, i32) {
    let px = f64::from(window.x) + f64::from(window.width) * aim_x;
    let py = f64::from(window.y) + f64::from(window.height) * aim_y;
    let normalize = |value: f64, origin: i32, extent: i32| -> i32 {
        let extent = f64::from(extent.max(1));
        let fraction = ((value - f64::from(origin)) / extent).clamp(0.0, 1.0);
        (fraction * 65535.0).round() as i32
    };
    (
        normalize(px, screen.x, screen.width),
        normalize(py, screen.y, screen.height),
    )
}

impl Module for Input {
    fn name(&self) -> &'static str {
        "input"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &ACTIONS
    }

    /// No section is the defaults. The settings are checked before they are
    /// kept, so a refused section leaves the module as it was.
    fn configure(&mut self, section: Option<&toml::Table>) -> Result<(), ConfigError> {
        let refused = |reason| ConfigError::Module("input".to_string(), reason);
        let scroll = match section {
            Some(section) => ScrollSettings::from_table(section).map_err(refused)?,
            None => ScrollSettings::default(),
        };
        scroll.check().map_err(refused)?;
        self.scroll = scroll;
        Ok(())
    }

    fn run(
        &mut self,
        action: &str,
        params: &toml::Table,
        press: &Press,
        services: &mut Services,
    ) -> Result<(), Fault> {
        match action {
            "key" => self.key(params, press, services),
            "wheel" => self.wheel(params, press, services),
            "click" => {
                if matches!(press.event, PressEvent::Start(_)) {
                    services.input.click()?;
                }
                Ok(())
            }
            other => Err(format!("input has no action {other}").into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klik_core::module::{Desktop, Journal, Rect, Step, Window, WindowMatch};
    use klik_core::platform::{Injector, Process, Run};
    use klik_core::select::Selection;
    use std::io;

    /// Records what an action asked the injector for. The one allowed fake:
    /// the real injector is a uinput device, and a test sends nothing.
    #[derive(Default)]
    struct Recorder {
        calls: Vec<String>,
    }

    impl Injector for Recorder {
        fn tap(&mut self, key: &str) -> io::Result<()> {
            self.calls.push(format!("tap {key}"));
            Ok(())
        }
        fn press(&mut self, key: &str) -> io::Result<()> {
            self.calls.push(format!("press {key}"));
            Ok(())
        }
        fn release(&mut self, key: &str) -> io::Result<()> {
            self.calls.push(format!("release {key}"));
            Ok(())
        }
        fn wheel(&mut self, units: i32) -> io::Result<()> {
            self.calls.push(format!("wheel {units}"));
            Ok(())
        }
        fn place(&mut self, x: i32, y: i32) -> io::Result<()> {
            self.calls.push(format!("place {x} {y}"));
            Ok(())
        }
        fn click(&mut self) -> io::Result<()> {
            self.calls.push("click".to_string());
            Ok(())
        }
    }

    #[derive(Default)]
    struct Lines(Vec<String>);

    impl Journal for Lines {
        fn line(&mut self, text: &str) {
            self.0.push(text.to_string());
        }
        fn debug(&mut self, text: &str) {
            self.0.push(text.to_string());
        }
    }

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    /// A placeholder for the process service, which no action of `input`
    /// calls: every method panics.
    struct Unused;

    impl Process for Unused {
        fn output(&mut self, _: &Run) -> io::Result<std::process::Output> {
            unreachable!("input runs no program")
        }
        fn spawn(&mut self, _: &Run, _: Box<dyn FnOnce(String) + Send>) -> io::Result<()> {
            unreachable!("input runs no program")
        }
    }

    fn press(event: PressEvent, at: f64) -> Press<'static> {
        Press {
            device: "beauty-r1",
            button: "up",
            event,
            at,
            profile: "one",
        }
    }

    /// A desktop every call of which fails, as in a session without KWin or
    /// without `kdotool`. It answers nothing but the failure.
    struct Broken;

    fn broken() -> io::Error {
        io::Error::other("kdotool could not be started")
    }

    impl Desktop for Broken {
        fn front(&mut self) -> io::Result<Option<Window>> {
            Err(broken())
        }
        fn screen(&mut self) -> io::Result<Rect> {
            Err(broken())
        }
        fn switch(&mut self, _: Step) -> io::Result<()> {
            Err(broken())
        }
        fn activate(&mut self, _: &WindowMatch) -> io::Result<()> {
            Err(broken())
        }
        fn current_desktop(&mut self) -> io::Result<String> {
            Err(broken())
        }
    }

    fn run(
        input: &mut Input,
        action: &str,
        params: &str,
        event: PressEvent,
        at: f64,
    ) -> (Vec<String>, Vec<String>) {
        run_on(None, input, action, params, event, at)
    }

    fn run_on(
        desktop: Option<&mut Broken>,
        input: &mut Input,
        action: &str,
        params: &str,
        event: PressEvent,
        at: f64,
    ) -> (Vec<String>, Vec<String>) {
        let mut recorder = Recorder::default();
        let mut lines = Lines::default();
        let mut selection = Selection::new(vec!["one".to_string()], vec![]);
        let mut services = Services {
            input: &mut recorder,
            journal: &mut lines,
            desktop: desktop.map(|desktop| desktop as &mut dyn Desktop),
            process: &mut Unused,
            profiles: &mut selection,
        };
        input
            .run(action, &table(params), &press(event, at), &mut services)
            .unwrap();
        (recorder.calls, lines.0)
    }

    #[test]
    fn a_chord_puts_the_modifiers_down_taps_the_last_and_lifts_them_in_reverse() {
        let mut input = Input::new();
        let (calls, _) = run(
            &mut input,
            "key",
            r#"key = "KEY_LEFTCTRL+KEY_LEFTSHIFT+KEY_A""#,
            PressEvent::Start(PressKind::Tap),
            1.0,
        );
        assert_eq!(
            calls,
            [
                "press KEY_LEFTCTRL",
                "press KEY_LEFTSHIFT",
                "tap KEY_A",
                "release KEY_LEFTSHIFT",
                "release KEY_LEFTCTRL"
            ]
        );
    }

    #[test]
    fn a_key_goes_on_the_start_and_not_on_a_tick_or_an_end() {
        let mut input = Input::new();
        let (calls, _) = run(
            &mut input,
            "key",
            r#"key = "KEY_1""#,
            PressEvent::Tick { nth: 2 },
            1.0,
        );
        assert!(calls.is_empty());
        let (calls, _) = run(&mut input, "key", r#"key = "KEY_1""#, PressEvent::End, 1.0);
        assert!(calls.is_empty());
    }

    /// The tick comes 100 ms after the start, inside the series; the down
    /// tap a second later starts another one.
    #[test]
    fn the_wheel_rises_along_the_gesture_and_says_once_that_no_desktop_aims() {
        let mut input = Input::new();
        let (first, lines) = run(
            &mut input,
            "wheel",
            r#"direction = "up""#,
            PressEvent::Start(PressKind::Hold),
            1.0,
        );
        assert_eq!(first, ["wheel 40"]);
        assert_eq!(
            lines,
            ["no desktop module, the wheel goes where the cursor is"]
        );
        let (eleventh, lines) = run(
            &mut input,
            "wheel",
            r#"direction = "up""#,
            PressEvent::Tick { nth: 11 },
            1.1,
        );
        assert_eq!(eleventh, ["wheel 80"]);
        assert!(lines.is_empty(), "said once per series");
        let (down, _) = run(
            &mut input,
            "wheel",
            r#"direction = "down""#,
            PressEvent::Start(PressKind::Tap),
            2.0,
        );
        assert_eq!(down, ["wheel -40"]);
    }

    /// A desktop that cannot name the window in front leaves the cursor
    /// where it is, and the wheel turns all the same. The journal says why
    /// once, and again only after an aim that did not fail.
    #[test]
    fn a_failed_aim_turns_the_wheel_where_the_cursor_is_and_says_why_once() {
        let mut input = Input::new();
        let up = r#"direction = "up""#;
        let said = "the wheel goes where the cursor is: kdotool could not be started";
        let start = PressEvent::Start(PressKind::Tap);
        let (calls, lines) = run_on(Some(&mut Broken), &mut input, "wheel", up, start, 1.0);
        assert_eq!(calls, ["wheel 40"]);
        assert_eq!(lines, [said]);
        let tick = PressEvent::Tick { nth: 2 };
        let (calls, lines) = run_on(Some(&mut Broken), &mut input, "wheel", up, tick, 1.03);
        assert_eq!(calls, ["wheel 40"]);
        assert!(lines.is_empty(), "no second aim inside the series");
        let (calls, lines) = run_on(Some(&mut Broken), &mut input, "wheel", up, start, 3.0);
        assert_eq!(calls, ["wheel 40"]);
        assert!(lines.is_empty(), "the next series fails in silence");
        let (_, lines) = run_on(None, &mut input, "wheel", up, start, 5.0);
        assert_eq!(
            lines,
            ["no desktop module, the wheel goes where the cursor is"]
        );
        let (_, lines) = run_on(Some(&mut Broken), &mut input, "wheel", up, start, 7.0);
        assert_eq!(lines, [said], "said again after an aim that did not fail");
    }

    #[test]
    fn the_end_of_a_hold_moves_no_wheel() {
        let mut input = Input::new();
        let (calls, _) = run(
            &mut input,
            "wheel",
            r#"direction = "up""#,
            PressEvent::End,
            1.0,
        );
        assert!(calls.is_empty());
    }

    #[test]
    fn a_click_goes_on_the_start_only() {
        let mut input = Input::new();
        assert_eq!(
            run(
                &mut input,
                "click",
                "",
                PressEvent::Start(PressKind::Tap),
                1.0
            )
            .0,
            ["click"]
        );
        assert!(
            run(&mut input, "click", "", PressEvent::Tick { nth: 2 }, 1.0)
                .0
                .is_empty()
        );
    }

    #[test]
    fn the_aim_point_is_a_fraction_of_the_window_in_the_range_of_the_pointer() {
        let screen = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let window = Rect {
            x: 0,
            y: 32,
            width: 1920,
            height: 1048,
        };
        let (x, y) = aim_point(window, screen, 0.5, 0.0);
        assert_eq!(x, 32768);
        assert_eq!(y, (32.0 / 1080.0 * 65535.0_f64).round() as i32);
        let (x, _) = aim_point(window, screen, 0.992, 0.3);
        assert_eq!(x, (0.992 * 65535.0_f64).round() as i32);
    }

    #[test]
    fn a_section_of_the_wrong_shape_names_the_module() {
        let mut input = Input::new();
        let error = input
            .configure(Some(&table("hi_res_per_event = 0")))
            .unwrap_err();
        assert!(matches!(error, ConfigError::Module(ref module, _) if module == "input"));
        assert!(input.configure(None).is_ok(), "no section is the defaults");
    }
}
