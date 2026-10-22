pub mod dbus;
pub mod kdotool;
pub mod screen;

use std::io;
use std::time::Duration;

use klik_core::error::ConfigError;
use klik_core::module::{
    ActionSpec, Desktop, Fault, Module, ParamKind, ParamSpec, Press, PressEvent, Rect, Services,
    Step, Window, WindowMatch,
};
use klik_core::platform::Process;

use dbus::Bus;

/// How long a helper program gets. The loop stands still for the whole of a
/// call, and the remote with it. At the median on 2026-10-13
/// `kdotool getactivewindow` took 3.8 ms.
pub const HELPER_LIMIT: Duration = Duration::from_secs(2);

/// The facts this module is asked for at a press.
pub const FACTS: [&str; 3] = ["window.class", "window.title", "desktop.current"];

pub const ACTIONS: [ActionSpec; 3] = [
    ActionSpec {
        module: "kwin",
        name: "desktop.next",
        ..ActionSpec::PLAIN
    },
    ActionSpec {
        module: "kwin",
        name: "desktop.prev",
        ..ActionSpec::PLAIN
    },
    ActionSpec {
        module: "kwin",
        name: "window.activate",
        params: &[
            ParamSpec {
                name: "class",
                kind: ParamKind::Text,
                required: false,
            },
            ParamSpec {
                name: "title",
                kind: ParamKind::Text,
                required: false,
            },
        ],
        exactly_one_of: &["class", "title"],
        ..ActionSpec::PLAIN
    },
];

/// The window an action of `window.activate` means, from its parameters.
pub fn window_match(params: &toml::Table) -> Option<WindowMatch> {
    let text = |name: &str| {
        params
            .get(name)
            .and_then(toml::Value::as_str)
            .map(str::to_string)
    };
    text("class")
        .map(WindowMatch::Class)
        .or_else(|| text("title").map(WindowMatch::Title))
}

/// The window facts among `wanted`, out of the window in front, in the order
/// they were asked for. No window in front gives none.
pub fn window_facts(wanted: &[String], front: Option<&Window>) -> Vec<(String, String)> {
    let Some(window) = front else {
        return Vec::new();
    };
    wanted
        .iter()
        .filter_map(|fact| match fact.as_str() {
            "window.class" => Some((fact.clone(), window.class.clone())),
            "window.title" => Some((fact.clone(), window.title.clone())),
            _ => None,
        })
        .collect()
}

/// The desktop of KDE Plasma: KWin over the session bus, `kdotool` for the
/// window in front and for activation, `kscreen-doctor` for the screen.
pub struct KwinDesktop {
    bus: Bus,
    process: Box<dyn Process>,
    screen: Option<Rect>,
}

impl KwinDesktop {
    /// Connects to the session bus. A session without one is a desktop that
    /// is out of reach, and the daemon goes on without it.
    pub fn new(process: Box<dyn Process>) -> io::Result<Self> {
        Ok(Self {
            bus: Bus::new().map_err(io::Error::other)?,
            process,
            screen: None,
        })
    }
}

impl Desktop for KwinDesktop {
    fn front(&mut self) -> io::Result<Option<Window>> {
        let Some(id) = kdotool::active_window(self.process.as_mut())? else {
            return Ok(None);
        };
        let info = self.bus.window_info(&id).map_err(io::Error::other)?;
        Ok(dbus::window_from_info(&id, &info))
    }

    fn screen(&mut self) -> io::Result<Rect> {
        if let Some(rect) = self.screen {
            return Ok(rect);
        }
        let rect = screen::current(self.process.as_mut())?
            .ok_or_else(|| io::Error::other("kscreen-doctor named no enabled output"))?;
        self.screen = Some(rect);
        Ok(rect)
    }

    fn switch(&mut self, step: Step) -> io::Result<()> {
        match step {
            Step::Next => self.bus.next_desktop(),
            Step::Previous => self.bus.previous_desktop(),
        }
        .map_err(io::Error::other)
    }

