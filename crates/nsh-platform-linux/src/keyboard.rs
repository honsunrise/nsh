//! The keys of a focused view: the `on_key_down` / `on_key_up` events a
//! view and its ancestors hear, and the capture rules mounted on them.
//!
//! The focus and the keys themselves are [`crate::key_focus`]'s: its one
//! key controller per window decides, in GTK's capture phase, whether the
//! key is the app's ([`crate::keymap::claims`]), captured by the focused
//! view (`capture_keys`), or a default (Tab's traversal, Return and Space
//! pressing, a context menu, scrolling). This module only does what that
//! decision needs: it builds the [`KeyEvent`], matches the focus's capture
//! rules ([`captures`]), and reports the event to the engine as the plain
//! `KeyDown` / `KeyUp` events ([`notify`], [`released`]). No engine answer
//! is waited for: what a listener does is reported afterwards, in the
//! app's frame.
//!
//! Events follow the focus, in both phases: a key up goes to the view
//! focused when the source reports it, even when its press went to another
//! view (a focus move, a popup grab), as a browser reports it on the
//! focused element. No platform retains an original down target, and a
//! blur emits no synthetic release. Keyboard ancestry follows
//! `ViewProps::key_parent` for a portaled view whose native `Insert` parent
//! differs from its logical parent; focus, hit testing and layout keep the
//! native parent. `KeyEvent::physical` keeps the hardware keycode GDK
//! reports; the logical key is GDK's actual keyval, so Shift's case and a
//! non-US character are reported as the source gives them.
//!
//! A captured key still notifies the focused view and its ancestors (the
//! engine bubbles it), with `KeyEvent::captured` set; it runs no default
//! (no traversal, activation or context menu). A key that only *listens*
//! is not captured: the app hears it and the default still runs.
//!
//! A view that hears none of the raw keys and captures none takes
//! [`crate::key_focus`]'s original path: [`listens`] is a few mask
//! checks, with no raw [`KeyEvent`] allocation. Modifier transitions
//! remain tracked across focus moves, including from native inputs.

use std::cell::RefCell;

use gtk4::{self as gtk, gdk, glib, prelude::*};
use nsh_core::{
    EventKind, EventPayload, KeyEvent, PhysicalKey, ViewKind,
    element::SharedString,
    keymap::{Key as ShortcutKey, Keystroke},
};
use rustc_hash::FxHashMap;

use crate::{surface::Surface, view::NshView};

/// A surface's keyboard state.
#[derive(Default)]
pub(crate) struct Keyboard {
    /// The hardware keycodes held down, and the window each was pressed
    /// in, for auto-repeat detection (GDK does not mark repeats).
    pressed: RefCell<FxHashMap<u32, glib::WeakRef<gtk::Window>>>,
    /// The physical modifier keys held (by keycode) and the modifier each
    /// stands for: a key event's modifier mask comes from these, not from
    /// its pre-key state, so releasing one Shift keeps the other's.
    modifiers: RefCell<FxHashMap<u32, gdk::ModifierType>>,
}

/// The app stopped: forgets the keys held.
pub(crate) fn reset(surface: &Surface) {
    surface.keyboard.pressed.borrow_mut().clear();
    surface.keyboard.modifiers.borrow_mut().clear();
}

/// `window` lost the OS focus (`is-active` off): its releases may never
/// come, so the keys held there are no press of a key any more (another
/// window's stay). Nothing is reported: a blur is no key release.
pub(crate) fn window_deactivated(surface: &Surface, window: &gtk::Window) {
    surface
        .keyboard
        .pressed
        .borrow_mut()
        .retain(|_, held| held.upgrade().as_ref() != Some(window));
    // Their releases may never come: a held modifier is no more held.
    surface.keyboard.modifiers.borrow_mut().clear();
}

