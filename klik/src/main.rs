mod control;
mod daemon;
mod journal;

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use klik_core::control::{ProfileChange, ProfileEntry, Reply, Request, Status};
use klik_core::device::Profile;
use klik_core::platform::{Grab, Pulse};
use klik_core::press::Recognized;
use klik_core::raw::{Event, EventType};
use klik_linux::source::Source;

#[derive(Parser)]
#[command(name = "klik", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read the devices of the configuration and run their bindings until stopped.
    Daemon {
        /// Defaults to `$XDG_CONFIG_HOME/klik/klik.toml`.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Feed a recorded capture through the Beauty-R1 profile and print what it resolves.
    Replay { file: PathBuf },
    /// Record the raw events of one device into a file, for a profile and its tests.
    Record {
        /// Where to write. Defaults to standard output.
        #[arg(long)]
        output: Option<PathBuf>,
        /// The Bluetooth address of the device.
        #[arg(long)]
        uniq: String,
        /// Leave the device visible to the desktop while recording.
        #[arg(long)]
        no_grab: bool,
    },
    /// Ask the running daemon for its state.
    Status,
    /// Print every press the running daemon handles, until stopped.
    Events,
    /// Switch the profile of the running daemon by hand, or hand it back to the rules.
    Profile {
        #[command(subcommand)]
        change: ProfileArg,
    },
    /// List the profiles, the active one marked, and why it is active.
    Profiles,
    /// Print what the modules tell of the world now, the facts the rules read.
    Facts {
        /// Seconds to wait first, to bring a window to the front.
        #[arg(long, default_value_t = 0)]
        wait: u64,
    },
    /// Print the udev rule for the devices of the configuration and how to install it.
    InstallUdev {
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ProfileArg {
    /// Keep this profile active until the next switch.
    Set { name: String },
    /// Keep the next profile of the configuration active.
    Next,
    /// Let the rules choose again.
    Auto,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Profile { change } => ask(&Request::Profile {
            change: match change {
                ProfileArg::Set { name } => ProfileChange::Set { name },
                ProfileArg::Next => ProfileChange::Next,
                ProfileArg::Auto => ProfileChange::Auto,
            },
        }),
        Command::Profiles => ask(&Request::Profiles),
        Command::Facts { wait } => {
            std::thread::sleep(Duration::from_secs(wait));
            ask(&Request::Facts)
        }
        Command::Daemon { config } => daemon::run(&config.unwrap_or_else(default_config_path)),
        Command::Replay { file } => replay(&file),
        Command::Record {
            output,
            uniq,
            no_grab,
        } => record(output, &uniq, no_grab),
        Command::Status => ask(&Request::Status),
        Command::Events => ask(&Request::Events),
        Command::InstallUdev { config } => {
            install_udev(&config.unwrap_or_else(default_config_path))
        }
    }
}

/// `$XDG_CONFIG_HOME/klik/klik.toml`, and `~/.config/klik/klik.toml` without it.
/// Built rather than written as a literal: clap does not expand `~`, so a
/// literal default would reach the filesystem as a directory named `~`.
pub fn default_config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_default()
        .join("klik")
        .join("klik.toml")
}

/// The profile of a Beauty-R1 at this address, with the measured defaults.
fn beauty(uniq: &str) -> anyhow::Result<klik_beauty_r1::Beauty> {
    let table =
        toml::Table::from_iter([("uniq".to_string(), toml::Value::String(uniq.to_string()))]);
    Ok(klik_beauty_r1::Beauty::new(
        klik_beauty_r1::Settings::from_table(&table)?,
    ))
}

fn replay(path: &Path) -> anyhow::Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let events =
        Event::parse_stream(&text).with_context(|| format!("cannot parse {}", path.display()))?;
    let mut beauty = beauty("replay")?;
    let started_at = events.first().map(|e| e.time).unwrap_or_default();
    beauty.connected(started_at - 1.0, &Vec::new());
    for event in &events {
        match beauty.feed(event) {
            Some(Recognized::Press(button, kind)) => println!("press {button} {}", kind.as_str()),
            Some(Recognized::End(button)) => println!("end {button}"),
            Some(Recognized::Unmatched(what)) => println!("unmatched {what}"),
            Some(Recognized::Refused { button, after_ms }) => {
                println!("refused {button} after {after_ms} ms")
            }
            Some(Recognized::Tick(_)) | None => {}
        }
    }
    Ok(())
}

