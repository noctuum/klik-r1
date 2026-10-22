use std::io::{self, Read};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use klik_core::platform::{Process, Run};

/// How often a program run for its output is asked whether it has ended,
/// over its first `QUICK`: a helper returns within a few milliseconds.
const TICK: Duration = Duration::from_micros(200);

/// How long a program run for its output is asked at the pace of `TICK`;
/// after that it is asked at the pace of `WATCH`.
const QUICK: Duration = Duration::from_millis(50);

/// How often the watcher of a detached program asks whether it has ended.
const WATCH: Duration = Duration::from_millis(10);

/// How long the streams of a program that has ended get to reach their end.
/// What it printed is in the pipes already, and a program it left behind in
/// the background can hold them open for as long as that one runs.
const GRACE: Duration = Duration::from_millis(20);

/// What a reader thread has read of one stream of the child so far, and how
/// the stream ended once it has.
#[derive(Default)]
struct Taken {
    bytes: Vec<u8>,
    end: Option<io::Result<()>>,
}

type Stream = Arc<Mutex<Taken>>;

/// The `Process` of Linux: `std::process` with a reader thread per stream,
/// so that a program printing more than a pipe holds does not stand still.
#[derive(Debug, Default)]
pub struct Runner;

fn command(run: &Run) -> Command {
    let mut command = Command::new(&run.program);
    command
        .args(&run.args)
        .envs(run.env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null());
    command
}

/// Starts the program and names it when it cannot start: the bare error says
/// `No such file or directory` and not of what.
fn started(run: &Run, command: &mut Command) -> io::Result<Child> {
    command.spawn().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("{} could not be started: {error}", run.program),
        )
    })
}

impl Process for Runner {
    fn output(&mut self, run: &Run) -> io::Result<Output> {
        let mut child = started(
            run,
            command(run).stdout(Stdio::piped()).stderr(Stdio::piped()),
        )?;
        let deadline = Instant::now() + run.limit;
        let stdout = drained(child.stdout.take());
        let stderr = drained(child.stderr.take());
        match ended(&mut child, &stdout, &stderr, deadline) {
            Ok(Some(output)) => Ok(output),
            Ok(None) => {
                discard(child);
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{} did not return within {} ms and was killed",
                        run.program,
                        run.limit.as_millis()
                    ),
                ))
            }
            Err(error) => {
                discard(child);
                Err(error)
            }
        }
    }

    /// The output of the program goes where the daemon's goes, which for the
    /// service is the journal.
    fn spawn(&mut self, run: &Run, report: Box<dyn FnOnce(String) + Send>) -> io::Result<()> {
        let mut child = started(run, &mut command(run))?;
        let program = run.program.clone();
        let limit = run.limit;
        let deadline = Instant::now() + limit;
        thread::spawn(move || {
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if let Some(line) = failure(&program, status) {
                            report(line);
                        }
                        return;
                    }
                    Ok(None) if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        report(format!(
                            "{program} did not end within {} ms and was killed",
                            limit.as_millis()
                        ));
                        return;
                    }
                    Ok(None) => thread::sleep(WATCH),
                    Err(error) => {
                        report(format!("{program} could not be waited for: {error}"));
                        return;
                    }
                }
            }
        });
        Ok(())
    }
}

/// What a program that ended badly is reported as, and nothing for one that
/// succeeded.
fn failure(program: &str, status: ExitStatus) -> Option<String> {
    if status.success() {
        return None;
    }
    Some(match (status.code(), status.signal()) {
        (Some(code), _) => format!("{program} exited with {code}"),
        (None, Some(signal)) => format!("{program} ended on signal {signal}"),
        (None, None) => format!("{program} ended with {status}"),
    })
}

/// Reads a stream of the child on a thread of its own and keeps what it has
/// read so far. A pipe holds 64 KiB, and a program that prints more stands
/// still until it is read, so the reading cannot wait for the program to
/// end. A program left behind in the background keeps the thread until it
/// ends.
fn drained<R: Read + Send + 'static>(stream: Option<R>) -> Stream {
    let shared = Stream::default();
    let taken = Arc::clone(&shared);
    thread::spawn(move || {
        let end = match stream {
            Some(mut stream) => read_into(&mut stream, &taken),
            None => Ok(()),
        };
        lock(&taken).end = Some(end);
    });
    shared
}

