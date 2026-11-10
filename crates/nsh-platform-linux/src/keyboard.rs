//! The keys of a focused view: the `on_key_down` / `on_key_up` events a
//! view and its ancestors hear, and the capture rules mounted on them.
//!
//! The focus and the keys themselves are [`crate::key_focus`]'s: its one
//! key controller per window decides, in GTK's capture phase, whether the
//! key is the app's ([`crate::keymap::claims`]), captured by the focused
//! view (`capture_keys`), or a default (Tab's traversal, Return and Space
//! pressing, a context menu, scrolling). This module only does what that
//! decision needs: it builds the [`KeyEvent`], matches the focus's capture
//! rules ([`captures`]), reports the event to the engine as the plain
//! `KeyDown` / `KeyUp` events ([`notify`], [`released`]), and tracks the
//! keys and modifiers held so releases reach their original target. No
//! engine answer is waited for: what a listener does is reported
//! afterwards, in the app's frame.
//!
//! A captured key still notifies the focused view and its ancestors (the
//! engine bubbles it), with `KeyEvent::captured` set; it runs no default
//! (no traversal, activation or context menu). A key that only *listens*
//! is not captured: the app hears it and the default still runs.
//!
//! A view that hears none of the raw keys and captures none takes
//! [`crate::key_focus`]'s original path: [`listens`] is a few mask
//! checks, and no [`KeyEvent`], no key state and no allocation is made for
//! its keys.

use std::cell::RefCell;

use gtk4::{self as gtk, gdk, glib, prelude::*};
use nsh_core::{EventKind, EventPayload, Key, KeyEvent, NamedKey};
use rustc_hash::FxHashMap;

use crate::{surface::Surface, view::NshView};

/// A surface's keyboard state.
#[derive(Default)]
pub(crate) struct Keyboard {
    /// The hardware keycodes held down, and the window each was pressed
    /// in, for auto-repeat detection (GDK does not mark repeats).
    pressed: RefCell<FxHashMap<u32, glib::WeakRef<gtk::Window>>>,
    /// The view each held key went down on, its key up returning there,
    /// and whether the press was captured (the key up says the same).
    down_targets: RefCell<FxHashMap<u32, (glib::WeakRef<NshView>, bool)>>,
    /// The physical modifier keys held (by keycode) and the modifier each
    /// stands for: a key event's modifier mask comes from these, not from
    /// its pre-key state, so releasing one Shift keeps the other's.
    modifiers: RefCell<FxHashMap<u32, gdk::ModifierType>>,
}

/// The app stopped: forgets the keys held and the views they went down on.
pub(crate) fn reset(surface: &Surface) {
    surface.keyboard.pressed.borrow_mut().clear();
    surface.keyboard.modifiers.borrow_mut().clear();
    surface.keyboard.down_targets.borrow_mut().clear();
}

/// `window` lost the OS focus (`is-active` off): its releases may never
/// come, so the keys held there are no press of a key any more and the down
/// targets of its views are forgotten (another window's stay).
pub(crate) fn window_deactivated(surface: &Surface, window: &gtk::Window) {
    surface
        .keyboard
        .pressed
        .borrow_mut()
        .retain(|_, held| held.upgrade().as_ref() != Some(window));
    // Their releases may never come: a held modifier is no more held.
    surface.keyboard.modifiers.borrow_mut().clear();
    surface
        .keyboard
        .down_targets
        .borrow_mut()
        .retain(|_, (weak, _)| {
            weak.upgrade().is_some_and(|view| {
                view.root().and_downcast::<gtk::Window>().as_ref() != Some(window)
            })
        });
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

/// Whether `view` or an ancestor hears a raw key event, or captures keys:
/// only then does a key need a [`KeyEvent`], its capture rules and the
/// per-key state. A view that hears none takes [`crate::key_focus`]'s
/// original path, with nothing built.
pub(crate) fn listens(view: &NshView) -> bool {
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
        current = view.parent_view();
    }
    false
}

/// Whether the focused view or one of its enabled ancestors captures the
/// key (`capture_keys`, `ViewProps::key_capture`): a captured key
/// notifies and runs no default (Tab's traversal, Return and Space
/// pressing, a context menu, scrolling).
pub(crate) fn captures(view: &NshView, event: &KeyEvent) -> bool {
    let mut current = Some(view.clone());
    while let Some(view) = current {
        let state = view.state();
        if !state.props.accessibility.state.disabled && state.props.key_capture.captures(event) {
            return true;
        }
        drop(state);
        current = view.parent_view();
    }
    false
}

