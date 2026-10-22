use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use klik_core::control::{ProfileChange, ProfileEntry, Reply, Request, Status};

/// How long a client waits for the loop to answer a request only the loop
/// can answer.
pub const ANSWER_LIMIT: Duration = Duration::from_secs(2);

/// A request for the loop, with the way back to the client.
pub enum Command {
    Profile(ProfileChange, Sender<Reply>),
    Facts(Sender<Reply>),
}

/// What the loop shares with the clients: the status it keeps current, one
/// sender per client that subscribed to the events, the way to hand the loop
/// a command, and the profiles of the configuration in the order of the file
/// with the one each extends.
#[derive(Clone)]
pub struct Shared {
    pub status: Arc<Mutex<Status>>,
    pub subscribers: Arc<Mutex<Vec<Sender<String>>>>,
    pub commands: Sender<Command>,
    pub profiles: Arc<Vec<(String, Option<String>)>>,
    pub answer_limit: Duration,
}

impl Shared {
    pub fn new(
        status: Status,
        profiles: Vec<(String, Option<String>)>,
        commands: Sender<Command>,
    ) -> Self {
        Self {
            status: Arc::new(Mutex::new(status)),
            subscribers: Arc::new(Mutex::new(Vec::new())),
            commands,
            profiles: Arc::new(profiles),
            answer_limit: ANSWER_LIMIT,
        }
    }

    fn status_now(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// One line to every subscriber. A subscriber whose client went away is
    /// dropped here, which is what keeps the loop running after a client
    /// closed in the middle of the stream.
    pub fn publish(&self, line: &str) {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        subscribers.retain(|sender| sender.send(line.to_string()).is_ok());
    }

    pub fn update(&self, change: impl FnOnce(&mut Status)) {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        change(&mut status);
    }
}

/// Binds the socket and serves each client on a thread of its own.
///
/// A socket a daemon answers on is refused: removing its file would leave that
/// daemon unreachable while it goes on holding the remote. A file nobody
/// answers on is the stale one of a daemon that did not clean up, and goes.
pub fn serve(path: &Path, shared: Shared) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if UnixStream::connect(path).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "another daemon answers on this socket",
        ));
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let shared = shared.clone();
            thread::spawn(move || serve_one(client, &shared));
        }
    });
    Ok(())
}

fn serve_one(stream: UnixStream, shared: &Shared) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    });
    let mut writer = stream;
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Request>(line.trim()) {
        Ok(Request::Status) => Reply::Status(shared.status_now()),
        Ok(Request::Profiles) => {
            let status = shared.status_now();
            Reply::Profiles {
                profiles: shared
                    .profiles
                    .iter()
                    .map(|(name, extends)| ProfileEntry {
                        name: name.clone(),
                        extends: extends.clone(),
                        active: *name == status.profile,
                    })
                    .collect(),
                why: status.why,
            }
        }
        Ok(Request::Profile { change }) => {
            to_loop(shared, |answer| Command::Profile(change, answer))
        }
        Ok(Request::Facts) => to_loop(shared, Command::Facts),
        Ok(Request::Events) => {
            let (sender, receiver) = channel();
            shared
                .subscribers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(sender);
            stream_events(&mut writer, receiver);
            return;
        }
        Err(error) => Reply::Error {
            message: error.to_string(),
        },
    };
    let _ = write_reply(&mut writer, &reply);
}

/// Hands the loop a command and waits for its answer, for the answer limit
/// at most: the loop may be inside a helper program, and a client is not
/// held for longer than one is allowed to take.
fn to_loop(shared: &Shared, command: impl FnOnce(Sender<Reply>) -> Command) -> Reply {
    let (answer, reply) = channel();
    if shared.commands.send(command(answer)).is_err() {
        return Reply::Error {
            message: "the daemon is stopping".to_string(),
        };
    }
    reply
        .recv_timeout(shared.answer_limit)
        .unwrap_or_else(|_| Reply::Error {
            message: format!(
                "the daemon did not answer within {} ms",
                shared.answer_limit.as_millis()
            ),
        })
}

fn stream_events(writer: &mut UnixStream, receiver: Receiver<String>) {
    while let Ok(line) = receiver.recv() {
        if write_reply(writer, &Reply::Event { line }).is_err() {
            return;
        }
    }
}

pub fn write_reply(writer: &mut impl Write, reply: &Reply) -> std::io::Result<()> {
    let mut text = serde_json::to_string(reply)?;
    text.push('\n');
    writer.write_all(text.as_bytes())?;
    writer.flush()
}

