use std::io;
use std::os::fd::AsFd;
use std::time::{Duration, Instant, SystemTime};

use klik_core::device::Profile;
use klik_core::platform::{Devices, Grab, Pulse};
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::capture::seconds;
use crate::hotplug::Hotplug;
use crate::remote::Remote;

/// The shortest the loop will ever sleep, in milliseconds.
pub const MIN_TICK_MS: u32 = 10;

/// How long one `poll` may wait: the period the profiles asked for.
///
/// The period is the profile's to choose; the Beauty-R1 asks for half its hold
/// threshold, see its `poll_ms`. The floor exists so a configuration of zero
/// cannot turn the loop into a spin.
pub fn timeout_for(poll_ms: u32) -> PollTimeout {
    let ms = poll_ms.max(MIN_TICK_MS);
    PollTimeout::try_from(ms).unwrap_or(PollTimeout::MAX)
}

/// How often the backstop rescan in `next_batch` may run, in milliseconds.
///
/// The udev-triggered rescan already covers the ordinary case. This exists for
/// when no such signal ever arrives at all: a missed or overflowed udev event,
/// or a read failure that turns out not to be a disconnect. A human notices a
/// dead remote in longer than a second, so scanning `/dev/input` at that rate
/// costs nothing worth avoiding.
const RESCAN_EVERY_MS: u64 = 1000;

/// Whether enough time has passed since the last scan to run another one.
///
/// `None` means never scanned, and is always due.
///
/// Monotonic rather than the wall clock the rest of the loop carries. This is
/// internal scheduling, compared against nothing outside the process, and
/// `CLOCK_REALTIME` moves in *both* directions: NTP steps it and resume from
/// suspend steps it. A backward step of N seconds while the remote is absent
/// would make the difference negative and suppress the backstop for N seconds.
/// The backstop exists precisely for the case where no udev event ever
/// arrives, so the remote would stay undiscovered. The recognizer's own
/// windows must stay on the wall clock, because they are compared against evdev
/// event timestamps, which are `CLOCK_REALTIME`; this one is compared against
/// itself.
///
/// `saturating_duration_since` rather than `-` or `duration_since`: on this
/// toolchain all three saturate to zero when the arguments arrive out of order,
/// but only this one is documented to, and the other two reserve the right to
/// go back to panicking. A daemon that panics stops answering the remote.
fn due_for_rescan(last: Option<Instant>, now: Instant) -> bool {
    match last {
        None => true,
        Some(last) => now.saturating_duration_since(last) >= Duration::from_millis(RESCAN_EVERY_MS),
    }
}

/// Whether slot `index` waits for a device: it exists and holds none.
fn wants<T>(slots: &[Option<T>], index: usize) -> bool {
    slots.get(index).is_some_and(Option::is_none)
}

/// The devices, the monitor and the clock, behind one call.
#[derive(Debug)]
pub struct Source {
    /// One slot per profile, in the order of the profiles the daemon holds.
    remotes: Vec<Option<Remote>>,
    hotplug: Hotplug,
    timeout: PollTimeout,
    grab: bool,
    /// When the evdev scan last ran, for `due_for_rescan`. `None` until the
    /// first one, which is also how `next_batch` knows to run it. Monotonic,
    /// and deliberately not the clock the pulses carry; see `due_for_rescan`.
    last_scan: Option<Instant>,
}

impl Source {
    /// Opens the monitor and leaves the first scan to the first `next_batch`,
    /// which has the profiles to scan for. The order is on purpose: a remote
    /// that connects between the two is caught by the monitor, whereas the
    /// other order drops it and waits forever for an event already past.
    ///
    /// `grab` decides whether each connection takes exclusive access. It lives
    /// here rather than in the caller because a reconnect has to re-take the
    /// grab. It is tied to the descriptor, and the descriptor is new.
    pub fn new(count: usize, poll_ms: u32, grab: bool) -> io::Result<Self> {
        let hotplug = Hotplug::new()?;
        Ok(Self {
            remotes: (0..count).map(|_| None).collect(),
            hotplug,
            timeout: timeout_for(poll_ms),
            grab,
            last_scan: None,
        })
    }