/// Whether a key went down before in `window` (an auto-repeat, which GDK
/// does not mark).
pub(crate) fn held(surface: &Surface, window: &gtk::Window, keycode: u32) -> bool {
    let mut pressed = surface.keyboard.pressed.borrow_mut();
    if pressed
        .get(&keycode)
        .is_some_and(|held| held.upgrade().as_ref() == Some(window))
    {
        return true;
    }
    pressed.insert(keycode, window.downgrade());
    false
}

/// The keyboard ancestor of a view: its logical parent when `key_parent`
/// overrides the native one (a portaled `Overlay` is inserted at a native
/// host, not under its logical parent), else its mounted native parent.
/// A logical parent outside the view's window is not an ancestor, so the
/// window boundary stays the one the app sees.
fn keyboard_parent(surface: &Surface, view: &NshView) -> Option<NshView> {
    let Some(id) = ({ view.state().props.key_parent }) else {
        return view.parent_view();
    };
    let parent = surface.mounter.try_borrow().ok()?.views.get(&id).cloned()?;
    (parent.id() != view.id() && parent.root() == view.root()).then_some(parent)
}

/// Whether `view` or a logical ancestor hears a raw key event, or captures
/// keys: only then does a key need a [`KeyEvent`], its capture rules and
/// the per-key state. A view that hears none takes
/// [`crate::key_focus`]'s original path, with nothing built.
pub(crate) fn listens(surface: &Surface, view: &NshView) -> bool {
    let mut current = Some(view.clone());
    while let Some(view) = current {
        {
            let state = view.state();
            let raw = state.events.contains(EventKind::KeyDown)
                || state.events.contains(EventKind::KeyUp);
            if raw || !state.props.key_capture.is_none() {
                return true;
            }
        }
        current = keyboard_parent(surface, &view);
    }
    false
}

/// Whether `view` hosts native content that owns its own keys and text: a
/// native input, a web view, a native component or a video player. A
/// capture rule around it never intercepts its editing, and the platform
/// does not report its keys as view input.
pub(crate) fn native_owns(view: &NshView) -> bool {
    matches!(
        view.state().kind,
        ViewKind::Input | ViewKind::WebView | ViewKind::Native | ViewKind::Video
    )
}

/// Whether the focused view or one of its enabled ancestors captures `key`
/// (`capture_keys`, `ViewProps::key_capture`): a captured key notifies and
/// runs no default (Tab's traversal, Return and Space pressing, a context
/// menu, scrolling).
///
/// The rule is matched against the canonical shortcut `Keystroke` of the
/// event (`event`'s modifiers, and the keyval that `keycode` types without
/// Shift on the event's layout group), the vocabulary `nsh/keymap` already
/// parses. The raw logical event key is never parsed as a shortcut.
pub(crate) fn captures(
    surface: &Surface,
    view: &NshView,
    event: &KeyEvent,
    keycode: u32,
    state: gdk::ModifierType,
    group: i32,
) -> bool {
    let stroke = shortcut_stroke(view, event, keycode, state, group);
    let mut current = Some(view.clone());
    while let Some(view) = current {
        let state = view.state();
        if !state.props.accessibility.state.disabled
            && state.props.key_capture.captures_key(stroke.as_ref())
        {
            return true;
        }
        drop(state);
        current = keyboard_parent(surface, &view);
    }
    false
}

/// Reports a key transition to the focused view (`KeyDown` / `KeyUp`): the
/// engine bubbles it to the view's ancestors, whatever they (or a
/// capture) do with it.
pub(crate) fn notify(surface: &Surface, view: &NshView, kind: EventKind, event: KeyEvent) {
    surface.event(view.id(), kind, EventPayload::Key(event));
}

/// The keyboard layout group of a GDK key event (`0` when there is no
/// event or it is not a key event): `gdk::KeyEvent::layout`.
pub(crate) fn layout(event: Option<&gdk::Event>) -> i32 {
    event
        .and_then(|event| event.downcast_ref::<gdk::KeyEvent>())
        .map_or(0, |event| event.layout() as i32)
}

