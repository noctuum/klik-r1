use std::path::Path;
use std::sync::mpsc::channel;
use std::time::Instant;

use anyhow::Context as _;
use klik_core::config::{Config, Layout};
use klik_core::control::{DeviceStatus, ProfileChange, Reply, Status};
use klik_core::device::{Profile, ProfileKind};
use klik_core::gesture::Gesture;
use klik_core::module::{Desktop, Journal, Module, Press, PressEvent, Profiles, Services};
use klik_core::platform::{Grab, Pulse};
use klik_core::press::{PressKind, Recognized};
use klik_core::select::{Facts, Pins, ProfileActions, Selection, Why};
use klik_linux::injector::Uinput;
use klik_linux::process::Runner;
use klik_linux::source::Source;

use crate::control::{Command, Shared, serve};
use crate::journal::{Stderr, came_to_line, label, set_debug, switched_line};

/// The profiles this build knows, by the id a device section names.
pub const KINDS: [ProfileKind; 1] = [klik_beauty_r1::KIND];

pub struct Assembled {
    pub config: Config,
    pub ids: Vec<String>,
    pub profiles: Vec<Box<dyn Profile>>,
    pub modules: Vec<Box<dyn Module>>,
    pub layout: Layout,
    pub selection: Selection,
    pub keys: Vec<String>,
}

/// Everything that can be refused before a device is touched: the file, the
/// profiles of the devices, the modules and their sections, the bindings
/// against the buttons and the actions.
pub fn assemble(text: &str) -> anyhow::Result<Assembled> {
    let config = Config::from_toml(text)?;
    let mut ids = Vec::new();
    let mut profiles: Vec<Box<dyn Profile>> = Vec::new();
    for (id, section) in &config.devices {
        let kind = KINDS
            .iter()
            .find(|kind| kind.id == section.profile)
            .with_context(|| {
                format!(
                    "device {id}: no profile named {} in this build",
                    section.profile
                )
            })?;
        profiles.push((kind.build)(&section.settings).with_context(|| format!("device {id}"))?);
        ids.push(id.clone());
    }
    let mut modules: Vec<Box<dyn Module>> =
        vec![Box::new(klik_input::Input::new()), Box::new(ProfileActions)];
    #[cfg(feature = "kwin")]
    modules.push(Box::new(klik_kwin::Kwin));
    #[cfg(feature = "script")]
    modules.push(Box::new(klik_script::Script));
    for module in &mut modules {
        module.configure(config.modules.get(module.name()))?;
    }
    for name in config.modules.keys() {
        if !modules.iter().any(|module| module.name() == name) {
            anyhow::bail!("[modules.{name}] names a module that is not in this build");
        }
    }
    let actions: Vec<_> = modules
        .iter()
        .flat_map(|module| module.actions().iter().copied())
        .collect();
    let buttons = |device: &str| -> Option<&'static [klik_core::device::ButtonSpec]> {
        ids.iter()
            .position(|id| id == device)
            .map(|index| profiles[index].buttons())
    };
    let facts: Vec<&str> = modules
        .iter()
        .flat_map(|module| module.facts().iter().copied())
        .collect();
    let layout = Layout::new(&config, &buttons, &actions, &facts)?;
    let keys = layout.key_names(&actions);
    let selection = layout.selection();
    Ok(Assembled {
        config,
        ids,
        profiles,
        modules,
        layout,
        selection,
        keys,
    })
}

/// The status the daemon starts with: every configured device, none of them
/// connected yet. A device the daemon never sees stays as this leaves it.
pub fn initial_status(assembled: &Assembled) -> Status {
    let (profile, why) = assembled.selection.active(&Facts::new());
    Status {
        version: env!("CARGO_PKG_VERSION").to_string(),
        devices: assembled
            .ids
            .iter()
            .map(|id| DeviceStatus {
                id: id.clone(),
                profile: assembled.config.devices[id].profile.clone(),
                connected: false,
                grab: None,
            })
            .collect(),
        profile,
        why: why.to_string(),
    }
}