    /// Waits for whichever comes first: events from a remote, a change in the
    /// input subsystem, or the timeout.
    pub fn next_batch(&mut self, profiles: &[Box<dyn Profile>]) -> io::Result<Vec<Pulse>> {
        let mut pulses = Vec::new();

        if self.last_scan.is_none() {
            self.rescan(profiles, Instant::now(), &mut pulses)?;
        }

        // A remote found before the first poll, or one that reappeared. Opening
        // it is reported here rather than in the scan, so the caller has one
        // path for the first connection and every later one.
        for (device, slot) in self.remotes.iter_mut().enumerate() {
            let Some(remote) = slot.as_mut() else {
                continue;
            };
            if remote.announced {
                continue;
            }
            // Set before the fallible call below on purpose. The other order
            // livelocks, retrying `axes` forever on a device that never stops
            // failing it. A failure here drops the remote outright instead of
            // leaving it half-announced; the backstop rescan picks it up again
            // once it is ready.
            remote.announced = true;
            let axes = match remote.axes() {
                Ok(axes) => axes,
                Err(error) => {
                    *slot = None;
                    pulses.push(Pulse::Disconnected {
                        device,
                        error: Some(error.to_string()),
                    });
                    continue;
                }
            };
            let grab = if self.grab {
                remote.grab()
            } else {
                Grab::Skipped
            };
            let at = seconds(SystemTime::now());
            pulses.push(Pulse::Connected {
                device,
                at,
                axes,
                grab,
            });
        }
        if !pulses.is_empty() {
            return Ok(pulses);
        }

        // The readiness of the monitor, and of each slot that held a remote
        // when it was polled.
        let (monitor_ready, ready) = {
            let mut fds = Vec::with_capacity(1 + self.remotes.len());
            let mut polled = Vec::with_capacity(self.remotes.len());
            fds.push(PollFd::new(self.hotplug.as_fd(), PollFlags::POLLIN));
            for (device, slot) in self.remotes.iter().enumerate() {
                if let Some(remote) = slot {
                    fds.push(PollFd::new(remote.as_fd(), PollFlags::POLLIN));
                    polled.push(device);
                }
            }
            // A signal caught mid-poll returns an empty batch rather than a
            // retry. The handlers of `stop` set a flag the loop reads between
            // batches, and a retry here would read it only at the next event
            // or timeout. `Remote::read` treats `Interrupted` as nothing to
            // read, one file over, and the two agree: neither kills the loop.
            match poll(&mut fds, self.timeout) {
                Ok(_) => {}
                Err(Errno::EINTR) => return Ok(pulses),
                Err(error) => return Err(io::Error::from(error)),
            }
            let flags = |fd: &PollFd| fd.revents().unwrap_or(PollFlags::empty());
            let mut ready = vec![None; self.remotes.len()];
            for (fd, device) in fds.iter().skip(1).zip(polled) {
                ready[device] = Some(flags(fd));
            }
            (fds.first().map(flags).unwrap_or(PollFlags::empty()), ready)
        };

        // Two clocks for one instant, deliberately. `now` is the wall clock,
        // because it leaves this function inside a pulse and ends up compared
        // against evdev event timestamps, which are `CLOCK_REALTIME`.
        // `polled_at` never leaves; it is only ever compared against an earlier
        // reading of itself, which is the one job a wall clock cannot do.
        let now = seconds(SystemTime::now());
        let polled_at = Instant::now();

        if monitor_ready.contains(PollFlags::POLLIN) && self.hotplug.drain() {
            self.rescan(profiles, polled_at, &mut pulses)?;
        }

        // Only a slot that still holds a remote, because `poll` reports every
        // fd that is ready when it returns rather than only the one that woke
        // it: a disconnect can make the device fd POLLHUP-ready and the udev
        // monitor fd POLLIN-ready inside the same blocking window, and `rescan`
        // above may already have found the device gone and reported it. A
        // vanished device reports POLLERR rather than becoming readable, and the
        // read below would return ENODEV; either way the remote is gone, but it
        // must be reported only once per disconnection. A slot that holds a
        // remote now holds the one that was polled, since `rescan` never fills
        // a slot it emptied in the same pass.
        for (device, flags) in ready.into_iter().enumerate() {
            let Some(flags) = flags else {
                continue;
            };
            let Some(remote) = self.remotes[device].as_mut() else {
                continue;
            };
            if flags.intersects(PollFlags::POLLERR | PollFlags::POLLHUP) {
                self.remotes[device] = None;
                pulses.push(Pulse::Disconnected {
                    device,
                    error: None,
                });
            } else if flags.contains(PollFlags::POLLIN) {
                let mut events = Vec::new();
                match remote.read(&mut events) {
                    Ok(unnameable) => {
                        if unnameable > 0 {
                            eprintln!(
                                "warning: {}: {unnameable} events have no name in the capture format and were dropped",
                                remote.path().display()
                            );
                        }
                        pulses.extend(
                            events
                                .into_iter()
                                .map(|event| Pulse::Event { device, event }),
                        );
                    }
                    // A real failure, distinct from a vanished device: that case
                    // is caught above by POLLERR/POLLHUP without ever reaching a
                    // read. Kept and reported rather than discarded, so this
                    // does not look like an ordinary disconnect with no cause.
                    Err(error) => {
                        self.remotes[device] = None;
                        pulses.push(Pulse::Disconnected {
                            device,
                            error: Some(error.to_string()),
                        });
                    }
                }
            }
        }

        // The backstop: udev is the ordinary way back, but recovery must not
        // depend on an event that is not guaranteed to arrive. A missed or
        // overflowed udev event, or the read failure just above, both leave a
        // remote absent with nothing pending to wake the loop. Throttled so an
        // idle loop does not enumerate `/dev/input` at the poll rate.
        //
        // Also gated on the batch still being empty. `rescan` can fail and
        // unwind through `?`, taking `pulses` with it, and a `Disconnected`
        // pushed just above by the device-fd block would be discarded along
        // with it. An empty vec makes that impossible rather than merely
        // unlikely. It costs nothing to wait: the remote only just vanished,
        // scanning for it in the same instant it left was never going to find
        // it, and the throttle already means the next tick is at most a
        // second away.
        if pulses.is_empty()
            && self.remotes.iter().any(Option::is_none)
            && due_for_rescan(self.last_scan, polled_at)
        {
            self.rescan(profiles, polled_at, &mut pulses)?;
        }

        if pulses.is_empty() {
            pulses.push(Pulse::Idle(now));
        }
        Ok(pulses)
    }