/// Keeps GDK's private modifier bits (including X11 NumLock), which the
/// gtk-rs flags conversion discards before invoking key callbacks.
pub(crate) fn modifier_state(
    event: Option<&gdk::Event>,
    fallback: gdk::ModifierType,
) -> gdk::ModifierType {
    use glib::translate::ToGlibPtr;

    event.map_or(fallback, |event| {
        // SAFETY: the borrowed event owns this valid GdkEvent pointer for
        // the duration of the call; the getter returns a value, not storage.
        let bits = unsafe { gdk::ffi::gdk_event_get_modifier_state(event.to_glib_none().0) };
        gdk::ModifierType::from_bits_retain(bits)
    })
}

/// The modifier a modifier key stands for.
fn modifier_mask(keyval: gdk::Key) -> Option<gdk::ModifierType> {
    use gdk::ModifierType as M;
    Some(match keyval {
        gdk::Key::Control_L | gdk::Key::Control_R => M::CONTROL_MASK,
        gdk::Key::Shift_L | gdk::Key::Shift_R => M::SHIFT_MASK,
        gdk::Key::Alt_L | gdk::Key::Alt_R | gdk::Key::Meta_L | gdk::Key::Meta_R => M::ALT_MASK,
        gdk::Key::Super_L | gdk::Key::Super_R => M::SUPER_MASK,
        _ => return None,
    })
}

/// Records a modifier key's press or release. Its mask then follows the
/// physical keys held (either Shift, Control, Alt, Super), not the event's
/// pre-key state, which GDK also cannot give after the transition.
pub(crate) fn track_modifiers(surface: &Surface, keyval: gdk::Key, keycode: u32, down: bool) {
    let Some(mask) = modifier_mask(keyval) else {
        return;
    };
    let mut held = surface.keyboard.modifiers.borrow_mut();
    if down {
        held.insert(keycode, mask);
    } else {
        held.remove(&keycode);
    }
}

/// A key event's modifiers: GDK's state (the one before the key) with a
/// modifier key's own mask taken from the physical keys held, so one Shift
/// releasing does not clear the other's. Every other mask is GDK's, which
/// this key does not change.
fn effective_modifiers(
    surface: &Surface,
    state: gdk::ModifierType,
    keyval: gdk::Key,
) -> gdk::ModifierType {
    let Some(mask) = modifier_mask(keyval) else {
        return state;
    };
    let mut state = state;
    let held = surface
        .keyboard
        .modifiers
        .borrow()
        .values()
        .any(|held| held.contains(mask));
    state.set(mask, held);
    state
}

/// A real key came up: clear its hold in `window` (whatever view heard its
/// press) and report `KeyUp` to the view focused now, if any. A held
/// modifier is tracked even when nothing has the focus or the key went to
/// a native widget: the next raw event must carry it.
pub(crate) fn released(
    surface: &Surface,
    window: Option<&gtk::Window>,
    view: Option<&NshView>,
    keyval: gdk::Key,
    keycode: u32,
    state: gdk::ModifierType,
    group: i32,
) {
    track_modifiers(surface, keyval, keycode, false);
    // Only the window the key was pressed in loses its hold (another
    // window's press of the same code is its own); a release reported
    // without a window ends any hold.
    let mut pressed = surface.keyboard.pressed.borrow_mut();
    if let Some(window) = window {
        if pressed
            .get(&keycode)
            .is_some_and(|held| held.upgrade().as_ref() == Some(window))
        {
            pressed.remove(&keycode);
        }
    } else {
        pressed.remove(&keycode);
    }
    drop(pressed);
    let Some(view) = view else {
        return;
    };
    if !listens(surface, view) || native_owns(view) {
        return;
    }
    // The view focused now hears the release, even when the press went to
    // another view; its capture decision is the mounted policy now.
    let mut event = key_event(surface, view, keyval, keycode, state, KeyPhase::Up, group);
    event.captured = captures(surface, view, &event, keycode, state, group);
    notify(surface, view, EventKind::KeyUp, event);
}

