use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use klik_core::control::{Reply, Request, Status};

/// What the loop shares with the clients: the status it keeps current, and
/// one sender per client that subscribed to the events.
#[derive(Clone)]
pub struct Shared {
    pub status: Arc<Mutex<Status>>,
    pub subscribers: Arc<Mutex<Vec<Sender<String>>>>,
}

impl Shared {
    pub fn new(status: Status) -> Self {
        Self {
            status: Arc::new(Mutex::new(status)),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
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
        Ok(Request::Status) => {
            let status = shared
                .status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            Reply::Status(status)
        }
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

    /// A client that went away is dropped at the next line, and the others
    /// keep receiving.
    #[test]
    fn a_dead_subscriber_is_dropped_and_the_others_keep_the_stream() {
        let shared = Shared::new(status());
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
        let shared = Shared::new(status());
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
        let error = serve(&path, Shared::new(status())).unwrap_err();
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
        serve(&path, Shared::new(status())).unwrap();
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