/// One event as one JSON Lines record.
///
/// Flushed by the caller per line rather than buffered: a recording is stopped
/// with Ctrl-C, and a buffer means the last second of it is never written. That
/// is usually the part worth keeping.
///
/// The `serde_json` branch is mapped but untested, because no `Event` can reach
/// it: the fields are an `f64`, a fieldless enum, a `String` and an `i32`, and
/// serde refuses none of those. The one candidate is a non-finite `f64`, and
/// `serde_json` writes that as `null` rather than failing. Such a line would be
/// written and then refused on the way back in, since `null` is not an `f64`.
/// Nothing produces one, because the clock of a capture is finite for every
/// `SystemTime`.
pub fn write_event(out: &mut impl Write, event: &Event) -> io::Result<()> {
    let line = serde_json::to_string(event).map_err(io::Error::other)?;
    writeln!(out, "{line}")
}

/// The position the recording starts from, as events that say so.
///
/// The kernel reports an absolute axis only when it changes, so a recording that
/// merely starts listening does not contain the pointer's position, and a
/// motionless first press in it is unidentifiable. `EVIOCGABS` knows the
/// position at any moment, and writing it as an ordinary opening packet keeps
/// the capture format unchanged: a replay sees exactly what the daemon saw.
///
/// An axis with no known value is omitted. Writing a zero for it would be
/// writing a coordinate, and a coordinate near the origin can match a signature.
pub fn seed_packet(time: f64, x: Option<i32>, y: Option<i32>) -> Vec<Event> {
    let mut packet = Vec::new();
    let mut axis = |code: &str, value: i32| {
        packet.push(Event {
            time,
            event_type: EventType::Abs,
            code: code.to_string(),
            value,
        });
    };
    if let Some(value) = x {
        axis("ABS_X", value);
    }
    if let Some(value) = y {
        axis("ABS_Y", value);
    }
    if !packet.is_empty() {
        packet.push(Event {
            time,
            event_type: EventType::Syn,
            code: "SYN_REPORT".to_string(),
            value: 0,
        });
    }
    packet
}

fn record(output: Option<PathBuf>, uniq: &str, no_grab: bool) -> anyhow::Result<()> {
    let mut out: Box<dyn Write> = match output {
        Some(path) => Box::new(
            std::fs::File::create(&path)
                .with_context(|| format!("cannot create {}", path.display()))?,
        ),
        None => Box::new(io::stdout()),
    };

    // Recording grabs by default. Without it a held Left or Right changes the
    // system volume while the capture runs, and what gets captured is a desktop
    // reacting rather than a remote sending.
    let profiles: Vec<Box<dyn Profile>> = vec![Box::new(beauty(uniq)?)];
    let mut source = Source::new(1, profiles[0].poll_ms(), !no_grab)
        .with_context(|| format!("cannot watch for {uniq}"))?;
    eprintln!("waiting for {uniq}, press Ctrl-C to stop");

    // Whether a connection has already been seeded. Tracked here rather than
    // asked of `Source`, which reports connections and has no opinion about
    // what a recorder does with them.
    let mut seeded_before = false;

    loop {
        for pulse in source.next_batch(&profiles)? {
            match pulse {
                // `at` is the instant `Source` read this position, which is the
                // instant the seed describes. Sampling the clock again here
                // would date the packet from when it was written rather than
                // from when it was taken, and `replay` reports nothing for an
                // event earlier than the first line of the file, although an
                // `ABS_X` or `ABS_Y` event among them still moves the
                // remembered position.
                Pulse::Connected { at, axes, grab, .. } => {
                    match grab {
                        Grab::Taken => eprintln!("holding the remote"),
                        Grab::Refused(error) => {
                            eprintln!("warning: cannot hold the remote: {error}")
                        }
                        Grab::Skipped => eprintln!("not holding the remote"),
                    }
                    // Every connection gets a seed, reconnects included: drop it
                    // and the first motionless press after a reconnect has no
                    // position and cannot be identified, which is the whole
                    // reason the seed exists.
                    //
                    // The cost is that such a capture no longer replays to what
                    // the daemon saw. The daemon resets the recognizer on a
                    // connection; the format has no way to say the device went
                    // away, so `replay` cannot, and a disconnect mid-gesture
                    // leaves a live direction tracker that reads the seed's axes
                    // as one more delta, resolving a press the hardware never
                    // produced. Said out loud here, while the operator can still
                    // discard the file, rather than discovered later by someone
                    // trusting it as a regression case.
                    let (x, y) = klik_beauty_r1::position(&axes);
                    let seed = seed_packet(at, x, y);
                    if seeded_before && !seed.is_empty() {
                        eprintln!(
                            "warning: the remote reconnected; this file can now replay a press the hardware never sent, so record it again if it is meant to be a fixture"
                        );
                    }
                    seeded_before |= !seed.is_empty();
                    for event in seed {
                        write_event(&mut out, &event)?;
                    }
                    out.flush()?;
                }
                Pulse::Event { event, .. } => {
                    write_event(&mut out, &event)?;
                    out.flush()?;
                }
                Pulse::Disconnected {
                    error: Some(error), ..
                } => eprintln!("the remote went away: {error}, waiting"),
                Pulse::Disconnected { error: None, .. } => {
                    eprintln!("the remote went away, waiting")
                }
                Pulse::Idle(_) => {}
            }
        }
    }
}

