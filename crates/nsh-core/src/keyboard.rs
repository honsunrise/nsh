//! Keyboard, wheel and text input events for views: [`KeyEvent`],
//! [`WheelEvent`] and [`TextInputEvent`], as the `on_key_down`,
//! `on_key_up`, `on_wheel`, `on_text_input` and `on_composition_change`
//! elements report them, and [`KeyCapture`], which a view mounts to have
//! the platform consume keys before its native defaults.
//!
//! Linux delivers them today, and the headless test driver; the other
//! platforms keep the API compiling and never send one yet, as they do
//! for other events only some platforms have. A view takes the keyboard
//! focus on Linux when it is `focusable(true)` (a press on it, a ref's
//! `focus()`, Tab); what has the focus hears the keys.

use std::sync::Arc;

use crate::{
    element::SharedString,
    geometry::{Point, Points},
    keymap::{Key as KeystrokeKey, Keystroke, Modifiers},
};

/// A key, independent of the layout's shifted characters.
///
/// A printable key is a [`Key::Character`] of the character it types
/// without Shift (`"a"`, `"1"`, `"/"`, `"é"`); the other keys are
/// [`Key::Named`]. Shift and the layout change [`KeyEvent::text`], not
/// the key: Shift+A is `Key::Character("a")` with `text: Some("A")`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    /// A named (non-character) key.
    Named(NamedKey),
    /// A key typing a character: the unshifted, layout-aware character,
    /// lower case for letters.
    Character(SharedString),
}

impl Default for Key {
    /// A key the platform has no name for.
    fn default() -> Self {
        Key::Named(NamedKey::Unidentified)
    }
}

impl Key {
    /// A key typing `c`.
    pub fn character(c: char) -> Key {
        let mut buf = [0; 4];
        Key::Character(SharedString::from(c.encode_utf8(&mut buf).to_owned()))
    }

    /// How the platforms name it in a payload (`"a"`, `"Enter"`, `"F5"`).
    pub fn name(&self) -> &str {
        match self {
            Key::Named(named) => named.name(),
            Key::Character(c) => c,
        }
    }

    /// The key `name` names (`Key::name`), or a character when it is one.
    pub fn from_name(name: &str) -> Option<Key> {
        let mut chars = name.chars();
        match (chars.next(), chars.next()) {
            // Space is named, though it types a character.
            (Some(' '), None) => Some(Key::Named(NamedKey::Space)),
            (Some(c), None) => Some(Key::Character(SharedString::from(c.to_string()))),
            _ => NamedKey::from_name(name).map(Key::Named),
        }
    }
}

/// A key that is not a character ([`Key::Named`]).
///
/// The set is not exhaustive: platforms report the keys their keyboards
/// have.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NamedKey {
    /// A key the platform has no name for, or does not report.
    #[default]
    Unidentified,
    /// Return / Enter (the keypad's too).
    Enter,
    /// Tab.
    Tab,
    /// Backspace (the Mac's Delete).
    Backspace,
    /// Escape.
    Escape,
    /// The space bar (it types a space: `text` is `Some(" ")`).
    Space,
    /// Forward delete.
    Delete,
    /// Insert.
    Insert,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// The left arrow.
    ArrowLeft,
    /// The right arrow.
    ArrowRight,
    /// The up arrow.
    ArrowUp,
    /// The down arrow.
    ArrowDown,
    /// A function key, F1 to F24.
    F(u8),
    /// The Shift key itself.
    Shift,
    /// The Control key itself.
    Control,
    /// The Alt key itself (Option on Apple keyboards).
    Alt,
    /// The Command / Super / Windows key itself.
    Meta,
}

