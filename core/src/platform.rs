use std::io;

use crate::device::{Axes, Profile};
use crate::raw::Event;

/// What came of asking for exclusive access to a device.
#[derive(Debug)]
pub enum Grab {
    Taken,
    Refused(String),
    Skipped,
}

/// What one turn of the platform's loop produced. `device` is the index of
/// the profile in the list the daemon holds.
#[derive(Debug)]
pub enum Pulse {
    Connected {
        device: usize,
        at: f64,
        axes: Axes,
        grab: Grab,
    },
    Event {
        device: usize,
        event: Event,
    },
    Disconnected {
        device: usize,
        error: Option<String>,
    },
    /// Nothing arrived before the timeout, at this time of the event clock.
    Idle(f64),
}

/// The input devices of the platform, behind one call that waits.
pub trait Devices {
    /// Waits for events, a change of the devices present, or the timeout.
    /// `profiles` say which devices are wanted: a device that matches a
    /// profile with no open device is opened.
    fn next_batch(&mut self, profiles: &[Box<dyn Profile>]) -> io::Result<Vec<Pulse>>;
}

/// Keys and the pointer, injected into the system.
pub trait Injector {
    fn tap(&mut self, key: &str) -> io::Result<()>;
    fn press(&mut self, key: &str) -> io::Result<()>;
    fn release(&mut self, key: &str) -> io::Result<()>;
    /// One movement of the wheel in high resolution units, 120 to a notch.
    fn wheel(&mut self, units: i32) -> io::Result<()>;
    /// The cursor at a point of the screen, both in 0..=65535.
    fn place(&mut self, x: i32, y: i32) -> io::Result<()>;
    fn click(&mut self) -> io::Result<()>;
}