pub fn run(path: &Path) -> anyhow::Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let assembled = assemble(&text).with_context(|| format!("cannot use {}", path.display()))?;
    let status = initial_status(&assembled);
    let Assembled {
        config,
        ids,
        mut profiles,
        mut modules,
        layout,
        mut selection,
        keys,
    } = assembled;
    set_debug(config.debug.enabled);
    let mut journal = Stderr;
    // The socket comes first: a daemon that already answers on it holds the
    // remote, and a second one is refused before it declares a keyboard.
    let (commands, orders) = channel();
    let listed = config
        .order
        .iter()
        .map(|name| (name.clone(), config.profiles[name].extends.clone()))
        .collect();
    let shared = Shared::new(status, listed, commands);
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR is not set, so there is no place for the control socket")?;
    let socket = klik_core::control::socket_path(Path::new(&runtime));
    serve(&socket, shared.clone())
        .with_context(|| format!("cannot listen on {}", socket.display()))?;
    let mut injector = Uinput::new(&keys).context("cannot create the keyboard and the pointer")?;
    let poll_ms = profiles
        .iter()
        .map(|profile| profile.poll_ms())
        .min()
        .unwrap_or(100);
    let mut source =
        Source::new(profiles.len(), poll_ms, true).context("cannot watch the input devices")?;
    klik_linux::stop::install().context("cannot catch SIGTERM and SIGINT")?;
    let mut runner = Runner;
    #[cfg(feature = "kwin")]
    let mut desktop: Option<Box<dyn Desktop>> = match klik_kwin::KwinDesktop::new(Box::new(Runner))
    {
        Ok(desktop) => Some(Box::new(desktop)),
        Err(error) => {
            journal.line(&format!("kwin: off, {error}"));
            None
        }
    };
    #[cfg(not(feature = "kwin"))]
    let mut desktop: Option<Box<dyn Desktop>> = None;
    let mut current = selection.active(&Facts::new());
    let mut last_facts = Facts::new();
    let mut pins = Pins::default();
    let mut gestures: Vec<Gesture> = ids.iter().map(|_| Gesture::new()).collect();
    journal.line(&format!("waiting for {}", ids.join(", ")));

    loop {
        if let Some(signal) = klik_linux::stop::requested() {
            journal.line(&format!("stopping on {signal}"));
            let _ = std::fs::remove_file(&socket);
            return Ok(());
        }
        for pulse in source.next_batch(&profiles)? {
            let (device, recognized, now) = match pulse {
                Pulse::Connected {
                    device,
                    at,
                    axes,
                    grab,
                } => {
                    profiles[device].connected(at, &axes);
                    let grab = match grab {
                        Grab::Taken => "taken".to_string(),
                        Grab::Refused(why) => format!("refused: {why}"),
                        Grab::Skipped => "skipped".to_string(),
                    };
                    journal.line(&format!("{}: connected, the grab is {grab}", ids[device]));
                    shared.update(|status| {
                        status.devices[device].connected = true;
                        status.devices[device].grab = Some(grab.clone());
                    });
                    continue;
                }
                Pulse::Disconnected { device, error } => {
                    profiles[device].disconnected();
                    match error {
                        Some(error) => {
                            journal.line(&format!("{}: went away: {error}, waiting", ids[device]))
                        }
                        None => journal.line(&format!("{}: went away, waiting", ids[device])),
                    }
                    shared.update(|status| {
                        status.devices[device].connected = false;
                        status.devices[device].grab = None;
                    });
                    // A hold on this device will never see its end.
                    selection.release_device(&ids[device]);
                    switch_to(
                        &mut current,
                        selection.active(&last_facts),
                        &mut journal,
                        &shared,
                    );
                    continue;
                }
                Pulse::Event { device, event } => {
                    let at = event.time;
                    match profiles[device].feed(&event) {
                        Some(recognized) => (device, recognized, at),
                        None => continue,
                    }
                }
                Pulse::Idle(now) => {
                    let mut found = None;
                    for (device, profile) in profiles.iter_mut().enumerate() {
                        if let Some(recognized) = profile.tick(now) {
                            found = Some((device, recognized, now));
                            break;
                        }
                    }
                    match found {
                        Some(found) => found,
                        None => continue,
                    }
                }
            };
            let started = Instant::now();
            let nth = gestures[device].note(&recognized);
            let (button, event) = match &recognized {
                Recognized::Press(button, kind) => (*button, PressEvent::Start(*kind)),
                Recognized::Tick(button) => (*button, PressEvent::Tick { nth }),
                Recognized::End(button) => (*button, PressEvent::End),
                Recognized::Unmatched(what) => {
                    journal_line(
                        &mut journal,
                        &shared,
                        &format!("{}: {what}", label(now, &ids[device], &recognized)),
                    );
                    continue;
                }
                Recognized::Refused { after_ms, .. } => {
                    journal_line(
                        &mut journal,
                        &shared,
                        &format!(
                            "{}: refused {after_ms} ms after the last release",
                            label(now, &ids[device], &recognized)
                        ),
                    );
                    continue;
                }
            };
            let kind = match &recognized {
                Recognized::Press(_, kind) => *kind,
                _ => gestures[device].kind_of(button).unwrap_or(PressKind::Hold),
            };
            // The profile of a gesture is chosen at its press, with the facts
            // asked then, and its ticks and its end keep it.
            let profile = if matches!(event, PressEvent::Start(_)) {
                let needed = selection.needs_facts();
                if !needed.is_empty() {
                    let mut services = Services {
                        input: &mut injector,
                        journal: &mut journal,
                        desktop: desk(&mut desktop),
                        process: &mut runner,
                        profiles: &mut selection,
                    };
                    last_facts = gather(&needed, &mut modules, &mut services);
                }
                switch_to(
                    &mut current,
                    selection.active(&last_facts),
                    &mut journal,
                    &shared,
                );
                pins.pin(&ids[device], button, &current.0);
                current.0.clone()
            } else {
                pins.of(&ids[device], button)
                    .map_or_else(|| current.0.clone(), str::to_string)
            };
            let binding = layout.lookup(&profile, &ids[device], button, kind);
            if let Some(binding) = binding {
                let press = Press {
                    device: &ids[device],
                    button,
                    event,
                    at: now,
                    profile: &profile,
                };
                let module = modules
                    .iter_mut()
                    .find(|module| module.name() == binding.module);
                if let Some(module) = module {
                    let mut services = Services {
                        input: &mut injector,
                        journal: &mut journal,
                        desktop: desk(&mut desktop),
                        process: &mut runner,
                        profiles: &mut selection,
                    };
                    if let Err(fault) =
                        module.run(&binding.action, &binding.params, &press, &mut services)
                    {
                        journal.line(&format!(
                            "{}: {}.{} failed: {fault}",
                            label(now, &ids[device], &recognized),
                            binding.module,
                            binding.action
                        ));
                    }
                }
                if binding.module == "profile" {
                    switch_to(
                        &mut current,
                        selection.active(&last_facts),
                        &mut journal,
                        &shared,
                    );
                }
            }
            if event == PressEvent::End {
                pins.unpin(&ids[device], button);
            }
            if matches!(event, PressEvent::Start(_)) {
                journal_line(
                    &mut journal,
                    &shared,
                    &format!(
                        "{}: {}",
                        label(now, &ids[device], &recognized),
                        came_to_line(binding, &profile, started.elapsed())
                    ),
                );
            }
        }
        // What the control socket handed over since the last batch.
        while let Ok(command) = orders.try_recv() {
            match command {
                Command::Profile(change, answer) => {
                    let reply = match change {
                        ProfileChange::Set { name } if !selection.knows(&name) => Reply::Error {
                            message: format!("no profile named {name}"),
                        },
                        change => {
                            match change {
                                ProfileChange::Set { name } => selection.set(&name),
                                ProfileChange::Next => {
                                    let from = current.0.clone();
                                    selection.next_after(&from);
                                }
                                ProfileChange::Auto => selection.auto(),
                            }
                            switch_to(
                                &mut current,
                                selection.active(&last_facts),
                                &mut journal,
                                &shared,
                            );
                            Reply::Status(
                                shared
                                    .status
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .clone(),
                            )
                        }
                    };
                    let _ = answer.send(reply);
                }
                Command::Facts(answer) => {
                    let every: Vec<String> = modules
                        .iter()
                        .flat_map(|module| module.facts().iter().map(|fact| fact.to_string()))
                        .collect();
                    let mut services = Services {
                        input: &mut injector,
                        journal: &mut journal,
                        desktop: desk(&mut desktop),
                        process: &mut runner,
                        profiles: &mut selection,
                    };
                    let facts = gather(&every, &mut modules, &mut services);
                    let _ = answer.send(Reply::Facts { facts });
                }
            }
        }
    }
}