impl NamedKey {
    /// Its payload name (`"Enter"`, `"ArrowLeft"`, `"F5"`).
    pub fn name(self) -> &'static str {
        match self {
            NamedKey::Unidentified => "Unidentified",
            NamedKey::Enter => "Enter",
            NamedKey::Tab => "Tab",
            NamedKey::Backspace => "Backspace",
            NamedKey::Escape => "Escape",
            NamedKey::Space => " ",
            NamedKey::Delete => "Delete",
            NamedKey::Insert => "Insert",
            NamedKey::Home => "Home",
            NamedKey::End => "End",
            NamedKey::PageUp => "PageUp",
            NamedKey::PageDown => "PageDown",
            NamedKey::ArrowLeft => "ArrowLeft",
            NamedKey::ArrowRight => "ArrowRight",
            NamedKey::ArrowUp => "ArrowUp",
            NamedKey::ArrowDown => "ArrowDown",
            NamedKey::F(1) => "F1",
            NamedKey::F(2) => "F2",
            NamedKey::F(3) => "F3",
            NamedKey::F(4) => "F4",
            NamedKey::F(5) => "F5",
            NamedKey::F(6) => "F6",
            NamedKey::F(7) => "F7",
            NamedKey::F(8) => "F8",
            NamedKey::F(9) => "F9",
            NamedKey::F(10) => "F10",
            NamedKey::F(11) => "F11",
            NamedKey::F(12) => "F12",
            NamedKey::F(13) => "F13",
            NamedKey::F(14) => "F14",
            NamedKey::F(15) => "F15",
            NamedKey::F(16) => "F16",
            NamedKey::F(17) => "F17",
            NamedKey::F(18) => "F18",
            NamedKey::F(19) => "F19",
            NamedKey::F(20) => "F20",
            NamedKey::F(21) => "F21",
            NamedKey::F(22) => "F22",
            NamedKey::F(23) => "F23",
            NamedKey::F(24) => "F24",
            NamedKey::F(_) => "F",
            NamedKey::Shift => "Shift",
            NamedKey::Control => "Control",
            NamedKey::Alt => "Alt",
            NamedKey::Meta => "Meta",
        }
    }

    /// The named key `name` names.
    pub fn from_name(name: &str) -> Option<NamedKey> {
        Some(match name {
            "Unidentified" | "unidentified" => NamedKey::Unidentified,
            "Enter" | "enter" | "Return" | "return" => NamedKey::Enter,
            "Tab" | "tab" => NamedKey::Tab,
            "Backspace" | "backspace" => NamedKey::Backspace,
            "Escape" | "escape" | "Esc" | "esc" => NamedKey::Escape,
            " " | "Space" | "space" => NamedKey::Space,
            "Delete" | "delete" => NamedKey::Delete,
            "Insert" | "insert" => NamedKey::Insert,
            "Home" | "home" => NamedKey::Home,
            "End" | "end" => NamedKey::End,
            "PageUp" | "pageup" | "Page_Up" => NamedKey::PageUp,
            "PageDown" | "pagedown" | "Page_Down" => NamedKey::PageDown,
            "ArrowLeft" | "arrowleft" | "Left" | "left" => NamedKey::ArrowLeft,
            "ArrowRight" | "arrowright" | "Right" | "right" => NamedKey::ArrowRight,
            "ArrowUp" | "arrowup" | "Up" | "up" => NamedKey::ArrowUp,
            "ArrowDown" | "arrowdown" | "Down" | "down" => NamedKey::ArrowDown,
            "Shift" | "shift" => NamedKey::Shift,
            "Control" | "control" | "Ctrl" | "ctrl" => NamedKey::Control,
            "Alt" | "alt" | "Option" | "option" => NamedKey::Alt,
            "Meta" | "meta" | "Super" | "super" | "Cmd" | "cmd" | "Command" => NamedKey::Meta,
            _ => {
                let number = name.strip_prefix('F').or_else(|| name.strip_prefix('f'))?;
                let number: u8 = number.parse().ok()?;
                if !(1..=24).contains(&number) {
                    return None;
                }
                NamedKey::F(number)
            }
        })
    }

    /// The app shortcut key of this named key, for matching a
    /// [`Keystroke`]: `None` for the modifier keys themselves, which no
    /// keystroke names.
    fn keystroke_key(self) -> Option<KeystrokeKey> {
        Some(match self {
            NamedKey::Enter => KeystrokeKey::Enter,
            NamedKey::Tab => KeystrokeKey::Tab,
            NamedKey::Backspace => KeystrokeKey::Backspace,
            NamedKey::Escape => KeystrokeKey::Escape,
            NamedKey::Space => KeystrokeKey::Char(' '),
            NamedKey::Delete => KeystrokeKey::Delete,
            NamedKey::Insert => KeystrokeKey::Insert,
            NamedKey::Home => KeystrokeKey::Home,
            NamedKey::End => KeystrokeKey::End,
            NamedKey::PageUp => KeystrokeKey::PageUp,
            NamedKey::PageDown => KeystrokeKey::PageDown,
            NamedKey::ArrowLeft => KeystrokeKey::Left,
            NamedKey::ArrowRight => KeystrokeKey::Right,
            NamedKey::ArrowUp => KeystrokeKey::Up,
            NamedKey::ArrowDown => KeystrokeKey::Down,
            NamedKey::F(n) => KeystrokeKey::F(n),
            NamedKey::Unidentified
            | NamedKey::Shift
            | NamedKey::Control
            | NamedKey::Alt
            | NamedKey::Meta => return None,
        })
    }
}

