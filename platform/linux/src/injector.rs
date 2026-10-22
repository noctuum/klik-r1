use std::io;

use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, AttributeSetRef, EventType, InputEvent, KeyCode,
    PropType, RelativeAxisCode, UinputAbsSetup,
};
use klik_core::platform::Injector;

/// The top of both absolute axes of the pointer. A position is a fraction of
/// the screen in 0..=`AXIS_MAX`, so the pointer needs no geometry of its own.
pub const AXIS_MAX: i32 = 65535;

/// High resolution units in one wheel click, as the kernel defines a click.
pub const HI_RES_PER_CLICK: i32 = 120;

/// The four events one keystroke is made of.
///
/// Separated from the device so the shape can be tested without creating one.
/// The two `SYN_REPORT` events are what make this a keystroke rather than a
/// single atomic change: the kernel groups by report, and a press and a release
/// inside one report arrive together and are seen by no client as a key going
/// down and coming back up.
pub fn tap_events(key: KeyCode) -> Vec<InputEvent> {
    vec![
        InputEvent::new(EventType::KEY.0, key.0, 1),
        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        InputEvent::new(EventType::KEY.0, key.0, 0),
        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
    ]
}

/// The two events that put a key down and close the report.
///
/// Separated from the device for the same reason as `tap_events`, and because
/// these two are what brackets a modifier: built inline, a swapped pair would
/// send the key up before the tap and down after it, leaving it held, and
/// nothing could assert their shape.
pub fn press_events(key: KeyCode) -> Vec<InputEvent> {
    vec![
        InputEvent::new(EventType::KEY.0, key.0, 1),
        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
    ]
}

/// The two events that lift a key and close the report.
pub fn release_events(key: KeyCode) -> Vec<InputEvent> {
    vec![
        InputEvent::new(EventType::KEY.0, key.0, 0),
        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
    ]
}

/// Runs `write`, and once more when it fails.
fn twice(mut write: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    write().or_else(|_| write())
}

/// A virtual keyboard, created once at startup.
///
/// Separate from the pointer on purpose. libinput classifies a device by the
/// capabilities it declares, and one device declaring both a keyboard and a
/// pointer gets classified badly. That is the failure that made the remote
/// itself arrive as a touchscreen, so the daemon does not repeat it while
/// emitting.
#[derive(Debug)]
pub struct Emitter {
    device: VirtualDevice,
}

impl Emitter {
    pub fn new(keys: &AttributeSetRef<KeyCode>) -> io::Result<Self> {
        let device = VirtualDevice::builder()?
            .name("klik keyboard")
            .with_keys(keys)?
            .build()?;
        Ok(Self { device })
    }

    /// One keystroke, press and release.
    pub fn tap(&mut self, key: KeyCode) -> io::Result<()> {
        self.device.emit(&tap_events(key))
    }

    /// Holds a key down until `release` is called. A chord holds its
    /// modifiers this way around the tap of its last key.
    pub fn press(&mut self, key: KeyCode) -> io::Result<()> {
        self.device.emit(&press_events(key))
    }

    /// Lifts a key that `press` put down. A release that fails is tried once
    /// more: a modifier left down goes with every key the device sends after
    /// it.
    pub fn release(&mut self, key: KeyCode) -> io::Result<()> {
        twice(|| self.device.emit(&release_events(key)))
    }
}

/// One absolute position, as two reports: one unit beside the point, then the
/// point.
///
/// The kernel passes an absolute axis on only when its value changed. The aim
/// point of a window is the same on every series, so a single report of it
/// carried nothing after the first, and the cursor stayed wherever the mouse
/// had left it, seen on 2026-10-12. The first report makes the second one a
/// change whatever the device held. A unit is 1/65535 of the screen, a
/// fraction of a pixel, and at the far end of the axis the step goes back.
pub fn place_events(x: i32, y: i32) -> Vec<InputEvent> {
    let beside = if y < AXIS_MAX { y + 1 } else { y - 1 };
    [beside, y]
        .into_iter()
        .flat_map(|y| {
            [
                InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, x),
                InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, y),
                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
            ]
        })
        .collect()
}