/// The desktop for one action. The trait object is narrowed to the borrow
/// here, since inside an `Option` the coercion would not happen and the
/// services would borrow everything for the life of the box.
fn desk(desktop: &mut Option<Box<dyn Desktop>>) -> Option<&mut dyn Desktop> {
    match desktop {
        Some(desktop) => Some(desktop.as_mut()),
        None => None,
    }
}

/// The facts among `needed`, each asked of the module that publishes it, all
/// of one module's in one question. A module that fails says so in the
/// journal, and its facts stay absent, so the rules fall through to the
/// default.
fn gather(needed: &[String], modules: &mut [Box<dyn Module>], services: &mut Services) -> Facts {
    let mut facts = Facts::new();
    for module in modules.iter_mut() {
        let wanted: Vec<String> = needed
            .iter()
            .filter(|fact| module.facts().contains(&fact.as_str()))
            .cloned()
            .collect();
        if wanted.is_empty() {
            continue;
        }
        match module.ask(&wanted, services) {
            Ok(pairs) => facts.extend(pairs),
            Err(fault) => services
                .journal
                .line(&format!("facts from {}: {fault}", module.name())),
        }
    }
    facts
}

/// Makes `now` the active profile, and says so in the journal and in the
/// status when the profile or the reason changed.
fn switch_to(
    current: &mut (String, Why),
    now: (String, Why),
    journal: &mut Stderr,
    shared: &Shared,
) {
    if *current == now {
        return;
    }
    journal.line(&switched_line(&now.0, &now.1));
    shared.update(|status| {
        status.profile = now.0.clone();
        status.why = now.1.to_string();
    });
    *current = now;
}