/// The transition being reported; releases cannot be auto-repeats.
pub(crate) enum KeyPhase {
    Down { repeat: bool },
    Up,
}

/// The key event of a key press or release. `key` is the actual logical
/// key GDK reports (`"A"` with Shift, `"é"` on a French layout), named by
/// the DOM's spelling for common named keys; `text` is its character when
/// it types one. `physical` keeps GDK's hardware keycode, or is `None`
/// when the source has none.
pub(crate) fn key_event(
    surface: &Surface,
    view: &NshView,
    keyval: gdk::Key,
    keycode: u32,
    state: gdk::ModifierType,
    phase: KeyPhase,
    _group: i32,
) -> KeyEvent {
    let (down, repeat) = match phase {
        KeyPhase::Down { repeat } => (true, repeat),
        KeyPhase::Up => (false, false),
    };
    let modifiers = crate::input::modifiers(effective_modifiers(surface, state, keyval));
    let mut event = KeyEvent::default();
    event.key = logical_key(keyval);
    // A key types its text when nothing makes it a shortcut: Control, Alt
    // and Command do; the named keys type nothing (Space types a space).
    event.text = (down && !modifiers.ctrl && !modifiers.alt && !modifiers.cmd)
        .then(|| text_of(keyval))
        .flatten();
    event.modifiers = modifiers;
    event.physical = (keycode != 0).then_some(PhysicalKey::Native(keycode));
    event.repeat = repeat;
    event
}

/// Whether a key is held down (`gtk_test`).
#[cfg(test)]
pub(crate) fn held_key(surface: &Surface, keycode: u32) -> bool {
    surface.keyboard.pressed.borrow().contains_key(&keycode)
}

/// The canonical shortcut `Keystroke` of a key event: its modifiers, and
/// the keyval `keycode` types with Shift and CapsLock removed on the
/// event's layout group, as `nsh/keymap` names keys. `None` when the
/// layout names no shortcut key there (a dead key, a non-ASCII character).
pub(crate) fn shortcut_stroke(
    view: &NshView,
    event: &KeyEvent,
    keycode: u32,
    state: gdk::ModifierType,
    group: i32,
) -> Option<Keystroke> {
    let display = view.display();
    // Remove character case and Shift, not NumLock or layout-level
    // selectors: an unshifted numeric keypad key is still a digit with
    // NumLock on.
    let unshifted = state.difference(gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::LOCK_MASK);
    let base = shortcut_base(&display, keycode, unshifted, group)?;
    let key = shortcut_key(base)?;
    Some(Keystroke {
        key,
        modifiers: event.modifiers,
    })
}

/// The keyval a shortcut names for `keycode`: the event layout group's
/// key, or, under a non-Latin group, the first other group's key that the
/// shortcut vocabulary names, as GTK's shortcut triggers match a layout's
/// Latin group. Every keyval is the display's own; nothing is invented.
fn shortcut_base(
    display: &gdk::Display,
    keycode: u32,
    state: gdk::ModifierType,
    group: i32,
) -> Option<gdk::Key> {
    let translated = |group| {
        display
            .translate_key(keycode, state, group)
            .map(|(key, ..)| key)
    };
    let base = translated(group)?;
    if shortcut_key(base).is_some_and(|key| !matches!(key, ShortcutKey::Char(_)))
        || base.to_unicode().is_some_and(|c| c.is_ascii_graphic())
    {
        return Some(base);
    }
    // Layouts can hold several groups (`ru`, `us`); GDK reports each
    // group's keyval for the same keycode. A named key (Enter, Escape) is
    // the same in every group and was returned above.
    for other in 0..=8 {
        if other == group {
            continue;
        }
        if let Some(key) = translated(other)
            && key.to_unicode().is_some_and(|c| c.is_ascii_graphic())
        {
            return Some(key);
        }
    }
    Some(base)
}