/// The client side: one request, and the replies as they come.
pub fn ask(
    path: &Path,
    request: &Request,
    mut each: impl FnMut(Reply) -> bool,
) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    let mut text = serde_json::to_string(request)?;
    text.push('\n');
    stream.write_all(text.as_bytes())?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let reply: Reply = serde_json::from_str(&line?)?;
        if !each(reply) {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use klik_core::control::DeviceStatus;

    fn status() -> Status {
        Status {
            version: "0.1.0".to_string(),
            devices: vec![DeviceStatus {
                id: "beauty-r1".to_string(),
                profile: "beauty-r1".to_string(),
                connected: false,
                grab: None,
            }],
            profile: "reading".to_string(),
            why: "the one profile".to_string(),
        }
    }

    /// What the loop would share, and the receiving end of its commands.
    fn sharing() -> (Shared, Receiver<Command>) {
        let (commands, receiver) = channel();
        let profiles = vec![
            ("reading".to_string(), None),
            ("media".to_string(), Some("base".to_string())),
        ];
        (Shared::new(status(), profiles, commands), receiver)
    }

    fn asked(path: &Path, request: &Request) -> Option<Reply> {
        let mut got = None;
        ask(path, request, |reply| {
            got = Some(reply);
            false
        })
        .unwrap();
        got
    }

    #[test]
    fn a_profile_change_reaches_the_loop_and_its_answer_comes_back() {
        let dir = std::env::temp_dir().join(format!("klik-control-change-{}", std::process::id()));
        let path = klik_core::control::socket_path(&dir);
        let (shared, commands) = sharing();
        serve(&path, shared).unwrap();
        let the_loop = thread::spawn(move || {
            match commands
                .recv_timeout(Duration::from_secs(5))
                .expect("the command reached the loop")
            {
                Command::Profile(ProfileChange::Next, answer) => {
                    answer.send(Reply::Status(status())).unwrap()
                }
                _ => panic!("another command reached the loop"),
            }
        });
        let got = asked(
            &path,
            &Request::Profile {
                change: ProfileChange::Next,
            },
        );
        the_loop.join().unwrap();
        assert_eq!(got, Some(Reply::Status(status())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A loop stuck on a helper must not hold the client forever.
    #[test]
    fn a_loop_that_does_not_answer_is_reported() {
        let dir = std::env::temp_dir().join(format!("klik-control-mute-{}", std::process::id()));
        let path = klik_core::control::socket_path(&dir);
        let (mut shared, _held) = sharing();
        shared.answer_limit = Duration::from_millis(50);
        serve(&path, shared).unwrap();
        match asked(&path, &Request::Facts) {
            Some(Reply::Error { message }) => {
                assert!(message.contains("did not answer within 50 ms"), "{message}")
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_profiles_are_answered_from_what_the_loop_shares() {
        let dir =
            std::env::temp_dir().join(format!("klik-control-profiles-{}", std::process::id()));
        let path = klik_core::control::socket_path(&dir);
        serve(&path, sharing().0).unwrap();
        assert_eq!(
            asked(&path, &Request::Profiles),
            Some(Reply::Profiles {
                profiles: vec![
                    ProfileEntry {
                        name: "reading".to_string(),
                        extends: None,
                        active: true
                    },
                    ProfileEntry {
                        name: "media".to_string(),
                        extends: Some("base".to_string()),
                        active: false
                    },
                ],
                why: "the one profile".to_string(),
            })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A client that went away is dropped at the next line, and the others
    /// keep receiving.
    #[test]
    fn a_dead_subscriber_is_dropped_and_the_others_keep_the_stream() {
        let shared = sharing().0;
        let (alive, receiver) = channel();
        let (dead, gone) = channel();
        drop(gone);
        shared.subscribers.lock().unwrap().push(dead);
        shared.subscribers.lock().unwrap().push(alive);
        shared.publish("up tap");
        assert_eq!(receiver.recv().unwrap(), "up tap");
        assert_eq!(shared.subscribers.lock().unwrap().len(), 1);
    }

    /// The whole round trip over a real socket in a temporary directory:
    /// status, then events with one line published and the client closing.
    /// The system's temporary directory, since Cargo gives a directory of
    /// its own under `target/` to an integration test and not to a unit test.
    #[test]
    fn status_and_events_round_trip_over_the_socket() {
        let dir = std::env::temp_dir().join(format!("klik-control-test-{}", std::process::id()));
        let path = klik_core::control::socket_path(&dir);
        let shared = sharing().0;
        serve(&path, shared.clone()).unwrap();
        let mut got = None;
        ask(&path, &Request::Status, |reply| {
            got = Some(reply);
            false
        })
        .unwrap();
        assert_eq!(got, Some(Reply::Status(status())));

        let publisher = shared.clone();
        let handle = std::thread::spawn(move || {
            while publisher.subscribers.lock().unwrap().is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            publisher.publish("center tap: came to input.key, 2 ms");
        });
        let mut lines = Vec::new();
        ask(&path, &Request::Events, |reply| {
            lines.push(reply);
            false
        })
        .unwrap();
        handle.join().unwrap();
        assert_eq!(
            lines,
            vec![Reply::Event {
                line: "center tap: came to input.key, 2 ms".to_string()
            }]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A daemon that answers on the socket keeps it. Removing the file would
    /// leave that daemon unreachable while it goes on holding the remote.
    #[test]
    fn a_socket_a_daemon_answers_on_is_refused() {
        let dir = std::env::temp_dir().join(format!("klik-control-live-{}", std::process::id()));
        let path = klik_core::control::socket_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _first = UnixListener::bind(&path).unwrap();
        let error = serve(&path, sharing().0).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        assert!(
            UnixStream::connect(&path).is_ok(),
            "the file still leads to the first daemon"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file nobody answers on is what a daemon that did not clean up leaves
    /// behind, and the next daemon takes its place.
    #[test]
    fn a_socket_file_nobody_answers_on_is_replaced() {
        let dir = std::env::temp_dir().join(format!("klik-control-stale-{}", std::process::id()));
        let path = klik_core::control::socket_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        drop(UnixListener::bind(&path).unwrap());
        serve(&path, sharing().0).unwrap();
        let mut got = None;
        ask(&path, &Request::Status, |reply| {
            got = Some(reply);
            false
        })
        .unwrap();
        assert_eq!(got, Some(Reply::Status(status())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
