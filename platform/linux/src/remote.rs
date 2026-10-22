use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use evdev::{AbsoluteAxisCode, Device, EventType, InputEvent};
use klik_core::device::{Axes, Descriptor};
use klik_core::platform::Grab;
use klik_core::raw::Event;

use crate::capture::{code_name, to_event};

/// The name of an absolute axis, spelled as the events of the same axis spell
/// their code, so that a position read at connect and a later event cannot
/// name one axis two ways.
pub fn axis_name(axis: AbsoluteAxisCode) -> String {
    code_name(&InputEvent::new(EventType::ABSOLUTE.0, axis.0, 0))
}

/// A remote, open and ready to read.
///
/// The node path is kept for logging, and for the scan, which skips a node a
/// slot already holds. Nothing reopens by path, because the number is reused:
/// every reconnect measured on 2026-09-17 came back as `event23` while the
/// sysfs parent walked `input39` to `input41`.
#[derive(Debug)]
pub struct Remote {
    path: PathBuf,
    device: Device,
    /// Whether the loop has already reported this remote as connected. Set by
    /// `source::Source`, which is the only thing that can tell a remote it just
    /// opened from one it has been reading for an hour.
    pub(crate) announced: bool,
}

impl Remote {
    /// Opens the device at `path` for reading, non-blocking: the loop polls
    /// before it reads, so a read with nothing waiting must return.
    pub fn open(path: PathBuf, device: Device) -> io::Result<Self> {
        device.set_nonblocking(true)?;
        Ok(Self {
            path,
            device,
            announced: false,
        })
    }

    /// What the platform knows of an enumerated device before opening it.
    pub fn describe(device: &Device) -> Descriptor {
        let id = device.input_id();
        Descriptor {
            bus: id.bus_type().0,
            vendor: id.vendor(),
            product: id.product(),
            version: id.version(),
            name: device.name().unwrap_or_default().to_string(),
            uniq: device.unique_name().map(str::to_string),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Takes `EVIOCGRAB`. A refusal is returned, never raised: the udev rule
    /// already hides the device from the compositor, and the grab is the second
    /// line of defense for the case where that rule is missing.
    ///
    /// Nothing here releases it and there is no `Drop` of our own, on purpose.
    /// `evdev::Device` implements `Drop` itself, calling `ungrab` and printing
    /// the failure, and `RawDevice` already tracks whether it holds the grab,
    /// so `ungrab` on a device that never took one is a no-op. A second copy of
    /// that flag here would be a second thing to keep true, and the release it
    /// guarded would be a discarded `Result` in front of a real one. `Grab`
    /// carries the only part of this the caller has anything to say about.
    pub fn grab(&mut self) -> Grab {
        match self.device.grab() {
            Ok(()) => Grab::Taken,
            Err(error) => Grab::Refused(error.to_string()),
        }
    }

    /// Every absolute axis and where it stands, for `Profile::connected`.
    ///
    /// This is what a recognizer needs before its first gesture. An axis the
    /// device does not report is absent rather than a zero, which would be a
    /// coordinate and would match a signature near the origin.
    pub fn axes(&self) -> io::Result<Axes> {
        let mut axes = Vec::new();
        for (axis, info) in self.device.get_absinfo()? {
            axes.push((axis_name(axis), info.value()));
        }
        Ok(axes)
    }

    /// Drains whatever is waiting. Returns how many events the fixture format
    /// has no name for, so the caller can report them instead of losing them.
    pub fn read(&mut self, into: &mut Vec<Event>) -> io::Result<usize> {
        let mut unnameable = 0;
        match self.device.fetch_events() {
            Ok(events) => {
                for event in events {
                    match to_event(&event) {
                        Some(converted) => into.push(converted),
                        None => unnameable += 1,
                    }
                }
                Ok(unnameable)
            }
            // Nothing waiting, or a signal arrived mid-syscall. The descriptor is
            // non-blocking and the loop polls before reading, so the first is
            // ordinary rather than exceptional; the second means try again, not
            // that the device is gone.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(0)
            }
            Err(error) => Err(error),
        }
    }
}

impl AsFd for Remote {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The profile of the Beauty-R1 finds its position under these two names,
    /// and its events carry the same ones.
    #[test]
    fn an_axis_is_named_as_its_events_name_it() {
        assert_eq!(axis_name(AbsoluteAxisCode::ABS_X), "ABS_X");
        assert_eq!(axis_name(AbsoluteAxisCode::ABS_Y), "ABS_Y");
    }
}