/// What `klik status` prints: the daemon and its profile, then one line per
/// configured device, connected or not.
pub fn status_lines(status: &Status) -> Vec<String> {
    let mut lines = vec![format!(
        "klik {} on profile {} ({})",
        status.version, status.profile, status.why
    )];
    for device in &status.devices {
        let state = match (device.connected, &device.grab) {
            (true, Some(grab)) => format!("connected, the grab is {grab}"),
            (true, None) => "connected".to_string(),
            (false, _) => "not connected".to_string(),
        };
        lines.push(format!("  {} ({}): {state}", device.id, device.profile));
    }
    lines
}

/// What `klik profiles` prints: the profiles in the order of the file, the
/// active one marked with `*`, then why it is active.
pub fn profiles_lines(profiles: &[ProfileEntry], why: &str) -> Vec<String> {
    let mut lines: Vec<String> = profiles
        .iter()
        .map(|profile| {
            let mark = if profile.active { '*' } else { ' ' };
            match &profile.extends {
                Some(parent) => format!("{mark} {} (extends {parent})", profile.name),
                None => format!("{mark} {}", profile.name),
            }
        })
        .collect();
    lines.push(format!("why: {why}"));
    lines
}

/// What `klik facts` prints: one fact a line.
pub fn facts_lines(facts: &BTreeMap<String, String>) -> Vec<String> {
    if facts.is_empty() {
        return vec!["no facts".to_string()];
    }
    facts
        .iter()
        .map(|(fact, value)| format!("{fact} = {value}"))
        .collect()
}

fn ask(request: &Request) -> anyhow::Result<()> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    let socket = klik_core::control::socket_path(Path::new(&runtime));
    control::ask(&socket, request, |reply| match reply {
        Reply::Status(status) => {
            for line in status_lines(&status) {
                println!("{line}");
            }
            false
        }
        Reply::Event { line } => {
            println!("{line}");
            true
        }
        Reply::Error { message } => {
            eprintln!("the daemon refused the request: {message}");
            false
        }
        Reply::Profiles { profiles, why } => {
            for line in profiles_lines(&profiles, &why) {
                println!("{line}");
            }
            false
        }
        Reply::Facts { facts } => {
            for line in facts_lines(&facts) {
                println!("{line}");
            }
            false
        }
    })
    .with_context(|| {
        format!(
            "cannot reach the daemon at {}; is it running?",
            socket.display()
        )
    })
}

fn install_udev(config: &Path) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(config)
        .with_context(|| format!("cannot read {}", config.display()))?;
    let parsed = klik_core::config::Config::from_toml(&text)?;
    let mut rules = String::new();
    for (id, section) in &parsed.devices {
        let Some(uniq) = section.settings.get("uniq").and_then(toml::Value::as_str) else {
            anyhow::bail!("device {id} has no uniq, and the rule matches on it");
        };
        rules.push_str(&udev_rule(uniq));
    }
    println!("{rules}");
    println!("# Install it with:");
    println!(
        "#   printf '%s' '{}' | sudo tee /etc/udev/rules.d/99-klik.rules",
        rules.trim_end().replace('\'', "'\\''")
    );
    println!("#   sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=input");
    Ok(())
}