/// The `nsh/keymap` key of a GDK keyval, or `None` when the keymap's
/// vocabulary does not name it. Letters are the keymap's canonical lower
/// case.
fn shortcut_key(keyval: gdk::Key) -> Option<ShortcutKey> {
    use gdk::Key as K;
    Some(match keyval {
        K::Return | K::KP_Enter | K::ISO_Enter => ShortcutKey::Enter,
        K::Escape => ShortcutKey::Escape,
        K::Tab | K::KP_Tab | K::ISO_Left_Tab => ShortcutKey::Tab,
        K::BackSpace => ShortcutKey::Backspace,
        K::Delete | K::KP_Delete => ShortcutKey::Delete,
        K::Insert | K::KP_Insert => ShortcutKey::Insert,
        K::Up | K::KP_Up => ShortcutKey::Up,
        K::Down | K::KP_Down => ShortcutKey::Down,
        K::Left | K::KP_Left => ShortcutKey::Left,
        K::Right | K::KP_Right => ShortcutKey::Right,
        K::Home | K::KP_Home => ShortcutKey::Home,
        K::End | K::KP_End => ShortcutKey::End,
        K::Page_Up | K::KP_Page_Up => ShortcutKey::PageUp,
        K::Page_Down | K::KP_Page_Down => ShortcutKey::PageDown,
        K::space | K::KP_Space => ShortcutKey::Char(' '),
        K::F1 => ShortcutKey::F(1),
        K::F2 => ShortcutKey::F(2),
        K::F3 => ShortcutKey::F(3),
        K::F4 => ShortcutKey::F(4),
        K::F5 => ShortcutKey::F(5),
        K::F6 => ShortcutKey::F(6),
        K::F7 => ShortcutKey::F(7),
        K::F8 => ShortcutKey::F(8),
        K::F9 => ShortcutKey::F(9),
        K::F10 => ShortcutKey::F(10),
        K::F11 => ShortcutKey::F(11),
        K::F12 => ShortcutKey::F(12),
        K::F13 => ShortcutKey::F(13),
        K::F14 => ShortcutKey::F(14),
        K::F15 => ShortcutKey::F(15),
        K::F16 => ShortcutKey::F(16),
        K::F17 => ShortcutKey::F(17),
        K::F18 => ShortcutKey::F(18),
        K::F19 => ShortcutKey::F(19),
        K::F20 => ShortcutKey::F(20),
        K::F21 => ShortcutKey::F(21),
        K::F22 => ShortcutKey::F(22),
        K::F23 => ShortcutKey::F(23),
        K::F24 => ShortcutKey::F(24),
        _ => {
            let c = keyval.to_unicode().filter(char::is_ascii_graphic)?;
            ShortcutKey::Char(c.to_ascii_lowercase())
        }
    })
}

