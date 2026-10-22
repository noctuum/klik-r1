use std::cell::RefCell;
use std::collections::HashMap;

use klik_core::module::{Rect, Window};
use zbus::Message;
use zbus::blocking::Connection;
use zbus::zvariant::{DynamicType, OwnedValue};

const SERVICE: &str = "org.kde.KWin";
const PATH: &str = "/KWin";
const INTERFACE: &str = "org.kde.KWin";
const DESKTOPS_PATH: &str = "/VirtualDesktopManager";
const DESKTOPS_INTERFACE: &str = "org.kde.KWin.VirtualDesktopManager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// A window rectangle out of what `getWindowInfo` returns.
///
/// KWin reports the four numbers as doubles. Truncating rather than rounding
/// keeps a window from being reported larger than it is, which matters because
/// the aim point is computed as a fraction of the width.
///
/// A window with no area is refused: there is no point inside it to aim at.
pub fn rect_from_info(info: &HashMap<String, OwnedValue>) -> Option<Rect> {
    fn number(info: &HashMap<String, OwnedValue>, key: &str) -> Option<i32> {
        // `zvariant` implements `TryFrom<&OwnedValue>` for `f64`, so the value
        // is read through the reference and never cloned.
        f64::try_from(info.get(key)?).ok().map(|v| v as i32)
    }
    let rect = Rect {
        x: number(info, "x")?,
        y: number(info, "y")?,
        width: number(info, "width")?,
        height: number(info, "height")?,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return None;
    }
    Some(rect)
}

/// A window's class out of what `getWindowInfo` returns, read from
/// `resourceClass`. `None` when the field is missing or is not a string.
pub fn class_from_info(info: &HashMap<String, OwnedValue>) -> Option<String> {
    <&str>::try_from(info.get("resourceClass")?)
        .ok()
        .map(str::to_string)
}

/// A window's caption out of what `getWindowInfo` returns, read from
/// `caption`. Measured on 2026-09-27: an editor window answered in the form
/// `VSCodium - <project>`. `None` when the field is missing or is not a
/// string.
pub fn caption_from_info(info: &HashMap<String, OwnedValue>) -> Option<String> {
    <&str>::try_from(info.get("caption")?)
        .ok()
        .map(str::to_string)
}

/// The window `getWindowInfo` describes, with the id it was asked for. A
/// window with no area is no window to aim at or to name; a missing class or
/// caption is an empty one, which a rule's pattern then matches or not.
pub fn window_from_info(id: &str, info: &HashMap<String, OwnedValue>) -> Option<Window> {
    Some(Window {
        id: id.to_string(),
        class: class_from_info(info).unwrap_or_default(),
        title: caption_from_info(info).unwrap_or_default(),
        rect: rect_from_info(info)?,
    })
}

/// The name of the desktop with the id `current`, out of the property
/// `desktops`.
///
/// Measured on 2026-10-22: `busctl get-property` answers that property as
/// `a(uss)`, the position, the id and the name of each desktop, though the
/// introspection declares `a(iss)`. The id is what `current` reports.
pub fn desktop_name(current: &str, desktops: &[(u32, String, String)]) -> Option<String> {
    desktops
        .iter()
        .find(|(_, id, _)| id == current)
        .map(|(_, _, name)| name.clone())
}

/// The session bus connection KWin is asked over.
///
/// Held open for the life of the daemon. A fresh connection per call is what
/// `busctl` does and it measured 2.44 ms on 2026-09-19 against a 1.15 ms bare
/// process spawn, so most of that figure is setup this avoids entirely.
///
/// The connection sits behind a `RefCell` so a failed call can replace it
/// without every caller holding a mutable borrow. One reconnect and retry
/// covers KWin restarting, and a compositor that is not coming back does not
/// hold the loop for more than that.
#[derive(Debug)]
pub struct Bus {
    connection: RefCell<Connection>,
}

impl Bus {
    pub fn new() -> zbus::Result<Self> {
        Ok(Self {
            connection: RefCell::new(Connection::session()?),
        })
    }

    /// KWin applies its own `navigationWrappingAround` setting, so the daemon
    /// holds no notion of desktop order and cannot disagree with the user's.
    pub fn next_desktop(&self) -> zbus::Result<()> {
        self.call("nextDesktop", &())?;
        Ok(())
    }

    pub fn previous_desktop(&self) -> zbus::Result<()> {
        self.call("previousDesktop", &())?;
        Ok(())
    }

