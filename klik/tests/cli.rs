use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use klik_core::raw::{Event, EventType};

const CLEAN_RUN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../devices/beauty-r1/tests/fixtures/beauty-r1-clean-run.jsonl"
);

#[test]
fn replay_prints_the_six_buttons_of_the_clean_run() {
    let output = Command::new(env!("CARGO_BIN_EXE_klik"))
        .arg("replay")
        .arg(CLEAN_RUN)
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("output is utf8");
    let buttons: Vec<&str> = stdout.lines().filter(|l| l.starts_with("press ")).collect();
    assert_eq!(
        buttons,
        vec![
            "press up tap",
            "press down tap",
            "press left tap",
            "press right tap",
            "press center tap",
            "press camera tap",
        ]
    );
}

/// A press the quiet window refuses is printed as well, with the time since
/// the release before it. No recording on file holds a motionless press inside
/// the 45 ms of the window, the quickest repeat on record is 59 ms after a
/// release, so the input is written here: a Center press, the same press again
/// 30 ms after its release, which is the spacing of a bounce, and a third one
/// 100 ms after the first release, outside the window.
#[test]
fn replay_prints_the_presses_the_quiet_window_refuses() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("center-bounce.jsonl");
    let presses = [
        center_press(10.0, 10.03),
        center_press(10.06, 10.09),
        center_press(10.13, 10.16),
    ];
    let text: String = presses
        .concat()
        .iter()
        .map(|event| serde_json::to_string(event).expect("the event serializes") + "\n")
        .collect();
    std::fs::write(&path, text).expect("the capture is written");

    let output = Command::new(env!("CARGO_BIN_EXE_klik"))
        .arg("replay")
        .arg(&path)
        .output()
        .expect("the binary runs");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("output is utf8");
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![
            "press center tap",
            "end center",
            "refused center after 30 ms",
            "press center tap",
            "end center",
        ]
    );
}

/// One motionless Center press, as the recognizer tests build it: the touch,
/// the position of the Center signature, the report, the release.
fn center_press(open_at: f64, release_at: f64) -> Vec<Event> {
    let event = |time, event_type, code: &str, value| Event {
        time,
        event_type,
        code: code.to_string(),
        value,
    };
    vec![
        event(open_at, EventType::Key, "BTN_TOUCH", 1),
        event(open_at, EventType::Abs, "ABS_X", 1904),
        event(open_at, EventType::Abs, "ABS_Y", 1904),
        event(open_at, EventType::Syn, "SYN_REPORT", 0),
        event(release_at, EventType::Key, "BTN_TOUCH", 0),
    ]
}

#[test]
fn replay_fails_loudly_on_a_missing_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_klik"))
        .arg("replay")
        .arg("no/such/file")
        .output()
        .expect("runs");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no/such/file"));
}

/// Runs `klik daemon` on this configuration and returns what it printed when
/// it refused to start.
///
/// The configuration and the runtime directory are this test's own, under the
/// target directory, so a daemon that got past the refusal would not take the
/// control socket of a daemon the user runs. It is given five seconds and then
/// killed: past the refusal it would create a keyboard and a pointer and wait
/// for the remote, and the suite would hang with both on the desktop.
fn refused(name: &str, config: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).expect("the directory is made");
    let path = dir.join("klik.toml");
    std::fs::write(&path, config).expect("the configuration is written");

    let mut child = Command::new(env!("CARGO_BIN_EXE_klik"))
        .args(["daemon", "--config"])
        .arg(&path)
        .env("XDG_RUNTIME_DIR", &dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().expect("the child can be polled").is_none() {
        if Instant::now() > deadline {
            child.kill().ok();
            panic!("the daemon did not refuse within five seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("the output is read");
    assert!(!output.status.success(), "it must refuse to start");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The name passes the configuration, which knows the shape of a key name and
/// not the kernel's table, and is refused where the keyboard is declared,
/// before the keyboard exists.
#[test]
fn a_configuration_with_a_key_nobody_knows_is_refused_by_name() {
    let stderr = refused(
        "key-nobody-knows",
        r#"
        [devices.beauty-r1]
        uniq = "aa"
        [profiles.one.bindings]
        "beauty-r1.center:tap" = { action = "input.key", key = "KEY_NOPE" }
        "#,
    );
    assert!(stderr.contains("KEY_NOPE"), "{stderr}");
}

#[test]
fn a_configuration_with_two_profiles_names_the_rule_it_lacks() {
    let stderr = refused(
        "two-profiles",
        "[devices.beauty-r1]\nuniq = \"aa\"\n[profiles.a]\n[profiles.b]\n",
    );
    assert!(stderr.contains("[[rules]]"), "{stderr}");
}
