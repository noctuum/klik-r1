use std::fmt;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;

use udev::{EventType, MonitorBuilder, MonitorSocket};

/// Whether this udev event could have changed the set of openable input nodes.
///
/// Only `Add` and `Remove` create or destroy a node; `Bind`, `Change` and
/// `Unbind` do not. Only the `eventN` node can be opened as an evdev device, so
/// the `mouseN` and parent `inputN` events of the same reconnect are noise.
/// Each power cycle measured on 2026-09-17 produced three of each.
///
/// The name must be `event` followed by digits and nothing else: a prefix test
/// alone would accept `eventual`.
pub fn is_interesting(action: EventType, devnode: Option<&Path>) -> bool {
    if !matches!(action, EventType::Add | EventType::Remove) {
        return false;
    }
    devnode
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("event"))
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// The udev monitor, reduced to a single boolean question.
pub struct Hotplug {
    socket: MonitorSocket,
}

/// Written by hand because `udev::MonitorSocket` implements neither `Debug`
/// nor `Clone`, so `#[derive(Debug)]` cannot reach through it. There is
/// nothing useful to print about the socket itself.
impl fmt::Debug for Hotplug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hotplug").finish_non_exhaustive()
    }
}

impl Hotplug {
    /// Listens on the `udev` source rather than the `kernel` one, which
    /// `MonitorBuilder::new` gives by default and is the reason this crate was
    /// chosen over an inotify watch. The kernel announces a node before udev has
    /// applied its rules, measured at 51.0 ms and 15.3 ms on 2026-09-17, and
    /// during that window the node exists with root-only permissions and without
    /// `LIBINPUT_IGNORE_DEVICE`. The udev event arrives at the end of it, which
    /// is the first moment the node can actually be opened.
    pub fn new() -> io::Result<Self> {
        let socket = MonitorBuilder::new()?.match_subsystem("input")?.listen()?;
        Ok(Self { socket })
    }

    /// Consumes every queued event and says whether any of them matters.
    ///
    /// The whole queue is drained on every call, not just the first event: one
    /// reconnect delivers six, and leaving five behind would leave the
    /// descriptor readable and spin the loop.
    pub fn drain(&mut self) -> bool {
        let mut rescan = false;
        for event in self.socket.iter() {
            if is_interesting(event.event_type(), event.devnode()) {
                rescan = true;
            }
        }
        rescan
    }
}

impl AsFd for Hotplug {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use udev::EventType;

    #[test]
    fn an_event_node_appearing_is_worth_a_rescan() {
        assert!(is_interesting(
            EventType::Add,
            Some(Path::new("/dev/input/event23"))
        ));
    }

    #[test]
    fn an_event_node_vanishing_is_worth_a_rescan() {
        assert!(is_interesting(
            EventType::Remove,
            Some(Path::new("/dev/input/event23"))
        ));
    }

    /// Each reconnect measured on 2026-09-17 also moved mouse3 and the parent
    /// inputN. Rescanning on those would triple the work and change nothing,
    /// because only the event node can be opened as an evdev device.
    #[test]
    fn the_other_nodes_of_the_same_device_are_ignored() {
        assert!(!is_interesting(
            EventType::Add,
            Some(Path::new("/dev/input/mouse3"))
        ));
    }

    /// The parent inputN has no devnode at all, and it is the node whose
    /// attributes carry uniq. That is why this monitor does not do matching.
    #[test]
    fn a_device_with_no_node_is_ignored() {
        assert!(!is_interesting(EventType::Add, None));
    }

    #[test]
    fn bind_change_and_unbind_are_not_rescans() {
        for action in [EventType::Bind, EventType::Change, EventType::Unbind] {
            assert!(
                !is_interesting(action, Some(Path::new("/dev/input/event23"))),
                "{action:?} does not create or destroy a node"
            );
        }
    }

    /// "eventual" starts with "event" and is not an event node. Matching a
    /// prefix instead of the whole name is the bug this pins.
    #[test]
    fn a_name_that_merely_starts_with_event_is_ignored() {
        assert!(!is_interesting(
            EventType::Add,
            Some(Path::new("/dev/input/eventual"))
        ));
    }
}