/// One wheel movement in high resolution units, and the units left over.
///
/// `units` is what one received axis event is worth and `carry` is what the
/// previous event of the same series left behind. The high resolution axis
/// takes the units as they are, and the legacy axis takes whole notches only
/// once the units add up to one, which is what the high resolution axis exists
/// for: Electron reads it, older clients read the legacy one, and a held arrow
/// delivers about 33 events a second, so a whole notch per event moved the page
/// far more than the hand did.
///
/// A carry pointing the other way is discarded rather than added, because a
/// leftover from the previous direction would send the first event of the new
/// series backwards. Zero units emit no wheel event, because a wheel event
/// carrying zero is an event, and they leave the carry alone.
pub fn wheel_events(units: i32, carry: i32) -> (Vec<InputEvent>, i32) {
    if units == 0 {
        return (Vec::new(), carry);
    }
    let carry = if carry.signum() == -units.signum() {
        0
    } else {
        carry
    };
    let total = carry.saturating_add(units);
    let notches = total / HI_RES_PER_CLICK;

    let mut events = Vec::with_capacity(3);
    if notches != 0 {
        events.push(InputEvent::new(
            EventType::RELATIVE.0,
            RelativeAxisCode::REL_WHEEL.0,
            notches,
        ));
    }
    events.push(InputEvent::new(
        EventType::RELATIVE.0,
        RelativeAxisCode::REL_WHEEL_HI_RES.0,
        units,
    ));
    events.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
    (events, total % HI_RES_PER_CLICK)
}

/// A virtual pointer, created once at startup.
///
/// `INPUT_PROP_POINTER` is what keeps libinput from reading an absolute device
/// as a touchscreen, which is exactly what the remote itself is. `BTN_LEFT` is
/// its one button, which `click` sends.
#[derive(Debug)]
pub struct Pointer {
    device: VirtualDevice,
    /// High resolution units delivered but not yet worth a notch on the legacy
    /// axis. Carried from one event of a series to the next; see `wheel_events`.
    carry: i32,
}

impl Pointer {
    pub fn new() -> io::Result<Self> {
        let mut props = AttributeSet::<PropType>::new();
        props.insert(PropType::POINTER);

        let mut keys = AttributeSet::<KeyCode>::new();
        keys.insert(KeyCode::BTN_LEFT);

        let x = UinputAbsSetup::new(
            AbsoluteAxisCode::ABS_X,
            AbsInfo::new(0, 0, AXIS_MAX, 0, 0, 0),
        );
        let y = UinputAbsSetup::new(
            AbsoluteAxisCode::ABS_Y,
            AbsInfo::new(0, 0, AXIS_MAX, 0, 0, 0),
        );

        let mut axes = AttributeSet::<RelativeAxisCode>::new();
        axes.insert(RelativeAxisCode::REL_WHEEL);
        axes.insert(RelativeAxisCode::REL_WHEEL_HI_RES);

        let device = VirtualDevice::builder()?
            .name("klik pointer")
            .with_properties(&props)?
            .with_keys(&keys)?
            .with_absolute_axis(&x)?
            .with_absolute_axis(&y)?
            .with_relative_axes(&axes)?
            .build()?;
        Ok(Self { device, carry: 0 })
    }

    /// Places the cursor. Both values are in the normalized range,
    /// 0..=`AXIS_MAX`.
    pub fn place(&mut self, x: i32, y: i32) -> io::Result<()> {
        self.device.emit(&place_events(x, y))
    }

    /// One axis event's worth of scrolling, in high resolution units.
    pub fn wheel(&mut self, units: i32) -> io::Result<()> {
        let (events, carry) = wheel_events(units, self.carry);
        self.carry = carry;
        self.device.emit(&events)
    }