/// A line about one press: in the journal in the debug mode, and to every
/// client of `events` always.
fn journal_line(journal: &mut Stderr, shared: &Shared, line: &str) {
    journal.debug(line);
    shared.publish(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_configured_device_starts_as_not_connected_in_the_status() {
        let assembled = assemble(
            r#"
            [devices.beauty-r1]
            uniq = "aa"
            [devices.second]
            profile = "beauty-r1"
            uniq = "bb"
            [profiles.one.bindings]
            "beauty-r1.center:tap" = { action = "input.key", key = "KEY_1" }
            "#,
        )
        .unwrap();
        let status = initial_status(&assembled);
        assert_eq!(status.devices.len(), 2);
        assert!(
            status
                .devices
                .iter()
                .all(|device| !device.connected && device.grab.is_none())
        );
        assert_eq!(status.devices[1].id, "second");
        assert_eq!(status.devices[1].profile, "beauty-r1");
        assert_eq!(status.profile, "one");
    }

    /// A section a module refuses stops the daemon before a device is touched,
    /// and the error names the module.
    #[test]
    fn a_module_section_that_fails_its_check_is_refused() {
        let error = assemble(
            r#"
            [devices.beauty-r1]
            uniq = "aa"
            [profiles.one]
            [modules.input]
            hi_res_per_event = 0
            "#,
        )
        .err()
        .expect("refused");
        assert!(format!("{error:#}").contains("module input"), "{error:#}");
    }

    #[test]
    fn a_section_for_a_module_not_in_this_build_is_refused_by_name() {
        let error = assemble(
            r#"
            [devices.beauty-r1]
            uniq = "aa"
            [profiles.one]
            [modules.lua]
            "#,
        )
        .err()
        .expect("refused");
        assert!(error.to_string().contains("[modules.lua]"), "{error}");
    }

    const RULES: &str = r#"
        [devices.beauty-r1]
        uniq = "aa"
        [profiles.reading]
        [profiles.media]
        [[rules]]
        when = { window.class = "codium" }
        use = "reading"
        [[rules]]
        use = "media"
    "#;

    #[cfg(feature = "kwin")]
    #[test]
    fn rules_over_the_window_are_accepted_with_kwin() {
        let assembled = assemble(RULES).unwrap();
        assert_eq!(assembled.selection.needs_facts(), ["window.class"]);
    }

    #[cfg(not(feature = "kwin"))]
    #[test]
    fn without_kwin_a_rule_over_the_window_is_refused() {
        let error = format!("{:#}", assemble(RULES).err().expect("refused"));
        assert!(error.contains("window.class"), "{error}");
    }

    /// The example in `packaging` is what a new user copies, so it is held to
    /// what the daemon accepts, down to the names the keyboard declares.
    #[test]
    fn the_shipped_example_is_accepted() {
        let assembled = assemble(include_str!("../../packaging/klik.example.toml"))
            .expect("the example is accepted");
        klik_linux::keys::resolve(&assembled.keys).expect("every key of the example is a key");
    }
}