    /// Re-runs the evdev scan, whether udev said something changed or the
    /// backstop in `next_batch` decided it was time to check anyway. Records
    /// `at` as the last scan time unconditionally, so the two never race to
    /// rescan back-to-back.
    ///
    /// The scan is evdev's, not udev's, and that is deliberate. `uniq` is not an
    /// attribute of the event node. It lives on the parent, which has no
    /// devnode, so a udev enumerator matching it finds something that cannot be
    /// opened. `unique_name()` is `EVIOCGUNIQ` on the open descriptor, a
    /// property of the device itself, independent of the shape of the tree.
    ///
    /// A slot whose profile matches no device any more is emptied and
    /// reported. Then each slot without a remote takes the first matching
    /// device that no slot holds. A slot emptied in this pass stays empty until
    /// the next one, at most a second away, because the readiness `next_batch`
    /// reads after a scan belongs to the remote that was polled.
    fn rescan(
        &mut self,
        profiles: &[Box<dyn Profile>],
        at: Instant,
        pulses: &mut Vec<Pulse>,
    ) -> io::Result<()> {
        self.last_scan = Some(at);
        let found: Vec<_> = evdev::enumerate()
            .map(|(path, device)| {
                let descriptor = Remote::describe(&device);
                (path, device, descriptor)
            })
            .collect();

        let mut emptied = vec![false; self.remotes.len()];
        for (device, slot) in self.remotes.iter_mut().enumerate() {
            let present = profiles.get(device).is_some_and(|profile| {
                found
                    .iter()
                    .any(|(_, _, descriptor)| profile.matches(descriptor))
            });
            if slot.is_some() && !present {
                *slot = None;
                emptied[device] = true;
                pulses.push(Pulse::Disconnected {
                    device,
                    error: None,
                });
            }
        }

        for (path, device, descriptor) in found {
            if self
                .remotes
                .iter()
                .flatten()
                .any(|remote| remote.path() == path)
            {
                continue;
            }
            let slot = (0..self.remotes.len()).find(|&index| {
                wants(&self.remotes, index)
                    && !emptied[index]
                    && profiles
                        .get(index)
                        .is_some_and(|profile| profile.matches(&descriptor))
            });
            if let Some(index) = slot {
                self.remotes[index] = Some(Remote::open(path, device)?);
            }
        }
        Ok(())
    }
}