    /// What KWin knows of a window, by the id `kdotool getactivewindow`
    /// reports.
    pub fn window_info(&self, id: &str) -> zbus::Result<HashMap<String, OwnedValue>> {
        let reply = self.call("getWindowInfo", &(id,))?;
        reply.body().deserialize()
    }

    /// The id of the desktop the user is looking at.
    pub fn current_desktop(&self) -> zbus::Result<String> {
        let value = self.property("current")?;
        Ok(String::try_from(value)?)
    }

    /// Every desktop as position, id and name.
    pub fn desktops(&self) -> zbus::Result<Vec<(u32, String, String)>> {
        let value = self.property("desktops")?;
        Ok(Vec::<(u32, String, String)>::try_from(value)?)
    }

    fn property(&self, name: &str) -> zbus::Result<OwnedValue> {
        let reply = self.call_on(
            DESKTOPS_PATH,
            PROPERTIES,
            "Get",
            &(DESKTOPS_INTERFACE, name),
        )?;
        reply.body().deserialize()
    }

    fn call<B>(&self, method: &str, body: &B) -> zbus::Result<Message>
    where
        B: serde::Serialize + DynamicType,
    {
        self.call_on(PATH, INTERFACE, method, body)
    }

    /// One call, and one reconnect if it failed.
    ///
    /// KWin restarting takes its bus name with it and leaves this connection
    /// pointing at nothing. Retrying on a fresh connection covers that; retrying
    /// more than once would hold the loop against a compositor that is gone. The
    /// retry is unconditional rather than keyed on an error variant, because
    /// which variant a dead peer produces has not been measured, and guessing
    /// it wrong would silently disable the retry.
    fn call_on<B>(
        &self,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> zbus::Result<Message>
    where
        B: serde::Serialize + DynamicType,
    {
        let first = self.connection.borrow().call_method(
            Some(SERVICE),
            path,
            Some(interface),
            method,
            body,
        );
        let Err(_) = first else {
            return first;
        };
        let fresh = Connection::session()?;
        let reply = fresh.call_method(Some(SERVICE), path, Some(interface), method, body)?;
        *self.connection.borrow_mut() = fresh;
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn info(pairs: &[(&str, f64)]) -> HashMap<String, OwnedValue> {
        pairs
            .iter()
            .map(|(k, v)| {
                (
                    (*k).to_string(),
                    OwnedValue::try_from(Value::F64(*v)).unwrap(),
                )
            })
            .collect()
    }

    fn text(value: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(value)).unwrap()
    }

    /// The numbers KWin returned for an editor window on 2026-09-19, which
    /// `kdotool` reported identically.
    #[test]
    fn the_rectangle_comes_out_of_the_four_doubles() {
        let map = info(&[
            ("x", 0.0),
            ("y", 32.0),
            ("width", 1920.0),
            ("height", 1048.0),
        ]);
        assert_eq!(
            rect_from_info(&map),
            Some(Rect {
                x: 0,
                y: 32,
                width: 1920,
                height: 1048
            })
        );
    }

    /// KWin reports these as doubles even though every value observed has been
    /// whole. Truncation is the rule, so a window is never reported wider than
    /// it is.
    #[test]
    fn a_fractional_value_truncates_rather_than_rounding_up() {
        let map = info(&[("x", 0.9), ("y", 0.0), ("width", 100.9), ("height", 100.0)]);
        assert_eq!(
            rect_from_info(&map),
            Some(Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 100
            })
        );
    }

    #[test]
    fn a_map_missing_a_field_is_refused() {
        let map = info(&[("x", 0.0), ("y", 32.0), ("width", 1920.0)]);
        assert_eq!(rect_from_info(&map), None);
    }

    /// `a_map_missing_a_field_is_refused` drops `height`, and the zero-area
    /// guard refuses that on its own. This map keeps `width` and `height`
    /// positive, so only a missing `x` can produce the `None`.
    #[test]
    fn a_map_missing_only_x_is_refused() {
        let map = info(&[("y", 32.0), ("width", 1920.0), ("height", 1048.0)]);
        assert_eq!(rect_from_info(&map), None);
    }

    /// The mirror of the test above, for `y`. `x` and `y` go through separate
    /// calls to `number`, so a defect in one would not show up in a test of
    /// the other.
    #[test]
    fn a_map_missing_only_y_is_refused() {
        let map = info(&[("x", 0.0), ("width", 1920.0), ("height", 1048.0)]);
        assert_eq!(rect_from_info(&map), None);
    }

    #[test]
    fn an_empty_map_is_refused() {
        assert_eq!(rect_from_info(&HashMap::new()), None);
    }