fn read_into(stream: &mut impl Read, taken: &Mutex<Taken>) -> io::Result<()> {
    let mut buffer = [0; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => lock(taken).bytes.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn lock(taken: &Mutex<Taken>) -> MutexGuard<'_, Taken> {
    taken.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a stream gave so far, or the error that ended it.
fn taken(stream: &Stream) -> io::Result<Vec<u8>> {
    let mut taken = lock(stream);
    match taken.end.take() {
        Some(Err(error)) => Err(error),
        _ => Ok(std::mem::take(&mut taken.bytes)),
    }
}

/// What the child printed and how it ended, or `None` at the deadline.
///
/// The end of the streams and the end of the program are two events. One
/// that exits closes them a moment before it can be waited for; one that
/// closed them itself may go on; and one that left a program behind in the
/// background, as `sh -c "a &"` does, has ended while that program holds
/// them open. So the program is asked until the deadline, and once it has
/// ended its streams get `GRACE` to reach their end.
fn ended(
    child: &mut Child,
    stdout: &Stream,
    stderr: &Stream,
    deadline: Instant,
) -> io::Result<Option<Output>> {
    let begun = Instant::now();
    let mut exited: Option<(ExitStatus, Instant)> = None;
    loop {
        if exited.is_none() {
            exited = child.try_wait()?.map(|status| (status, Instant::now()));
        }
        if let Some((status, at)) = exited {
            let closed = lock(stdout).end.is_some() && lock(stderr).end.is_some();
            if closed || at.elapsed() >= GRACE {
                return Ok(Some(Output {
                    status,
                    stdout: taken(stdout)?,
                    stderr: taken(stderr)?,
                }));
            }
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(if begun.elapsed() < QUICK { TICK } else { WATCH });
    }
}

/// Kills the child and waits for it on a thread of its own: a program that
/// did not end in time is not given the loop to die in.
fn discard(mut child: Child) {
    let _ = child.kill();
    thread::spawn(move || {
        let _ = child.wait();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn run(program: &str, args: &[&str], limit_ms: u64) -> Run {
        Run {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: vec![],
            limit: Duration::from_millis(limit_ms),
        }
    }

    #[test]
    fn output_gives_what_the_program_printed() {
        let out = Runner.output(&run("printf", &["hi"], 2000)).unwrap();
        assert_eq!(out.stdout, b"hi");
    }

    #[test]
    fn output_passes_the_environment() {
        let mut r = run("sh", &["-c", "printf %s \"$KLIK_TEST\""], 2000);
        r.env = vec![("KLIK_TEST".to_string(), "ok".to_string())];
        assert_eq!(Runner.output(&r).unwrap().stdout, b"ok");
    }

    #[test]
    fn output_kills_a_program_past_its_limit() {
        let started = Instant::now();
        let error = Runner.output(&run("sleep", &["5"], 100)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("100 ms"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// `sh -c "sleep 2 & printf hi"` ends at once, and the `sleep` it left
    /// behind holds both streams open for two seconds more. The command has
    /// ended, and what it printed is kept.
    #[test]
    fn output_returns_when_the_program_ends_though_its_child_keeps_the_streams() {
        let started = Instant::now();
        let out = Runner
            .output(&run("sh", &["-c", "sleep 2 & printf hi"], 1000))
            .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hi");
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    /// Without the name the journal says `No such file or directory` and
    /// not of what.
    #[test]
    fn output_names_a_program_that_cannot_start() {
        let error = Runner
            .output(&run("klik-test-no-such-program", &[], 2000))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            error.to_string().contains("klik-test-no-such-program"),
            "{error}"
        );
    }

    #[test]
    fn spawn_names_a_program_that_cannot_start() {
        let error = Runner
            .spawn(
                &run("klik-test-no-such-program", &[], 2000),
                Box::new(|_| {}),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            error.to_string().contains("klik-test-no-such-program"),
            "{error}"
        );
    }

    fn reported(r: Run) -> Result<String, mpsc::RecvTimeoutError> {
        let (tx, rx) = mpsc::channel();
        Runner
            .spawn(
                &r,
                Box::new(move |line| {
                    let _ = tx.send(line);
                }),
            )
            .unwrap();
        rx.recv_timeout(Duration::from_secs(3))
    }

    #[test]
    fn spawn_reports_a_failure() {
        let line = reported(run("sh", &["-c", "exit 3"], 2000)).unwrap();
        assert!(line.contains("exited with 3"), "{line}");
    }

    #[test]
    fn spawn_reports_a_program_killed_at_its_limit() {
        let line = reported(run("sleep", &["5"], 100)).unwrap();
        assert!(line.contains("killed"), "{line}");
    }

    /// The watcher drops `report` without calling it, so the channel
    /// disconnects rather than timing out.
    #[test]
    fn spawn_says_nothing_of_a_success() {
        assert_eq!(
            reported(run("true", &[], 2000)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn spawn_returns_before_the_program_ends() {
        let started = Instant::now();
        Runner
            .spawn(&run("sleep", &["1"], 5000), Box::new(|_| {}))
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