/// A key that went down or came up on the focused view
/// (`on_key_down` / `on_key_up`), as GPUI's `KeyDownEvent`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeyEvent {
    /// The key, without Shift or the layout's shifted character
    /// ([`Key::Character`] is the unshifted one).
    pub key: Key,
    /// The text this press would generate with the layout and the
    /// modifiers held (`"A"` for Shift+A, `"?"` for Shift+/): `None` for a
    /// key that types nothing, when Control, Alt or Command is held, and
    /// for a key up.
    pub text: Option<SharedString>,
    /// The modifiers held after this transition: pressing Shift sets
    /// `shift`, and releasing it clears `shift`; other modifiers stay held.
    pub modifiers: Modifiers,
    /// The platform's hardware keycode: stable for a physical key, and for
    /// what position it has on the keyboard (never what it types).
    pub physical: u32,
    /// An auto-repeat of a key held down (the press itself is not one).
    pub repeat: bool,
    /// The platform consumed this press under a [`KeyCapture`] rule of the
    /// focused view's ancestry: no text or native default follows from it.
    /// A listener is still notified, on the focused view and its
    /// ancestors, whatever this is.
    pub captured: bool,
}

impl KeyEvent {
    /// A key event of `key`, typing `text`, with `modifiers`, the
    /// hardware keycode `physical` and `repeat` if it repeats; not
    /// captured.
    pub fn new(
        key: Key,
        text: Option<SharedString>,
        modifiers: Modifiers,
        physical: u32,
        repeat: bool,
    ) -> KeyEvent {
        KeyEvent {
            key,
            text,
            modifiers,
            physical,
            repeat,
            captured: false,
        }
    }

    /// The same key event with another `text`.
    pub fn with_text(mut self, text: impl Into<Option<SharedString>>) -> KeyEvent {
        self.text = text.into();
        self
    }
}

/// The keys a view consumes before the platform's native defaults run
/// ([`crate::element::Interactive::capture_keys`]).
///
/// A view mounts one as a reactive prop; the platform reads the mounted
/// rule of the focused view and its ancestry and decides without waiting
/// for a listener. An eligible app shortcut keeps its priority, and a
/// native input or child keeps its own keys. Disabled views, and views
/// that left the focused view's ancestry, capture nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyCapture {
    /// Nothing is captured (the default).
    #[default]
    None,
    /// Every key is captured, named and modifier keys included.
    All,
    /// The listed keys are captured, each by its unshifted logical key and
    /// the exact modifiers it names.
    Keys(Arc<[Keystroke]>),
}

impl KeyCapture {
    /// A rule capturing `keys` (a [`Keystroke`]'s key and modifiers, both
    /// exact).
    pub fn keys(keys: impl IntoIterator<Item = Keystroke>) -> Self {
        KeyCapture::Keys(keys.into_iter().collect())
    }

    /// Whether this rule captures nothing: [`Self::None`], or
    /// [`Self::Keys`] without keys. The platform need not look at a key's
    /// raw event where it is true.
    pub fn is_none(&self) -> bool {
        match self {
            KeyCapture::None => true,
            KeyCapture::All => false,
            KeyCapture::Keys(keys) => keys.is_empty(),
        }
    }

