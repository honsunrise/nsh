//! Hardware key transitions and mounted capture rules for custom Rust views
//! and for TSX views.
//!
//! Windows, desktops and browsers deliver [`KeyEvent`] notifications for the
//! focused view; app-rendered interaction surfaces can forward the logical
//! key, the optional physical identity, repeats and releases to their own
//! input engine. Raw logical key names are input data: the DOM's spelling
//! for named keys, and the character a printable key reports, never forced
//! onto a made-up layout. Named shortcuts remain [`crate::keymap`]'s
//! responsibility.
//!
//! [`KeyCapture`] decides native consumption before callbacks run. Its keys
//! are canonical [`Keystroke`]s, so a capture shares the shortcut vocabulary
//! and normalization ([`KeyEvent::keystroke`]). Focus stays with the
//! platform's existing Tab and explicit-focus behavior.

use std::sync::Arc;

use crate::{
    element::SharedString,
    keymap::{Keystroke, Modifiers},
    platform::Os,
};

/// The physical identity of a key, when the source reports one.
///
/// It is layout independent: what the key is, never what it types. A source
/// that has no real code (GPUI's logical events, a terminal) leaves
/// [`KeyEvent::physical`] `None` rather than guessing one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PhysicalKey {
    /// The platform's numeric code, as it reports it: a Windows scan code
    /// (`scan | (extended << 8)`, from `WM_KEYDOWN`'s LPARAM), AppKit's
    /// `NSEvent.keyCode`, a UIKit HID usage, a GTK keycode or an Android
    /// scan code (`KeyEvent.getScanCode`, the Linux key code of the key). Code 0 is a key on Apple keyboards; Android reports 0 for
    /// no scan code, which is `None`.
    Native(u32),
    /// The DOM's `KeyboardEvent.code` (`"KeyA"`, `"Digit1"`, `"ArrowLeft"`).
    Code(SharedString),
}

/// A key that went down or came up on the focused view (`on_key_down` /
/// `on_key_up`), as GPUI's `KeyDownEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeyEvent {
    /// The logical key the source reports: the DOM's spelling for named keys
    /// (`"Enter"`, `"Escape"`, `"ArrowLeft"`) and the character a printable
    /// key reports in its actual case, on the layout in use. Not normalized
    /// to another layout. `"Unidentified"` when the source names none.
    pub key: SharedString,
    /// A preview of the character the key produces with the modifiers held
    /// (`"A"` for Shift+A, `"?"` for Shift+/), from the source's key text.
    /// Never committed input-method text. `None` for keys that print
    /// nothing and when the source reports no text.
    pub text: Option<SharedString>,
    /// The key's physical identity, when the source reports one.
    pub physical: Option<PhysicalKey>,
    /// The modifiers held after this transition. Releasing one Shift keeps
    /// `shift` set while the other Shift is still held.
    pub modifiers: Modifiers,
    /// An auto-repeat of a key held down (the press itself is not one).
    pub repeat: bool,
    /// This transition matches a mounted [`KeyCapture`] rule of the focused
    /// view or an enabled ancestor: the platform consumes it, no text or
    /// native default follows. A listener is still notified.
    pub captured: bool,
}

impl Default for KeyEvent {
    /// A key the source has not identified.
    fn default() -> Self {
        Self {
            key: SharedString::new_static("Unidentified"),
            text: None,
            physical: None,
            modifiers: Modifiers::default(),
            repeat: false,
            captured: false,
        }
    }
}