/// The logical key of a GDK keyval: its actual typed character (Shift's
/// case and the layout's character, not a fabricated US one), a DOM name
/// for the common named keys, and GDK's name for the rest. A keyval with
/// no name is `Unidentified`.
fn logical_key(keyval: gdk::Key) -> SharedString {
    use gdk::Key as K;
    let named = match keyval {
        K::Return | K::KP_Enter | K::ISO_Enter => "Enter",
        K::Escape => "Escape",
        K::Tab | K::KP_Tab | K::ISO_Left_Tab => "Tab",
        K::BackSpace => "Backspace",
        K::space | K::KP_Space => " ",
        K::Delete | K::KP_Delete => "Delete",
        K::Insert | K::KP_Insert => "Insert",
        K::Home | K::KP_Home => "Home",
        K::End | K::KP_End => "End",
        K::Page_Up | K::KP_Page_Up => "PageUp",
        K::Page_Down | K::KP_Page_Down => "PageDown",
        K::Left | K::KP_Left => "ArrowLeft",
        K::Right | K::KP_Right => "ArrowRight",
        K::Up | K::KP_Up => "ArrowUp",
        K::Down | K::KP_Down => "ArrowDown",
        K::Shift_L | K::Shift_R => "Shift",
        K::Control_L | K::Control_R => "Control",
        K::Alt_L | K::Alt_R | K::Meta_L | K::Meta_R => "Alt",
        K::Super_L | K::Super_R => "Meta",
        K::Caps_Lock => "CapsLock",
        K::Num_Lock => "NumLock",
        K::Scroll_Lock => "ScrollLock",
        K::Print => "PrintScreen",
        K::Menu => "ContextMenu",
        K::ISO_Level3_Shift => "AltGraph",
        K::Multi_key => "Compose",
        K::F1 => "F1",
        K::F2 => "F2",
        K::F3 => "F3",
        K::F4 => "F4",
        K::F5 => "F5",
        K::F6 => "F6",
        K::F7 => "F7",
        K::F8 => "F8",
        K::F9 => "F9",
        K::F10 => "F10",
        K::F11 => "F11",
        K::F12 => "F12",
        K::F13 => "F13",
        K::F14 => "F14",
        K::F15 => "F15",
        K::F16 => "F16",
        K::F17 => "F17",
        K::F18 => "F18",
        K::F19 => "F19",
        K::F20 => "F20",
        K::F21 => "F21",
        K::F22 => "F22",
        K::F23 => "F23",
        K::F24 => "F24",
        _ => {
            if let Some(c) = keyval.to_unicode().filter(|c| !c.is_control()) {
                return SharedString::from(c.to_string());
            }
            return match keyval.name() {
                Some(name) if name.as_str() == "VoidSymbol" => SharedString::from("Unidentified"),
                Some(name) if name.as_str().starts_with("dead_") => SharedString::from("Dead"),
                Some(name) => SharedString::from(name.to_string()),
                None => SharedString::from("Unidentified"),
            };
        }
    };
    SharedString::from(named)
}

/// The text the keyval types (the signal's keyval has the layout's level
/// and Shift applied).
fn text_of(keyval: gdk::Key) -> Option<SharedString> {
    let c = keyval.to_unicode()?;
    (!c.is_control()).then(|| SharedString::from(c.to_string()))
}

#[cfg(test)]
mod tests {
    use nsh_core::{MountItem, keymap::Modifiers};

    use super::*;
    use crate::gtk_test::{apply, run, surface, view};

    /// The XTest pair: Shift_L and Shift_R down, Shift_L up (the right one
    /// still holds Shift, so the key up says so), the key typed meanwhile
    /// still has Shift, then Shift_R up (Shift is gone).
    #[test]
    fn both_modifier_keys_own_the_mask_they_share() {
        use gdk::ModifierType as M;

        run(|| {
            let surface = surface();
            apply(
                &surface,
                &[MountItem::Create {
                    id: 2,
                    kind: ViewKind::View,
                }],
            );
            let view = view(&surface, 2);
            let event = |keyval, state, repeat, down| {
                let phase = if down {
                    super::KeyPhase::Down { repeat }
                } else {
                    super::KeyPhase::Up
                };
                super::key_event(&surface, &view, keyval, 0, state, phase, 0)
            };
            super::track_modifiers(&surface, gdk::Key::Shift_L, 50, true);
            assert!(
                event(gdk::Key::Shift_L, M::empty(), false, true)
                    .modifiers
                    .shift
            );
            super::track_modifiers(&surface, gdk::Key::Shift_R, 62, true);
            assert!(
                event(gdk::Key::Shift_R, M::SHIFT_MASK, false, true)
                    .modifiers
                    .shift
            );
            // The left one goes: the right one still holds Shift, and its
            // key up says so.
            super::track_modifiers(&surface, gdk::Key::Shift_L, 50, false);
            assert!(
                event(gdk::Key::Shift_L, M::SHIFT_MASK, false, false)
                    .modifiers
                    .shift
            );
            // The key typed meanwhile still has Shift (`B`).
            let typed = event(gdk::Key::B, M::SHIFT_MASK, false, true);
            assert!(typed.modifiers.shift);
            assert_eq!(typed.key, "B", "the actual shifted character");
            assert_eq!(typed.text.as_deref(), Some("B"));
            // The right one goes too: Shift is gone.
            super::track_modifiers(&surface, gdk::Key::Shift_R, 62, false);
            assert!(
                !event(gdk::Key::Shift_R, M::SHIFT_MASK, false, false)
                    .modifiers
                    .shift
            );
            // Control has its own keys and mask.
            super::track_modifiers(&surface, gdk::Key::Control_L, 37, true);
            assert!(
                event(gdk::Key::Control_L, M::empty(), false, true)
                    .modifiers
                    .ctrl
            );
            super::track_modifiers(&surface, gdk::Key::Control_L, 37, false);
            assert!(
                !event(gdk::Key::Control_L, M::CONTROL_MASK, false, false)
                    .modifiers
                    .ctrl
            );
            // A non-modifier key keeps the masks GDK gave it.
            let held = event(gdk::Key::a, M::SHIFT_MASK | M::CONTROL_MASK, false, true);
            assert!(held.modifiers.shift && held.modifiers.ctrl);
        });
    }