    /// Whether this rule captures `event`: [`Self::All`] every key,
    /// [`Self::Keys`] those whose unshifted logical key and modifiers
    /// equal the event's, [`Self::None`] none.
    ///
    /// This only matches the key and its modifiers: the platform applies
    /// the rule to the enabled nodes on the focused view's ancestry.
    pub fn captures(&self, event: &KeyEvent) -> bool {
        match self {
            KeyCapture::None => false,
            KeyCapture::All => true,
            KeyCapture::Keys(keys) => keystroke_key(&event.key).is_some_and(|key| {
                keys.contains(&Keystroke {
                    key,
                    modifiers: event.modifiers,
                })
            }),
        }
    }
}

/// The shortcut key of a key event's key, for matching a [`Keystroke`]: a
/// character key only when it types one character. The event reports the
/// unshifted layout character; letters take the keymap's canonical lower
/// case spelling.
fn keystroke_key(key: &Key) -> Option<KeystrokeKey> {
    Some(match key {
        Key::Named(named) => named.keystroke_key()?,
        Key::Character(text) => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeystrokeKey::Char(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
    })
}

/// How far a wheel turned: whole lines or points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelDelta {
    /// Whole notches of a mouse wheel, as counts of lines: dimensionless,
    /// a notch being one line.
    Lines(Point<f32>),
    /// Precise deltas of a touchpad or a high-resolution wheel, in points.
    Points(Point<Points>),
}

impl Default for WheelDelta {
    /// No movement, in points.
    fn default() -> Self {
        WheelDelta::Points(Point::default())
    }
}

/// How far a wheel turned over a view (`on_wheel`), as DOM's `WheelEvent`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[non_exhaustive]
pub struct WheelEvent {
    /// Where the pointer was, in the view (its transforms undone, as
    /// [`crate::MouseEvent::position`]).
    pub position: Point<Points>,
    /// Where the pointer was, in the root view the view is in.
    pub page_position: Point<Points>,
    /// How far the content should move: positive y is a scroll down
    /// (content moves up, as DOM's `deltaY`), positive x a scroll right.
    pub delta: WheelDelta,
    /// The modifiers held.
    pub modifiers: Modifiers,
}

impl WheelEvent {
    /// A wheel turned by `delta` at `position` in the view (and
    /// `page_position` in its root view), with `modifiers` held.
    pub fn new(
        position: Point<Points>,
        page_position: Point<Points>,
        delta: WheelDelta,
        modifiers: Modifiers,
    ) -> WheelEvent {
        WheelEvent {
            position,
            page_position,
            delta,
            modifiers,
        }
    }
}

/// Text committed to the focused view (`on_text_input`): what a key typed,
/// a dead key's composition or an input method committed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TextInputEvent {
    /// The committed text (more than one character when an input method
    /// commits a word, or when a paste-like input method commits at once).
    pub text: SharedString,
}