impl Devices for Source {
    fn next_batch(&mut self, profiles: &[Box<dyn Profile>]) -> io::Result<Vec<Pulse>> {
        Source::next_batch(self, profiles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The camera is the one gesture with no event between its edges, so the
    /// only thing that can notice a hold is the poll timing out. A loop that
    /// blocks forever never calls `Profile::tick` and a held camera is never
    /// reported.
    #[test]
    fn the_timeout_is_never_infinite() {
        assert_ne!(timeout_for(100), PollTimeout::NONE);
        assert_ne!(timeout_for(0), PollTimeout::NONE);
    }

    /// The period the profiles ask for is taken as it is: the profile already
    /// derived it from what it has to notice.
    #[test]
    fn the_timeout_is_the_period_the_profiles_ask_for() {
        assert_eq!(timeout_for(100).as_millis(), Some(100));
    }

    /// A configuration asking for no wait at all would spin a core.
    #[test]
    fn a_zero_hold_still_leaves_the_loop_a_floor_to_sleep_on() {
        assert_eq!(timeout_for(0).as_millis(), Some(MIN_TICK_MS));
    }

    /// A device that has never been scanned must not block the backstop behind
    /// a throttle window that never started.
    #[test]
    fn a_scan_that_never_happened_is_always_due() {
        assert!(due_for_rescan(None, Instant::now()));
    }

    #[test]
    fn a_scan_just_run_is_not_due_again_yet() {
        let last = Instant::now();
        assert!(!due_for_rescan(
            Some(last),
            last + Duration::from_millis(100)
        ));
    }

    /// At exactly the throttle interval, not a fraction under or over it.
    #[test]
    fn a_scan_is_due_again_exactly_at_the_throttle_boundary() {
        let last = Instant::now();
        assert!(due_for_rescan(
            Some(last),
            last + Duration::from_millis(RESCAN_EVERY_MS)
        ));
    }

    /// One millisecond under the interval is still too soon, which is what
    /// makes the boundary case above a boundary rather than a coincidence.
    #[test]
    fn a_scan_a_millisecond_short_of_the_boundary_is_not_due() {
        let last = Instant::now();
        assert!(!due_for_rescan(
            Some(last),
            last + Duration::from_millis(RESCAN_EVERY_MS - 1)
        ));
    }

    /// `Instant` is monotonic, so the loop cannot hand these over out of order,
    /// which is the whole point of holding the throttle on it rather than on
    /// the wall clock. Pinned anyway, because the function is the one place
    /// that decides what an out-of-order pair means, and `-` and
    /// `duration_since` both reserve the right to panic on one; only
    /// `saturating_duration_since` promises not to. A rescan suppressed is
    /// recoverable, a panicking daemon is not, so the answer is `false`.
    #[test]
    fn a_pair_arriving_out_of_order_is_answered_rather_than_panicked_on() {
        let earlier = Instant::now();
        let last = earlier + Duration::from_millis(5000);
        assert!(!due_for_rescan(Some(last), earlier));
    }

    #[test]
    fn a_slot_is_opened_only_for_a_profile_without_a_device() {
        assert!(wants(&[None, Some(())], 0));
        assert!(!wants(&[None, Some(())], 1));
    }
}