impl KeyEvent {
    /// The canonical shortcut stroke of this transition, for
    /// [`KeyCapture::captures`]: the key's US-layout position from its
    /// physical code when the source gives one this core knows (Windows
    /// virtual keys, AppKit key codes, HID usages, Android key codes, DOM
    /// codes), else the reported key's shortcut name
    /// ([`crate::keymap::Key::from_name`]).
    ///
    /// A GTK keycode and a terminal's codes are not placed here: the
    /// backend that has the layout (GTK's Latin group, which its shortcuts
    /// match by) builds the stroke, and otherwise the reported key is used.
    /// On Windows this is the scan code on the US layout; the app keymap's
    /// vocabulary comes from the virtual key (`Key::from_virtual_key`), so
    /// a Win32 backend that has the VK builds that stroke itself and gives
    /// it to [`KeyCapture::captures_key`].
    /// A source that reports only the character it typed (no physical
    /// code) has its shifted punctuation placed back on the US key with
    /// Shift (`?` is `shift+/`, `*` is `shift+8`), as the shortcut
    /// vocabulary spells them; a capital stays its lower case key with the
    /// source's modifiers (Caps Lock is not Shift). The event's own `key`
    /// and `text` stay as the source reported them.
    ///
    /// `None` for the modifier keys themselves and for any key that is no
    /// shortcut: [`KeyCapture::All`] still captures it
    /// ([`KeyCapture::captures_key`]). `os` names what the numeric code
    /// means; a browser passes [`Os::Web`] and its DOM codes.
    pub fn keystroke(&self, os: Os) -> Option<Keystroke> {
        if let Some(key) = self.physical.as_ref().and_then(|physical| physical.key(os)) {
            return Some(Keystroke {
                modifiers: self.modifiers,
                key,
            });
        }
        // No physical code: the reported key's shortcut name. A single
        // character is placed back on its US key with Shift (`?` is
        // `shift+/`, `A` is `shift+a`), as the shortcuts spell it; the
        // event itself keeps the source's character.
        let mut modifiers = self.modifiers;
        let mut chars = self.key.chars();
        let key = match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_graphic() => {
                match crate::keymap::unshifted(c) {
                    Some(base) => {
                        modifiers.shift = true;
                        crate::keymap::Key::Char(base)
                    }
                    // A capital is not evidence of Shift: Caps Lock types
                    // one with the source's modifiers unchanged. Its
                    // shortcut key is the lower case one.
                    None => crate::keymap::Key::Char(c.to_ascii_lowercase()),
                }
            }
            _ => crate::keymap::Key::from_name(&self.key)?,
        };
        Some(Keystroke { modifiers, key })
    }
}

impl PhysicalKey {
    /// The shortcut key of this code on `os`, by the key's US-layout place.
    fn key(&self, os: Os) -> Option<crate::keymap::Key> {
        match self {
            PhysicalKey::Native(code) => {
                let code = u16::try_from(*code).ok()?;
                match os {
                    // A scan code (`scan | extended << 8`), not a virtual
                    // key: scan 0x1E is `a`, where VK 0x1E is no key.
                    Os::Windows => crate::keymap::Key::from_windows_scan_code(code),
                    // AppKit's virtual key codes (`NSEvent.keyCode`), not
                    // UIKit's HID usages.
                    Os::MacOs => crate::keymap::Key::from_appkit_keycode(code),
                    Os::Ios => crate::keymap::Key::from_hid_usage(code),
                    // Android's scan code is the key's Linux key code.
                    Os::Android => crate::keymap::Key::from_scan_code(code),
                    // GTK keycodes (and a terminal's, which are none) are
                    // not a US layout the core can name: the reported key
                    // is used.
                    Os::Linux | Os::Web => None,
                }
            }
            PhysicalKey::Code(code) => crate::keymap::Key::from_code(code),
        }
    }
}

/// The keys a view consumes before the platform's native defaults run
/// ([`crate::element::Interactive::capture_keys`]).
///
/// A view mounts one as a reactive prop; the platform reads the mounted
/// rule of the focused view and its ancestry and decides without waiting
/// for a listener. An eligible app shortcut keeps its priority, and a
/// native input or child keeps its own keys. Disabled views, and views that
/// left the focused view's ancestry, capture nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyCapture {
    /// Nothing is captured (the default).
    #[default]
    None,
    /// Every key is captured, named and modifier keys included.
    All,
    /// The listed keys are captured: each canonical [`Keystroke`] matches
    /// the same key and exactly its modifiers.
    Keys(Arc<[Keystroke]>),
}

impl KeyCapture {
    /// A rule capturing `keys` (each a [`Keystroke`]'s key and modifiers,
    /// both exact).
    pub fn keys(keys: impl IntoIterator<Item = Keystroke>) -> Self {
        KeyCapture::Keys(keys.into_iter().collect())
    }

    /// Whether this rule captures nothing: [`Self::None`], or
    /// [`Self::Keys`] without keys. The platform need not look at a key's
    /// event where it is true.
    pub fn is_none(&self) -> bool {
        match self {
            KeyCapture::None => true,
            KeyCapture::All => false,
            KeyCapture::Keys(keys) => keys.is_empty(),
        }
    }

    /// Whether this rule captures the transition whose canonical shortcut
    /// stroke is `keystroke`: [`Self::All`] everything, [`Self::Keys`] those
    /// that equal one of its strokes, [`Self::None`] nothing.
    ///
    /// This only matches the key and its modifiers: the platform applies the
    /// rule to the enabled nodes on the focused view's ancestry. The event's
    /// text does not change the match: Shift+A is still the `a` key.
    pub fn captures(&self, keystroke: &Keystroke) -> bool {
        match self {
            KeyCapture::None => false,
            KeyCapture::All => true,
            KeyCapture::Keys(keys) => keys.contains(keystroke),
        }
    }

