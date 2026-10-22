use std::sync::atomic::{AtomicI32, Ordering};

use nix::libc::{self, c_int};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

/// The signal that asked the daemon to stop, as its number, or 0.
static SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn note(signal: c_int) {
    SIGNAL.store(signal, Ordering::SeqCst);
    // SAFETY: `signal` is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        libc::signal(libc::SIGINT, libc::SIG_DFL);
    }
}

/// Catches `SIGTERM` and `SIGINT`, so that `systemctl --user stop` and Ctrl-C
/// let the daemon clean up before it exits. Without `SA_RESTART`: a poll in
/// progress then returns `EINTR`, and the loop reads the flag.
///
/// Only the first signal is caught. The handler puts both dispositions back
/// to the default, so a second signal ends the daemon whatever it is doing:
/// the loop reads the flag only between its calls, and a call that hangs
/// keeps it from reading.
pub fn install() -> nix::Result<()> {
    let action = SigAction::new(SigHandler::Handler(note), SaFlags::empty(), SigSet::empty());
    for signal in [Signal::SIGTERM, Signal::SIGINT] {
        // SAFETY: `note` stores to an atomic and calls `signal`, and both are
        // async-signal-safe.
        unsafe { sigaction(signal, &action)? };
    }
    Ok(())
}

/// The signal that asked the daemon to stop, by name, or nothing yet.
pub fn requested() -> Option<&'static str> {
    match SIGNAL.load(Ordering::SeqCst) {
        0 => None,
        n if n == Signal::SIGTERM as c_int => Some("SIGTERM"),
        n if n == Signal::SIGINT as c_int => Some("SIGINT"),
        _ => Some("a signal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::raise;

    /// Whether `signal` has the default disposition, read without changing it.
    fn kills(signal: Signal) -> bool {
        let mut action = std::mem::MaybeUninit::<libc::sigaction>::uninit();
        // SAFETY: a null new action changes nothing, and the call writes the
        // whole of the old one.
        let action = unsafe {
            assert_eq!(
                libc::sigaction(signal as c_int, std::ptr::null(), action.as_mut_ptr()),
                0
            );
            action.assume_init()
        };
        action.sa_sigaction == libc::SIG_DFL
    }

    /// One test for both signals: the flag is a static of the test process and
    /// the dispositions belong to it, so a second test could see neither
    /// clear. `raise` delivers to the calling thread, so the handler runs here
    /// and the other tests see nothing of it. The handlers are installed again
    /// before the second `raise`, which would end the test process otherwise.
    #[test]
    fn the_first_signal_raises_the_flag_and_the_second_would_kill() {
        install().expect("the handlers install");
        assert_eq!(requested(), None);
        assert!(!kills(Signal::SIGTERM) && !kills(Signal::SIGINT));
        raise(Signal::SIGTERM).expect("raised");
        assert_eq!(requested(), Some("SIGTERM"));
        assert!(kills(Signal::SIGTERM), "a second SIGTERM is caught again");
        assert!(kills(Signal::SIGINT), "SIGINT after SIGTERM is caught");
        SIGNAL.store(0, Ordering::SeqCst);

        install().expect("the handlers install again");
        assert!(!kills(Signal::SIGTERM) && !kills(Signal::SIGINT));
        raise(Signal::SIGINT).expect("raised");
        assert_eq!(requested(), Some("SIGINT"));
        assert!(kills(Signal::SIGINT), "a second SIGINT is caught again");
        assert!(kills(Signal::SIGTERM), "SIGTERM after SIGINT is caught");
        SIGNAL.store(0, Ordering::SeqCst);
    }
}