    /// A window with no area has no point inside it to aim at.
    #[test]
    fn a_window_with_no_area_is_refused() {
        let map = info(&[("x", 0.0), ("y", 0.0), ("width", 0.0), ("height", 100.0)]);
        assert_eq!(rect_from_info(&map), None);
        // Both halves of the guard. With the height half gone, a window of no
        // height passes and the cursor is placed on its top edge, where the
        // wheel turns whatever window lies under that point.
        let flat = info(&[("x", 0.0), ("y", 0.0), ("width", 100.0), ("height", 0.0)]);
        assert_eq!(rect_from_info(&flat), None);
    }

    /// `codium` is what KWin returned for the class of an editor window on
    /// 2026-09-21. The map holds only `resourceClass`, so reading any other
    /// field gives `None` and fails here.
    #[test]
    fn the_class_comes_out_of_the_map() {
        let map = HashMap::from([("resourceClass".to_string(), text("codium"))]);
        assert_eq!(class_from_info(&map), Some("codium".to_string()));
    }

    /// The class comes back exactly as KWin spelled it: a rule compares it
    /// case-sensitively, so a class changed on the way would match the wrong
    /// window.
    #[test]
    fn the_class_keeps_its_case_and_its_whitespace() {
        let map = HashMap::from([("resourceClass".to_string(), text("VSCodium "))]);
        assert_eq!(class_from_info(&map), Some("VSCodium ".to_string()));
    }

    /// Every other map here holds at most one string, always in
    /// `resourceClass`, so reading the first string of any field passes them
    /// all. This one holds a string only in `resourceName`.
    #[test]
    fn a_string_in_another_field_is_not_the_class() {
        let map = HashMap::from([("resourceName".to_string(), text("codium"))]);
        assert_eq!(class_from_info(&map), None);
    }

    #[test]
    fn a_map_without_the_class_is_refused() {
        let map = info(&[("x", 0.0), ("y", 32.0)]);
        assert_eq!(class_from_info(&map), None);
    }

    #[test]
    fn a_map_without_the_caption_is_refused() {
        let map = HashMap::from([("resourceClass".to_string(), text("codium"))]);
        assert_eq!(caption_from_info(&map), None);
    }

    #[test]
    fn a_class_that_is_not_a_string_is_refused() {
        let map = info(&[("resourceClass", 1.0)]);
        assert_eq!(class_from_info(&map), None);
    }

    /// The map holds a second string under `resourceClass`, so reading that
    /// field, or the first string of any field, fails here.
    #[test]
    fn the_caption_comes_out_of_the_map() {
        let map = HashMap::from([
            ("resourceClass".to_string(), text("codium")),
            ("caption".to_string(), text("VSCodium - klik-r1")),
        ]);
        assert_eq!(
            caption_from_info(&map),
            Some("VSCodium - klik-r1".to_string())
        );
    }

    #[test]
    fn a_caption_that_is_not_a_string_is_refused() {
        let map = info(&[("caption", 1.0)]);
        assert_eq!(caption_from_info(&map), None);
    }

    fn editor() -> HashMap<String, OwnedValue> {
        let mut map = info(&[
            ("x", 0.0),
            ("y", 32.0),
            ("width", 1920.0),
            ("height", 1048.0),
        ]);
        map.insert("resourceClass".to_string(), text("codium"));
        map.insert("caption".to_string(), text("VSCodium - klik-r1"));
        map
    }

    #[test]
    fn the_window_comes_out_of_one_info_map() {
        assert_eq!(
            window_from_info("w1", &editor()),
            Some(Window {
                id: "w1".to_string(),
                class: "codium".to_string(),
                title: "VSCodium - klik-r1".to_string(),
                rect: Rect {
                    x: 0,
                    y: 32,
                    width: 1920,
                    height: 1048
                },
            })
        );
    }

    #[test]
    fn a_window_without_a_class_has_an_empty_one() {
        let mut map = editor();
        map.remove("resourceClass");
        let window = window_from_info("w1", &map).unwrap();
        assert_eq!(window.class, "");
        assert_eq!(window.title, "VSCodium - klik-r1");
    }

    /// The shape measured on 2026-10-22, with made-up ids.
    #[test]
    fn the_name_of_the_current_desktop_is_looked_up_by_its_id() {
        let desktops = vec![
            (0, "d-one".to_string(), "Desktop 1".to_string()),
            (1, "d-two".to_string(), "Desktop 2".to_string()),
        ];
        assert_eq!(
            desktop_name("d-two", &desktops).as_deref(),
            Some("Desktop 2")
        );
        assert_eq!(desktop_name("d-nine", &desktops), None);
    }
}