/// The rule that keeps libinput off the device whether or not the daemon
/// holds it, matching on the address so that a second remote of the same
/// model is not caught.
pub fn udev_rule(uniq: &str) -> String {
    format!(
        "SUBSYSTEM==\"input\", ATTRS{{uniq}}==\"{}\", ENV{{LIBINPUT_IGNORE_DEVICE}}=\"1\"\n",
        uniq.to_ascii_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use klik_core::control::DeviceStatus;

    fn abs(time: f64, code: &str, value: i32) -> Event {
        Event {
            time,
            event_type: EventType::Abs,
            code: code.to_string(),
            value,
        }
    }

    fn line(event: &Event) -> String {
        let mut out = Vec::new();
        write_event(&mut out, event).expect("writes");
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn an_event_is_one_line_the_parser_accepts() {
        let text = line(&abs(1.5, "ABS_Y", 2036));
        assert!(text.ends_with('\n'), "each event is its own line");
        assert_eq!(
            Event::parse_stream(&text).expect("round trips"),
            vec![abs(1.5, "ABS_Y", 2036)]
        );
    }

    /// A `Write` that fails, so the error path of `write_event` is exercised
    /// rather than assumed. A full disk and a closed pipe both arrive here.
    struct FailingWrite;

    impl Write for FailingWrite {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failing_sink_surfaces_its_error_rather_than_being_swallowed() {
        let error = write_event(&mut FailingWrite, &abs(1.5, "ABS_Y", 2036))
            .expect_err("a broken pipe must reach the caller");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    /// A recording that does not say where the pointer started is not a faithful
    /// replay of what the daemon saw: the daemon reads that position with
    /// EVIOCGABS, and a motionless first press reports no coordinates of its own.
    #[test]
    fn the_seed_packet_states_the_starting_position() {
        let packet = seed_packet(100.0, Some(1904), Some(1904));
        assert_eq!(
            Event::parse_stream(&packet.iter().map(line).collect::<String>()).unwrap(),
            packet
        );
        assert_eq!(packet.len(), 3, "both axes and the report that closes them");
        assert_eq!(packet[0].code, "ABS_X");
        assert_eq!(packet[1].code, "ABS_Y");
        assert_eq!(packet[2].code, "SYN_REPORT");
        assert!(packet.iter().all(|e| e.time == 100.0));
    }

    /// An axis the kernel never reported is left out rather than written as a
    /// zero, which would be a coordinate and could match a signature.
    #[test]
    fn an_unknown_axis_is_omitted_from_the_seed() {
        let packet = seed_packet(100.0, None, Some(1904));
        assert_eq!(packet.len(), 2);
        assert_eq!(packet[0].code, "ABS_Y");
    }

    /// The mirror of the case above. Both single-axis branches are separate
    /// `if let`s, so covering one says nothing about the other.
    #[test]
    fn a_seed_with_only_x_known_omits_the_other_axis() {
        let packet = seed_packet(100.0, Some(1904), None);
        assert_eq!(packet.len(), 2);
        assert_eq!(packet[0].code, "ABS_X");
        assert_eq!(packet[0].value, 1904);
        assert_eq!(packet[1].code, "SYN_REPORT");
    }

    #[test]
    fn a_wholly_unknown_position_produces_no_seed_at_all() {
        assert!(seed_packet(100.0, None, None).is_empty());
    }

    #[test]
    fn the_udev_rule_matches_the_address_in_lower_case() {
        assert_eq!(
            udev_rule("AA:BB:CC:DD:EE:01"),
            "SUBSYSTEM==\"input\", ATTRS{uniq}==\"aa:bb:cc:dd:ee:01\", ENV{LIBINPUT_IGNORE_DEVICE}=\"1\"\n"
        );
    }

    /// Two devices configured and one of them absent: both are listed, and the
    /// absent one says so.
    #[test]
    fn the_status_names_every_device_and_says_which_is_not_connected() {
        let status = Status {
            version: "0.1.0".to_string(),
            devices: vec![
                DeviceStatus {
                    id: "beauty-r1".to_string(),
                    profile: "beauty-r1".to_string(),
                    connected: true,
                    grab: Some("taken".to_string()),
                },
                DeviceStatus {
                    id: "second".to_string(),
                    profile: "beauty-r1".to_string(),
                    connected: false,
                    grab: None,
                },
            ],
            profile: "reading".to_string(),
            why: "the one profile of the configuration".to_string(),
        };
        assert_eq!(
            status_lines(&status),
            [
                "klik 0.1.0 on profile reading (the one profile of the configuration)",
                "  beauty-r1 (beauty-r1): connected, the grab is taken",
                "  second (beauty-r1): not connected",
            ]
        );
    }

    #[test]
    fn the_profiles_mark_the_active_one_and_say_why() {
        let profiles = [
            ProfileEntry {
                name: "reading".to_string(),
                extends: None,
                active: true,
            },
            ProfileEntry {
                name: "media".to_string(),
                extends: Some("base".to_string()),
                active: false,
            },
        ];
        assert_eq!(
            profiles_lines(&profiles, "set by hand"),
            ["* reading", "  media (extends base)", "why: set by hand"]
        );
    }

    #[test]
    fn the_facts_are_one_per_line_or_none() {
        let facts = BTreeMap::from([("window.class".to_string(), "codium".to_string())]);
        assert_eq!(facts_lines(&facts), ["window.class = codium"]);
        assert_eq!(facts_lines(&BTreeMap::new()), ["no facts"]);
    }
}