impl TextInputEvent {
    /// Committed `text`.
    pub fn new(text: impl Into<SharedString>) -> TextInputEvent {
        TextInputEvent { text: text.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Key as ShortcutKey;

    /// The keystroke `text` names on a non-Apple platform.
    fn stroke(text: &str) -> Keystroke {
        Keystroke::parse_with(text, false).expect(text)
    }

    fn event(key: Key, modifiers: Modifiers) -> KeyEvent {
        KeyEvent::new(key, None, modifiers, 0, false)
    }

    #[test]
    fn keys_name_themselves() {
        for key in [
            Key::Named(NamedKey::Unidentified),
            Key::Named(NamedKey::Enter),
            Key::Named(NamedKey::ArrowDown),
            Key::Named(NamedKey::Space),
            Key::Named(NamedKey::F(12)),
            Key::character('/'),
            Key::character('é'),
        ] {
            let name = key.name().to_owned();
            assert_eq!(Key::from_name(&name), Some(key), "{name}");
        }
        assert_eq!(NamedKey::from_name("F25"), None);
        assert_eq!(NamedKey::from_name("F0"), None);
        assert_eq!(NamedKey::from_name("nope"), None);
    }

    #[test]
    fn a_capture_with_no_keys_is_none() {
        assert!(KeyCapture::None.is_none());
        assert!(KeyCapture::keys(std::iter::empty()).is_none());
        assert!(!KeyCapture::All.is_none());
        assert!(!KeyCapture::keys([stroke("a")]).is_none());
    }

    #[test]
    fn capture_matches_the_unshifted_key_and_modifiers_exactly() {
        let a = event(Key::character('a'), Modifiers::default());
        assert!(!KeyCapture::default().captures(&a));
        assert!(!KeyCapture::None.captures(&a));
        assert!(KeyCapture::All.captures(&a));
        assert!(KeyCapture::keys([stroke("a")]).captures(&a));
        // A keystroke names the modifiers it needs, all of them.
        assert!(!KeyCapture::keys([stroke("shift+a")]).captures(&a));
        assert!(!KeyCapture::keys([stroke("ctrl+a")]).captures(&a));
        assert!(!KeyCapture::keys([stroke("b")]).captures(&a));

        let shifted = event(
            Key::character('a'),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert!(KeyCapture::keys([stroke("shift+a")]).captures(&shifted));
        assert!(!KeyCapture::keys([stroke("a")]).captures(&shifted));
        // Shift+A still types `a`'s key: `text` does not change the match.
        let text = KeyEvent::new(
            Key::character('a'),
            Some(SharedString::from("A")),
            Modifiers {
                shift: true,
                ..Default::default()
            },
            0,
            false,
        );
        assert!(KeyCapture::keys([stroke("shift+a")]).captures(&text));
    }

    #[test]
    fn capture_matches_named_keys_by_their_shortcut_names() {
        for (key, name) in [
            (NamedKey::ArrowLeft, "left"),
            (NamedKey::Enter, "enter"),
            (NamedKey::Escape, "escape"),
            (NamedKey::Tab, "tab"),
            (NamedKey::Backspace, "backspace"),
            (NamedKey::Delete, "delete"),
            (NamedKey::PageDown, "pagedown"),
            (NamedKey::F(5), "f5"),
            (NamedKey::Space, "space"),
        ] {
            let pressed = event(Key::Named(key), Modifiers::default());
            assert!(
                KeyCapture::keys([stroke(name)]).captures(&pressed),
                "{name}"
            );
        }

        // An unknown key is no keystroke: only `All` captures it.
        let unknown = event(Key::default(), Modifiers::default());
        assert!(KeyCapture::All.captures(&unknown));
        assert!(!KeyCapture::keys([stroke("a")]).captures(&unknown));

        // A modifier key is no keystroke: only `All` captures it.
        let shift = event(
            Key::Named(NamedKey::Shift),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert!(KeyCapture::All.captures(&shift));
        assert!(!KeyCapture::keys([stroke("shift+a")]).captures(&shift));
    }

    #[test]
    fn capture_matches_the_layouts_unshifted_character() {
        let e_acute = event(Key::character('é'), Modifiers::default());
        let keystroke = Keystroke {
            key: ShortcutKey::Char('é'),
            modifiers: Modifiers::default(),
        };
        assert!(KeyCapture::keys([keystroke]).captures(&e_acute));

        // Shift is a modifier: `/` with Shift types `?`, and `?` is not
        // another spelling of `/`.
        let slash = KeyEvent::new(
            Key::character('/'),
            Some(SharedString::from("?")),
            Modifiers {
                shift: true,
                ..Default::default()
            },
            0,
            false,
        );
        assert!(KeyCapture::keys([stroke("shift+/")]).captures(&slash));
        let question = event(Key::character('?'), Modifiers::default());
        assert!(!KeyCapture::keys([stroke("shift+/")]).captures(&question));
        assert!(!KeyCapture::keys([stroke("/")]).captures(&question));
        // A keystroke naming `?` itself (built, as parsing rejects shifted
        // punctuation) matches it exactly.
        let question_stroke = Keystroke {
            key: ShortcutKey::Char('?'),
            modifiers: Modifiers::default(),
        };
        assert!(KeyCapture::keys([question_stroke]).captures(&question));

        // Letters take the keymap's canonical lower case spelling.
        let capital = event(
            Key::character('A'),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert!(KeyCapture::keys([stroke("shift+a")]).captures(&capital));

        // Only a key typing exactly one character can equal a keystroke.
        let two = event(
            Key::Character(SharedString::from("ab")),
            Modifiers::default(),
        );
        assert!(!KeyCapture::keys([keystroke]).captures(&two));
        let none = event(
            Key::Character(SharedString::default()),
            Modifiers::default(),
        );
        assert!(!KeyCapture::keys([stroke("a")]).captures(&none));
    }
}