    /// [`Self::captures`] for a key with no canonical shortcut stroke (a
    /// non-ASCII character or a key the source cannot name): [`Self::All`]
    /// captures it, [`Self::Keys`] cannot match it, [`Self::None`] never.
    pub fn captures_key(&self, keystroke: Option<&Keystroke>) -> bool {
        match self {
            KeyCapture::None => false,
            KeyCapture::All => true,
            KeyCapture::Keys(keys) => keystroke.is_some_and(|keystroke| keys.contains(keystroke)),
        }
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

    fn event(key: &'static str, modifiers: Modifiers) -> KeyEvent {
        KeyEvent {
            key: key.into(),
            modifiers,
            ..Default::default()
        }
    }

    #[test]
    fn a_default_event_names_no_key() {
        let event = KeyEvent::default();
        assert_eq!(event.key.as_str(), "Unidentified");
        assert_eq!(event.text, None);
        assert_eq!(event.physical, None);
        assert!(!event.captured && !event.repeat);
        assert_eq!(event.keystroke(Os::Linux), None);
    }

    #[test]
    fn a_capture_with_no_keys_is_none() {
        assert!(KeyCapture::None.is_none());
        assert!(KeyCapture::keys(std::iter::empty()).is_none());
        assert!(!KeyCapture::All.is_none());
        assert!(!KeyCapture::keys([stroke("a")]).is_none());
    }

    #[test]
    fn capture_matches_the_stroke_and_modifiers_exactly() {
        let a = stroke("a");
        assert!(!KeyCapture::default().captures(&a));
        assert!(!KeyCapture::None.captures(&a));
        assert!(KeyCapture::All.captures(&a));
        assert!(KeyCapture::keys([stroke("a")]).captures(&a));
        // A keystroke names the modifiers it needs, all of them.
        assert!(!KeyCapture::keys([stroke("shift+a")]).captures(&a));
        assert!(!KeyCapture::keys([stroke("ctrl+a")]).captures(&a));
        assert!(!KeyCapture::keys([stroke("b")]).captures(&a));

        let shifted = event(
            "A",
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        let shifted = shifted.keystroke(Os::Linux).unwrap();
        assert!(KeyCapture::keys([stroke("shift+a")]).captures(&shifted));
        assert!(!KeyCapture::keys([stroke("a")]).captures(&shifted));

        // A shortcut names US-layout ASCII keys: another layout's character
        // has no keystroke, and only `All` captures it.
        let e_acute = event("é", Modifiers::default());
        assert_eq!(e_acute.keystroke(Os::Linux), None);
        assert!(KeyCapture::All.captures_key(None));
        assert!(!KeyCapture::keys([stroke("a")]).captures_key(None));
    }

    #[test]
    fn capture_matches_named_keys_by_their_shortcut_names() {
        for (key, name) in [
            ("ArrowLeft", "left"),
            ("Enter", "enter"),
            ("Escape", "escape"),
            ("Tab", "tab"),
            ("Backspace", "backspace"),
            ("Delete", "delete"),
            ("PageDown", "pagedown"),
            ("F5", "f5"),
            (" ", "space"),
        ] {
            let pressed = event(key, Modifiers::default())
                .keystroke(Os::Linux)
                .unwrap_or_else(|| panic!("{key}"));
            assert!(
                KeyCapture::keys([stroke(name)]).captures(&pressed),
                "{name}"
            );
        }

        // An unknown key is no keystroke: only `All` captures it.
        let unknown = event("dead_acute", Modifiers::default());
        assert_eq!(unknown.keystroke(Os::Linux), None);
        assert!(KeyCapture::All.captures_key(None));
        assert!(!KeyCapture::keys([stroke("a")]).captures_key(None));

        // A modifier key is no keystroke: only `All` captures it.
        let shift = event(
            "Shift",
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert_eq!(shift.keystroke(Os::Linux), None);
        assert!(KeyCapture::All.captures_key(None));

        // A non-ASCII key the source cannot name as a shortcut.
        let sharp_s = event("ß", Modifiers::default());
        assert_eq!(sharp_s.keystroke(Os::Linux), None);
        assert!(!KeyCapture::keys([stroke("a")]).captures_key(None));
    }

    #[test]
    fn the_physical_code_names_the_us_layout_key() {
        // A DOM code beats a reported non-US character: shortcuts work on
        // any layout.
        let cyrillic = KeyEvent {
            key: "ф".into(),
            physical: Some(PhysicalKey::Code("KeyA".into())),
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            cyrillic.keystroke(Os::Web),
            Some(Keystroke {
                modifiers: Modifiers {
                    shift: true,
                    ..Default::default()
                },
                key: ShortcutKey::Char('a'),
            })
        );
        // A Windows virtual key, an AppKit key code (0 is `A`), a HID
        // usage and an Android key code.
        let native = |code| KeyEvent {
            physical: Some(PhysicalKey::Native(code)),
            ..Default::default()
        };
        // Windows scan codes: 0x1E is `a` (VK 0x1E is no key), 0x30 is `b`
        // (VK 0x30 is `0`), 0x14B is the extended Left arrow.
        assert_eq!(
            native(0x1E).keystroke(Os::Windows).map(|k| k.key),
            Some(ShortcutKey::Char('a'))
        );
        assert_eq!(
            native(0x30).keystroke(Os::Windows).map(|k| k.key),
            Some(ShortcutKey::Char('b'))
        );
        assert_eq!(
            native(0x14B).keystroke(Os::Windows).map(|k| k.key),
            Some(ShortcutKey::Left)
        );
        assert_eq!(
            native(0x47).keystroke(Os::Windows).map(|k| k.key),
            Some(ShortcutKey::Char('7'))
        );
        assert_eq!(
            native(0x147).keystroke(Os::Windows).map(|k| k.key),
            Some(ShortcutKey::Home)
        );
        assert_eq!(
            native(0).keystroke(Os::MacOs).map(|k| k.key),
            Some(ShortcutKey::Char('a'))
        );
        assert_eq!(
            native(4).keystroke(Os::Ios).map(|k| k.key),
            Some(ShortcutKey::Char('a'))
        );
        // Android's scan code of `A` (its key code is 29).
        assert_eq!(
            native(30).keystroke(Os::Android).map(|k| k.key),
            Some(ShortcutKey::Char('a'))
        );
        // A code this core cannot place falls back to the reported key.
        let linux = KeyEvent {
            key: "a".into(),
            physical: Some(PhysicalKey::Native(38)),
            ..Default::default()
        };
        assert_eq!(
            linux.keystroke(Os::Linux).map(|k| k.key),
            Some(ShortcutKey::Char('a'))
        );

        // DOM named and punctuation codes.
        for (code, expected) in [
            ("ArrowLeft", ShortcutKey::Left),
            ("Enter", ShortcutKey::Enter),
            ("Digit1", ShortcutKey::Char('1')),
            ("Slash", ShortcutKey::Char('/')),
            ("F5", ShortcutKey::F(5)),
            ("Space", ShortcutKey::Char(' ')),
        ] {
            let event = KeyEvent {
                physical: Some(PhysicalKey::Code(code.into())),
                ..Default::default()
            };
            assert_eq!(
                event.keystroke(Os::Web).map(|k| k.key),
                Some(expected),
                "{code}"
            );
        }
        let unknown = KeyEvent {
            physical: Some(PhysicalKey::Code("IntlBackslash".into())),
            key: "\\".into(),
            ..Default::default()
        };
        assert_eq!(
            unknown.keystroke(Os::Web).map(|k| k.key),
            Some(ShortcutKey::Char('\\'))
        );
    }

    #[test]
    fn text_does_not_change_the_match() {
        let slash = KeyEvent {
            key: "/".into(),
            text: Some("?".into()),
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            KeyCapture::keys([stroke("shift+/")]).captures(&slash.keystroke(Os::Linux).unwrap())
        );
        // A source that reports only the character it typed (`?`, no
        // physical code) is placed back on its US key with Shift for
        // matching; the event keeps the character it reported.
        let question = event("?", Modifiers::default());
        assert_eq!(question.key.as_str(), "?");
        assert!(
            KeyCapture::keys([stroke("shift+/")])
                .captures(question.keystroke(Os::Linux).as_ref().unwrap())
        );
        let bang = event("!", Modifiers::default());
        assert!(
            KeyCapture::keys([stroke("shift+1")])
                .captures(bang.keystroke(Os::Linux).as_ref().unwrap())
        );
        // A capital is not Shift (Caps Lock types one with the source's
        // modifiers): its shortcut key is the lower case one.
        let capital = event("A", Modifiers::default());
        assert!(
            KeyCapture::keys([stroke("a")])
                .captures(capital.keystroke(Os::Linux).as_ref().unwrap())
        );
        assert!(
            !KeyCapture::keys([stroke("shift+a")])
                .captures(capital.keystroke(Os::Linux).as_ref().unwrap())
        );
        let shifted_capital = event(
            "A",
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert!(
            KeyCapture::keys([stroke("shift+a")])
                .captures(shifted_capital.keystroke(Os::Linux).as_ref().unwrap())
        );
        // A keystroke built with `?` itself is not the US key's shortcut.
        let question_stroke = Keystroke {
            key: ShortcutKey::Char('?'),
            modifiers: Modifiers::default(),
        };
        assert!(
            !KeyCapture::keys([question_stroke])
                .captures(question.keystroke(Os::Linux).as_ref().unwrap())
        );
    }
}