/// Reports a key press to the focused view (`KeyDown`): the engine bubbles
/// it to the view's ancestors, whatever they (or a capture) do with it.
pub(crate) fn notify(surface: &Surface, view: &NshView, event: KeyEvent) {
    // The press itself keeps the first target: an auto-repeat goes down
    // again on whatever has the focus, but its key up returns to the view
    // the key went down on, as documented.
    surface
        .keyboard
        .down_targets
        .borrow_mut()
        .entry(event.physical)
        .or_insert((view.downgrade(), event.captured));
    surface.event(view.id(), EventKind::KeyDown, EventPayload::Key(event));
}

/// The keyboard layout group of a GDK key event (`0` when there is no
/// event or it is not a key event): `gdk::KeyEvent::layout`.
pub(crate) fn layout(event: Option<&gdk::Event>) -> i32 {
    event
        .and_then(|event| event.downcast_ref::<gdk::KeyEvent>())
        .map_or(0, |event| event.layout() as i32)
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

/// A key came up: tell the view the key went down on (that one heard the
/// down, whatever has the focus now).
pub(crate) fn released(
    surface: &Surface,
    window: Option<&gtk::Window>,
    keyval: gdk::Key,
    keycode: u32,
    state: gdk::ModifierType,
    group: i32,
) {
    // A modifier's release is tracked even when nothing has the focus or the
    // key went to a native widget: the next raw event must carry it.
    track_modifiers(surface, keyval, keycode, false);
    // Only the window the key was pressed in loses its hold (another
    // window's press of the same code is its own).
    let held_here = window.is_none_or(|window| {
        surface
            .keyboard
            .pressed
            .borrow()
            .get(&keycode)
            .is_some_and(|held| held.upgrade().as_ref() == Some(window))
    });
    if held_here {
        surface.keyboard.pressed.borrow_mut().remove(&keycode);
    }
    let Some((weak, captured)) = surface.keyboard.down_targets.borrow_mut().remove(&keycode) else {
        return;
    };
    // The raw key up returns to the view the key went down on, if it is
    // still alive; its key may no longer be mapped (a layout change, a dead
    // key), so the keycode held down is enough for an `Unidentified` key up.
    if let Some(view) = weak.upgrade() {
        let mut key = key_event(surface, &view, keyval, keycode, state, KeyPhase::Up, group);
        key.captured = captured;
        surface.event(view.id(), EventKind::KeyUp, EventPayload::Key(key));
    }
}

/// The transition being reported; releases cannot be auto-repeats.
pub(crate) enum KeyPhase {
    Down { repeat: bool },
    Up,
}

/// The key event of a key press or release. A key the layout cannot name
/// (a dead key, a code with no keymap entry) is
/// [`NamedKey::Unidentified`], with its hardware code kept: a capture that
/// takes every key takes it too, and it still reaches the view.
pub(crate) fn key_event(
    surface: &Surface,
    view: &NshView,
    keyval: gdk::Key,
    keycode: u32,
    state: gdk::ModifierType,
    phase: KeyPhase,
    group: i32,
) -> KeyEvent {
    let (down, repeat) = match phase {
        KeyPhase::Down { repeat } => (true, repeat),
        KeyPhase::Up => (false, false),
    };
    let display = view.display();
    // The layout's unshifted character names the key (`group` is the
    // event's active keyboard layout); Shift only changes the text
    // (`Shift+A` is `Character("a")` typing `"A"`).
    let base = display
        .translate_key(keycode, state, group)
        .and_then(|(_, group, _, _)| {
            display.translate_key(keycode, gdk::ModifierType::empty(), group)
        })
        .map(|(base, ..)| base);
    let key = base.map_or(Key::Named(NamedKey::Unidentified), key_of);
    let modifiers = crate::input::modifiers(effective_modifiers(surface, state, keyval));
    // A key types its text when nothing changes the character: Control, Alt
    // and Command make shortcuts, and the named keys type nothing (Space,
    // named, types a space).
    let types = matches!(key, Key::Character(_)) || key == Key::Named(NamedKey::Space);
    let text = (down && types && !modifiers.ctrl && !modifiers.alt && !modifiers.cmd)
        .then(|| text_of(keyval))
        .flatten();
    let mut event = KeyEvent::default();
    event.key = key;
    event.text = text;
    event.modifiers = modifiers;
    event.physical = keycode;
    event.repeat = repeat;
    event
}

/// Whether a key is held down (`gtk_test`).
#[cfg(test)]
pub(crate) fn held_key(surface: &Surface, keycode: u32) -> bool {
    surface.keyboard.pressed.borrow().contains_key(&keycode)
}

/// The view a held key went down on (`gtk_test`).
#[cfg(test)]
pub(crate) fn down_target(surface: &Surface, keycode: u32) -> Option<nsh_core::NodeId> {
    surface
        .keyboard
        .down_targets
        .borrow()
        .get(&keycode)
        .and_then(|(weak, _)| weak.upgrade())
        .map(|view| view.id())
}

/// The text the keyval types (the signal's keyval has the layout's level
/// and Shift applied).
fn text_of(keyval: gdk::Key) -> Option<nsh_core::element::SharedString> {
    let c = keyval.to_unicode()?;
    (!c.is_control()).then(|| nsh_core::element::SharedString::from(c.to_string()))
}

/// The key of a keyval: named keys by name, everything else by the
/// character it types (lower case for letters); a keyval that types no
/// character (a dead key, `VoidSymbol`) is [`NamedKey::Unidentified`].
fn key_of(keyval: gdk::Key) -> Key {
    use gdk::Key as K;
    let named = match keyval {
        K::Return | K::KP_Enter | K::ISO_Enter => NamedKey::Enter,
        K::Tab | K::KP_Tab | K::ISO_Left_Tab => NamedKey::Tab,
        K::BackSpace => NamedKey::Backspace,
        K::Escape => NamedKey::Escape,
        K::space | K::KP_Space => NamedKey::Space,
        K::Delete | K::KP_Delete => NamedKey::Delete,
        K::Insert | K::KP_Insert => NamedKey::Insert,
        K::Home | K::KP_Home => NamedKey::Home,
        K::End | K::KP_End => NamedKey::End,
        K::Page_Up | K::KP_Page_Up => NamedKey::PageUp,
        K::Page_Down | K::KP_Page_Down => NamedKey::PageDown,
        K::Left | K::KP_Left => NamedKey::ArrowLeft,
        K::Right | K::KP_Right => NamedKey::ArrowRight,
        K::Up | K::KP_Up => NamedKey::ArrowUp,
        K::Down | K::KP_Down => NamedKey::ArrowDown,
        K::Shift_L | K::Shift_R => NamedKey::Shift,
        K::Control_L | K::Control_R => NamedKey::Control,
        K::Alt_L | K::Alt_R | K::Meta_L | K::Meta_R => NamedKey::Alt,
        K::Super_L | K::Super_R => NamedKey::Meta,
        K::F1 => NamedKey::F(1),
        K::F2 => NamedKey::F(2),
        K::F3 => NamedKey::F(3),
        K::F4 => NamedKey::F(4),
        K::F5 => NamedKey::F(5),
        K::F6 => NamedKey::F(6),
        K::F7 => NamedKey::F(7),
        K::F8 => NamedKey::F(8),
        K::F9 => NamedKey::F(9),
        K::F10 => NamedKey::F(10),
        K::F11 => NamedKey::F(11),
        K::F12 => NamedKey::F(12),
        K::F13 => NamedKey::F(13),
        K::F14 => NamedKey::F(14),
        K::F15 => NamedKey::F(15),
        K::F16 => NamedKey::F(16),
        K::F17 => NamedKey::F(17),
        K::F18 => NamedKey::F(18),
        K::F19 => NamedKey::F(19),
        K::F20 => NamedKey::F(20),
        K::F21 => NamedKey::F(21),
        K::F22 => NamedKey::F(22),
        K::F23 => NamedKey::F(23),
        K::F24 => NamedKey::F(24),
        _ => {
            let Some(c) = keyval.to_unicode().filter(|c| !c.is_control()) else {
                return Key::Named(NamedKey::Unidentified);
            };
            let c = if c.is_uppercase() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                c
            };
            return Key::Character(nsh_core::element::SharedString::from(c.to_string()));
        }
    };
    Key::Named(named)
}

#[cfg(test)]
mod tests {
    use nsh_core::{MountItem, ViewKind};

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
            assert!(
                event(gdk::Key::b, M::SHIFT_MASK, false, true)
                    .modifiers
                    .shift
            );
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

    /// A key the layout cannot name is `Unidentified`, not dropped: a
    /// capture that takes every key takes it, and it still reaches the view.
    #[test]
    fn an_unmapped_key_is_unidentified() {
        crate::gtk_test::run(|| {
            assert!(matches!(
                super::key_of(gtk4::gdk::Key::dead_acute),
                nsh_core::Key::Named(nsh_core::NamedKey::Unidentified)
            ));
            assert!(matches!(
                super::key_of(gtk4::gdk::Key::VoidSymbol),
                nsh_core::Key::Named(nsh_core::NamedKey::Unidentified)
            ));
        });
    }
}