    #[test]
    fn numlock_preserves_numeric_keypad_keys_and_capture() {
        run(|| {
            let surface = surface();
            apply(
                &surface,
                &[MountItem::Create {
                    id: 2,
                    kind: ViewKind::View,
                }],
            );
            let view = view(&surface, 2);
            let display = view.display();
            // X11's NumLock uses Mod2, a private GDK modifier bit which
            // must survive removing Shift and CapsLock.
            if display.type_().name() != "GdkX11Display" {
                return;
            }
            let keycode = display.map_keyval(gdk::Key::KP_1).unwrap()[0].keycode();
            let numlock = gdk::ModifierType::from_bits_retain(1 << 4);
            let numeric = key_event(
                &surface,
                &view,
                gdk::Key::KP_1,
                keycode,
                numlock,
                KeyPhase::Down { repeat: false },
                0,
            );
            assert_eq!(numeric.key, "1");
            assert_eq!(numeric.text.as_deref(), Some("1"));
            assert_eq!(
                numeric.physical,
                Some(nsh_core::PhysicalKey::Native(keycode))
            );
            let capture = nsh_core::KeyCapture::keys([Keystroke::parse_with("1", false).unwrap()]);
            let stroke =
                shortcut_stroke(&view, &numeric, keycode, numlock, 0).expect("a shortcut key");
            assert!(capture.captures(&stroke));
            let navigation = key_event(
                &surface,
                &view,
                gdk::Key::KP_End,
                keycode,
                gdk::ModifierType::empty(),
                KeyPhase::Down { repeat: false },
                0,
            );
            assert_eq!(navigation.key, "End");
            assert_eq!(navigation.text, None);
            let stroke =
                shortcut_stroke(&view, &navigation, keycode, gdk::ModifierType::empty(), 0)
                    .expect("End is a shortcut key");
            assert!(!capture.captures(&stroke));
        });
    }

    /// A key the layout cannot name keeps its source name and reports no
    /// fabricated physical identity; a dead key is the DOM's `Dead`.
    #[test]
    fn an_unmapped_key_keeps_what_the_source_reports() {
        crate::gtk_test::run(|| {
            assert_eq!(
                super::logical_key(gtk4::gdk::Key::VoidSymbol),
                "Unidentified"
            );
            assert_eq!(super::logical_key(gtk4::gdk::Key::dead_acute), "Dead");
            let mut event = KeyEvent::default();
            event.key = super::logical_key(gtk4::gdk::Key::dead_acute);
            assert_eq!(event.key, "Dead");
            assert_eq!(event.physical, None);
            assert_eq!(Modifiers::default(), event.modifiers);
        });
    }
}