    fn activate(&mut self, which: &WindowMatch) -> io::Result<()> {
        let ids = kdotool::search(self.process.as_mut(), which)?;
        let Some(first) = ids.first() else {
            return Err(io::Error::other(format!("no window matches {which:?}")));
        };
        kdotool::activate(self.process.as_mut(), first)
    }

    fn current_desktop(&mut self) -> io::Result<String> {
        let id = self.bus.current_desktop().map_err(io::Error::other)?;
        let desktops = self.bus.desktops().map_err(io::Error::other)?;
        dbus::desktop_name(&id, &desktops)
            .ok_or_else(|| io::Error::other(format!("no desktop has the id {id}")))
    }
}

/// The actions and the facts of the desktop, through the `desktop` service.
#[derive(Debug, Default)]
pub struct Kwin;

impl Module for Kwin {
    fn name(&self) -> &'static str {
        "kwin"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &ACTIONS
    }

    fn configure(&mut self, section: Option<&toml::Table>) -> Result<(), ConfigError> {
        match section {
            Some(section) if !section.is_empty() => Err(ConfigError::Module(
                "kwin".to_string(),
                "takes no settings".to_string(),
            )),
            _ => Ok(()),
        }
    }

    fn run(
        &mut self,
        action: &str,
        params: &toml::Table,
        press: &Press,
        services: &mut Services,
    ) -> Result<(), Fault> {
        if !matches!(press.event, PressEvent::Start(_)) {
            return Ok(());
        }
        let desktop = services
            .desktop
            .as_deref_mut()
            .ok_or("no desktop: KWin is out of reach")?;
        match action {
            "desktop.next" => desktop.switch(Step::Next)?,
            "desktop.prev" => desktop.switch(Step::Previous)?,
            "window.activate" => {
                let which =
                    window_match(params).ok_or("window.activate needs a class or a title")?;
                desktop.activate(&which)?;
            }
            other => return Err(format!("kwin has no action {other}").into()),
        }
        Ok(())
    }

    fn facts(&self) -> &'static [&'static str] {
        &FACTS
    }

    /// One look at the window in front for any window fact, and one question
    /// to KWin for the desktop. Without a desktop there is nothing to tell,
    /// which the daemon said at startup.
    fn ask(
        &mut self,
        wanted: &[String],
        services: &mut Services,
    ) -> Result<Vec<(String, String)>, Fault> {
        let Some(desktop) = services.desktop.as_deref_mut() else {
            return Ok(Vec::new());
        };
        let mut facts = Vec::new();
        if wanted.iter().any(|fact| fact.starts_with("window.")) {
            let front = desktop.front()?;
            facts.extend(window_facts(wanted, front.as_ref()));
        }
        if wanted.iter().any(|fact| fact == "desktop.current") {
            facts.push(("desktop.current".to_string(), desktop.current_desktop()?));
        }
        Ok(facts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn a_window_is_matched_by_its_class_or_its_title() {
        assert_eq!(
            window_match(&table("class = \"codium\"")),
            Some(WindowMatch::Class("codium".to_string()))
        );
        assert_eq!(
            window_match(&table("title = \"Notes\"")),
            Some(WindowMatch::Title("Notes".to_string()))
        );
        assert_eq!(window_match(&table("")), None);
    }

    #[test]
    fn only_the_facts_asked_for_are_given() {
        let window = Window {
            id: "w1".to_string(),
            class: "codium".to_string(),
            title: "t".to_string(),
            rect: Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        };
        assert_eq!(
            window_facts(&["window.class".to_string()], Some(&window)),
            [("window.class".to_string(), "codium".to_string())]
        );
        assert_eq!(
            window_facts(
                &["window.title".to_string(), "window.class".to_string()],
                Some(&window)
            ),
            [
                ("window.title".to_string(), "t".to_string()),
                ("window.class".to_string(), "codium".to_string())
            ]
        );
        assert!(window_facts(&["window.class".to_string()], None).is_empty());
    }
}
