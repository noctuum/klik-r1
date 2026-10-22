use std::process::Output;
use std::time::Duration;

use klik_core::error::ConfigError;
use klik_core::module::{
    ActionSpec, Fault, Module, ParamKind, ParamSpec, Press, PressEvent, Services,
};
use klik_core::platform::Run;
use klik_core::press::PressKind;

/// How long a command runs before it is killed, unless `limit_ms` says
/// otherwise.
pub const DEFAULT_LIMIT_MS: u64 = 10_000;

pub const ACTIONS: [ActionSpec; 1] = [ActionSpec {
    module: "script",
    name: "run",
    params: &[
        ParamSpec {
            name: "command",
            kind: ParamKind::Text,
            required: true,
        },
        ParamSpec {
            name: "wait",
            kind: ParamKind::Flag,
            required: false,
        },
        ParamSpec {
            name: "limit_ms",
            kind: ParamKind::Millis,
            required: false,
        },
    ],
    ..ActionSpec::PLAIN
}];

/// The command of a binding as a program to run: a shell, so that the user
/// writes a command line, with the press in its environment.
pub fn command_of(params: &toml::Table, press: &Press) -> Run {
    let command = params
        .get("command")
        .and_then(toml::Value::as_str)
        .unwrap_or_default();
    let limit_ms = params
        .get("limit_ms")
        .and_then(toml::Value::as_integer)
        .and_then(|ms| u64::try_from(ms).ok())
        .unwrap_or(DEFAULT_LIMIT_MS);
    // A tick and an end belong to a hold; only the start knows a tap.
    let kind = match press.event {
        PressEvent::Start(kind) => kind,
        _ => PressKind::Hold,
    };
    let env = [
        ("KLIK_DEVICE", press.device),
        ("KLIK_BUTTON", press.button),
        ("KLIK_KIND", kind.as_str()),
        ("KLIK_PROFILE", press.profile),
    ];
    Run {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), command.to_string()],
        env: env
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        limit: Duration::from_millis(limit_ms),
    }
}

/// What a command that was waited for printed, a line each, its output
/// before its errors.
pub fn printed_lines(out: &Output) -> Vec<String> {
    [&out.stdout, &out.stderr]
        .into_iter()
        .flat_map(|bytes| {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Commands bound to buttons.
#[derive(Debug, Default)]
pub struct Script;

impl Module for Script {
    fn name(&self) -> &'static str {
        "script"
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &ACTIONS
    }

    fn configure(&mut self, section: Option<&toml::Table>) -> Result<(), ConfigError> {
        match section {
            Some(section) if !section.is_empty() => Err(ConfigError::Module(
                "script".to_string(),
                "takes no settings".to_string(),
            )),
            _ => Ok(()),
        }
    }

    /// The command runs at the press. Detached, its end is watched on
    /// another thread, which writes a failure to standard error, the journal
    /// of the daemon. Waited for, what it printed goes to the journal a line
    /// each.
    fn run(
        &mut self,
        _action: &str,
        params: &toml::Table,
        press: &Press,
        services: &mut Services,
    ) -> Result<(), Fault> {
        if !matches!(press.event, PressEvent::Start(_)) {
            return Ok(());
        }
        let run = command_of(params, press);
        let command = run.args.last().cloned().unwrap_or_default();
        if params.get("wait").and_then(toml::Value::as_bool) == Some(true) {
            let out = services.process.output(&run)?;
            for line in printed_lines(&out) {
                services.journal.line(&format!("script {command}: {line}"));
            }
            if !out.status.success() {
                return Err(format!("the command exited with {}", out.status).into());
            }
            return Ok(());
        }
        services.process.spawn(
            &run,
            Box::new(move |line| eprintln!("script {command}: {line}")),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    fn press(kind: PressKind) -> Press<'static> {
        Press {
            device: "beauty-r1",
            button: "center",
            event: PressEvent::Start(kind),
            at: 0.0,
            profile: "reading",
        }
    }

    #[test]
    fn the_command_runs_in_a_shell_with_the_press_in_its_environment() {
        let run = command_of(
            &table("command = \"notify-send hi\""),
            &press(PressKind::Tap),
        );
        assert_eq!(run.program, "sh");
        assert_eq!(run.args, ["-c", "notify-send hi"]);
        for (key, value) in [
            ("KLIK_DEVICE", "beauty-r1"),
            ("KLIK_BUTTON", "center"),
            ("KLIK_KIND", "tap"),
            ("KLIK_PROFILE", "reading"),
        ] {
            assert!(
                run.env.contains(&(key.to_string(), value.to_string())),
                "{key}"
            );
        }
        assert_eq!(run.limit, Duration::from_millis(DEFAULT_LIMIT_MS));
    }

    #[test]
    fn the_limit_comes_from_limit_ms() {
        let run = command_of(
            &table("command = \"x\"\nlimit_ms = 250"),
            &press(PressKind::Tap),
        );
        assert_eq!(run.limit, Duration::from_millis(250));
    }

    /// A command that was waited for reaches the journal as a detached one
    /// does by itself: what it printed, then its errors.
    #[test]
    fn a_waited_command_is_journaled_a_line_each() {
        let out = Output {
            status: ExitStatus::from_raw(0),
            stdout: b"one\ntwo\n".to_vec(),
            stderr: b"oops\n".to_vec(),
        };
        assert_eq!(printed_lines(&out), ["one", "two", "oops"]);
    }

    #[test]
    fn a_held_press_says_hold() {
        let run = command_of(&table("command = \"x\""), &press(PressKind::Hold));
        assert!(
            run.env
                .contains(&("KLIK_KIND".to_string(), "hold".to_string()))
        );
    }
}