    /// One click of the left button, press and release in separate reports.
    pub fn click(&mut self) -> io::Result<()> {
        self.device.emit(&tap_events(KeyCode::BTN_LEFT))
    }
}

/// The keyboard and the pointer, two devices on purpose: libinput classifies
/// a device by what it declares, and one that declares both gets classified
/// badly, which is how the remote itself arrives as a touchscreen.
#[derive(Debug)]
pub struct Uinput {
    keyboard: Emitter,
    pointer: Pointer,
}

impl Uinput {
    /// `keys` are declared once: a uinput device sends only what it declared
    /// at creation. A name that is no key refuses the whole device.
    pub fn new(keys: &[String]) -> io::Result<Self> {
        let keys = crate::keys::resolve(keys)?;
        Ok(Self {
            keyboard: Emitter::new(&keys)?,
            pointer: Pointer::new()?,
        })
    }
}

fn key(name: &str) -> io::Result<KeyCode> {
    crate::keys::code(name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{name} is not a key")))
}

impl Injector for Uinput {
    fn tap(&mut self, name: &str) -> io::Result<()> {
        self.keyboard.tap(key(name)?)
    }

    fn press(&mut self, name: &str) -> io::Result<()> {
        self.keyboard.press(key(name)?)
    }

    fn release(&mut self, name: &str) -> io::Result<()> {
        self.keyboard.release(key(name)?)
    }

    fn wheel(&mut self, units: i32) -> io::Result<()> {
        self.pointer.wheel(units)
    }

    fn place(&mut self, x: i32, y: i32) -> io::Result<()> {
        self.pointer.place(x, y)
    }

    fn click(&mut self) -> io::Result<()> {
        self.pointer.click()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(events: &[InputEvent]) -> Vec<(u16, u16, i32)> {
        events
            .iter()
            .map(|e| (e.event_type().0, e.code(), e.value()))
            .collect()
    }

    /// A tap is four events, not two. The kernel groups by `SYN_REPORT`, so a
    /// press and a release inside one report are one atomic change that no
    /// client sees as a keystroke.
    #[test]
    fn a_tap_is_a_press_and_a_release_in_separate_reports() {
        let events = tap_events(KeyCode::KEY_1);
        let shape: Vec<(u16, u16, i32)> = events
            .iter()
            .map(|e| (e.event_type().0, e.code(), e.value()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (EventType::KEY.0, KeyCode::KEY_1.0, 1),
                (EventType::SYNCHRONIZATION.0, 0, 0),
                (EventType::KEY.0, KeyCode::KEY_1.0, 0),
                (EventType::SYNCHRONIZATION.0, 0, 0),
            ]
        );
    }

    /// The value in this pair is what keeps the modifier of a chord down. A 0
    /// here would lift a key that was never pressed.
    #[test]
    fn a_press_puts_the_key_down_and_closes_the_report() {
        let shape: Vec<(u16, u16, i32)> = press_events(KeyCode::KEY_LEFTCTRL)
            .iter()
            .map(|e| (e.event_type().0, e.code(), e.value()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (EventType::KEY.0, KeyCode::KEY_LEFTCTRL.0, 1),
                (EventType::SYNCHRONIZATION.0, 0, 0),
            ]
        );
    }

    /// The other half of the bracket: a 1 here would leave `KEY_LEFTCTRL`
    /// held after the chord.
    #[test]
    fn a_release_lifts_the_key_and_closes_the_report() {
        let shape: Vec<(u16, u16, i32)> = release_events(KeyCode::KEY_LEFTCTRL)
            .iter()
            .map(|e| (e.event_type().0, e.code(), e.value()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (EventType::KEY.0, KeyCode::KEY_LEFTCTRL.0, 0),
                (EventType::SYNCHRONIZATION.0, 0, 0),
            ]
        );
    }

    /// A write that answers as the script says, counted.
    fn scripted(script: &[bool]) -> (u32, io::Result<()>) {
        let mut calls = 0;
        let result = twice(|| {
            let goes = script[calls as usize];
            calls += 1;
            if goes {
                Ok(())
            } else {
                Err(io::Error::other(format!("write {calls} refused")))
            }
        });
        (calls, result)
    }

    #[test]
    fn a_write_that_fails_is_tried_once_more() {
        let (calls, result) = scripted(&[false, true]);
        assert_eq!(calls, 2);
        assert!(result.is_ok());
    }

    #[test]
    fn a_write_that_goes_through_is_not_repeated() {
        let (calls, result) = scripted(&[true, true]);
        assert_eq!(calls, 1);
        assert!(result.is_ok());
    }

    #[test]
    fn a_write_that_fails_twice_is_the_fault_of_the_second() {
        let (calls, result) = scripted(&[false, false, true]);
        assert_eq!(calls, 2);
        assert_eq!(result.unwrap_err().to_string(), "write 2 refused");
    }

    #[test]
    fn the_key_in_the_events_is_the_key_that_was_asked_for() {
        let events = tap_events(KeyCode::KEY_ENTER);
        assert!(
            events
                .iter()
                .all(|e| e.event_type() != EventType::KEY || e.code() == KeyCode::KEY_ENTER.0)
        );
    }

    /// The kernel passes an absolute axis on only when its value changed, and
    /// the aim point of a window is the same on every series. Seen on
    /// 2026-10-12: the cursor went to the aim point once after the daemon
    /// started, and after that it stayed wherever the mouse had left it. So a
    /// placement is two reports. The first is one unit beside the point, and
    /// the second then differs from what the device holds, whatever that was.
    #[test]
    fn a_placement_is_two_reports_the_first_one_unit_beside() {
        assert_eq!(
            shape(&place_events(32768, 16384)),
            vec![
                (EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, 32768),
                (EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, 16385),
                (EventType::SYNCHRONIZATION.0, 0, 0),
                (EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, 32768),
                (EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, 16384),
                (EventType::SYNCHRONIZATION.0, 0, 0),
            ]
        );
    }

    /// The step stays inside the axis: at its far end it goes back.
    #[test]
    fn the_first_report_differs_from_the_point_and_stays_on_the_axis() {
        for y in [0, 1, AXIS_MAX - 1, AXIS_MAX] {
            let ys: Vec<i32> = place_events(7, y)
                .iter()
                .filter(|e| {
                    e.event_type() == EventType::ABSOLUTE && e.code() == AbsoluteAxisCode::ABS_Y.0
                })
                .map(|e| e.value())
                .collect();
            assert_eq!(ys.len(), 2, "{y}");
            assert_eq!(ys[1], y, "{y}");
            assert_ne!(ys[0], y, "{y}");
            assert!((0..=AXIS_MAX).contains(&ys[0]), "{y}");
        }
    }

    fn axis(events: &[InputEvent], code: RelativeAxisCode) -> Option<i32> {
        events
            .iter()
            .find(|e| e.event_type() == EventType::RELATIVE && e.code() == code.0)
            .map(|e| e.value())
    }

    /// Electron, and therefore VSCodium, reads the high resolution axis, so
    /// every event turns it. Older clients read the legacy one, which is why a
    /// whole notch reaches it as well.
    #[test]
    fn a_full_notch_turns_both_wheel_axes() {
        let (events, carry) = wheel_events(120, 0);
        assert_eq!(axis(&events, RelativeAxisCode::REL_WHEEL_HI_RES), Some(120));
        // The literal, not `HI_RES_PER_CLICK`: asserted against the constant
        // this held for any value of it, including one that disagrees with the
        // legacy axis by a factor of two.
        assert_eq!(axis(&events, RelativeAxisCode::REL_WHEEL), Some(1));
        assert_eq!(carry, 0);
    }

    /// A fraction of a notch turns the high resolution axis alone. This is the
    /// whole point of the unit: at the measured 33 events a second, a notch on
    /// every event left the editor animating after the button came up.
    #[test]
    fn units_short_of_a_notch_leave_the_legacy_axis_alone() {
        let (events, carry) = wheel_events(40, 0);
        assert_eq!(axis(&events, RelativeAxisCode::REL_WHEEL_HI_RES), Some(40));
        assert_eq!(axis(&events, RelativeAxisCode::REL_WHEEL), None);
        assert_eq!(carry, 40);
    }

    /// Three events of 40 units are one notch, and the carry is what makes the
    /// third one deliver it.
    #[test]
    fn the_carry_turns_three_events_of_forty_into_one_notch() {
        let (_, carry) = wheel_events(40, 0);
        let (_, carry) = wheel_events(40, carry);
        assert_eq!(carry, 80);
        let (events, carry) = wheel_events(40, carry);
        assert_eq!(axis(&events, RelativeAxisCode::REL_WHEEL), Some(1));
        assert_eq!(carry, 0);
    }

    /// Several notches in one event go out together.
    #[test]
    fn three_notches_of_units_carry_three_notches() {
        let (events, carry) = wheel_events(360, 0);
        assert_eq!(axis(&events, RelativeAxisCode::REL_WHEEL_HI_RES), Some(360));
        assert_eq!(axis(&events, RelativeAxisCode::REL_WHEEL), Some(3));
        assert_eq!(carry, 0);
    }

    /// Down is negative on both axes, which is the direction a real wheel
    /// reports when it is turned toward the user, and the remainder keeps that
    /// sign.
    #[test]
    fn negative_units_scroll_the_other_way() {
        let (events, carry) = wheel_events(-130, 0);
        assert_eq!(
            axis(&events, RelativeAxisCode::REL_WHEEL_HI_RES),
            Some(-130)
        );
        assert_eq!(axis(&events, RelativeAxisCode::REL_WHEEL), Some(-1));
        assert_eq!(carry, -10);
    }

    /// A leftover from the other direction is discarded rather than added, or
    /// the first event after a turn would move the page backwards.
    #[test]
    fn a_change_of_direction_discards_the_carry() {
        let (events, carry) = wheel_events(-40, 80);
        assert_eq!(axis(&events, RelativeAxisCode::REL_WHEEL), None);
        assert_eq!(carry, -40, "the 80 units the other way are gone");
    }

    /// Nothing to scroll means no wheel event, rather than a wheel event
    /// carrying zero, which is a real event that clients act on. The report
    /// itself is not this function's to withhold: `VirtualDevice::emit` closes
    /// whatever it is given, an empty slice included.
    #[test]
    fn zero_units_emit_nothing() {
        let (events, carry) = wheel_events(0, 40);
        assert!(events.is_empty());
        assert_eq!(carry, 40, "an event worth nothing spends no carry");
    }

    /// `units` of the input module saturates a large `hi_res_per_event` to
    /// `i32::MAX` on purpose, and adding the carry must not undo it: a wrap
    /// gives the two axes of one frame opposite signs in a release build, and
    /// panics in a debug build, which takes the daemon down with the remote
    /// grabbed.
    #[test]
    fn a_saturated_unit_count_does_not_wrap_the_high_resolution_axis() {
        let (events, _) = wheel_events(i32::MAX, 119);
        assert_eq!(
            axis(&events, RelativeAxisCode::REL_WHEEL_HI_RES),
            Some(i32::MAX)
        );
    }

    #[test]
    fn a_wheel_report_ends_with_a_syn() {
        let (events, _) = wheel_events(40, 0);
        let last = events.last().expect("a click emits events");
        assert_eq!(last.event_type(), EventType::SYNCHRONIZATION);
    }
}
