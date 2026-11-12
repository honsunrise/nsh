//! Raw hardware key transitions for focused custom views: `on_key_down` /
//! `on_key_up`, and the capture rules mounted on them
//! (`capture_keys`, `ViewProps::key_capture`).
//!
//! AppKit sends a key to the window's first responder: a field's editor, a
//! hosted view, or one of NSH's views (the root, or a focusable view Tab,
//! `focus()` or a press moved the focus to). This module turns the events
//! of that focused NSH view into the core's [`KeyEvent`] and reports each
//! transition once, at the focused view: the engine bubbles both kinds to
//! its listening ancestors. Nothing waits for JS.
//!
//! One local monitor per surface sees keys before AppKit dispatches them,
//! so the order is:
//!
//! 1. the app's keymap ([`crate::keymap::pressed`]), which keeps its
//!    precedence and consumes a binding;
//! 2. the raw `KeyDown` ([`key_down`]) with the capture rules of the
//!    focused view and its enabled ancestors ([`captures`]); a captured
//!    key is consumed here, before any native action;
//! 3. the application menu's key equivalents and the responder's defaults
//!    (Tab, Return and Space, scrolling, going back), which a captured key
//!    never reaches.
//!
//! A key that is only listened to is not captured: the app hears it and
//! the default still runs. The dev host's Cmd+R is never captured (as the
//! keymap leaves it to the reload monitor).
//!
//! A view that hears none of the raw keys and captures none builds no
//! [`KeyEvent`] and takes the existing path. Fields, hosted views, menus
//! and OS-reserved shortcuts keep their own keys.
//!
//! Direct responder calls (`-[NSView keyDown:]` outside
//! `-[NSApplication sendEvent:]`, as a synthetic event from `synth`) go
//! through the same entry points from [`NshView`]'s overrides; the event
//! is remembered ([`Mark`]) so the monitor's notification is not repeated.
//!
//! Modifier keys change flags, not key values: `flagsChanged` gives the
//! virtual key code of the modifier and the flags after the transition.
//! Each is reported as a named `KeyDown`/`KeyUp` (`Shift`, `Control`,
//! `Alt`, `Meta`, `CapsLock`, `Fn`); [`KeyboardState::modifiers`] keeps
//! the physical keys held, so releasing one Shift while the other is held
//! reports the release, and nothing is reported twice. Caps Lock toggles
//! instead of holding, so its direction is whether the event changed its
//! flag. Losing the app's focus clears what was held, and no release is
//! made up.

use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
    rc::Rc,
};

use block2::RcBlock;
use nsh_core::{
    EventKind, EventPayload, KeyEvent, PhysicalKey, ROOT_ID, element::SharedString,
    platform::InputModality,
};
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSView};
use objc2_foundation::NSString;
use rustc_hash::FxHashSet;

use crate::{surface::Surface, view::NshView};

/// A surface's raw keyboard state.
#[derive(Default)]
pub(crate) struct KeyboardState {
    /// The local monitor of this surface, while it runs.
    monitor: RefCell<Option<Retained<AnyObject>>>,
    /// The modifier keys held, by virtual key code: the next `flagsChanged`
    /// of a code in here is its release. Only modifiers, whose transitions
    /// AppKit reports this way.
    modifiers: RefCell<FxHashSet<u16>>,
    /// The event last reported, so the monitor and a responder's own
    /// `keyDown:` do not report it twice.
    last: Cell<Option<Mark>>,
    /// Whether Caps Lock was on at the last of its `flagsChanged` events:
    /// its key toggles (pressing it again turns it off) where the other
    /// modifier keys hold, so its direction comes from the flag.
    caps: Cell<bool>,
}

impl Drop for KeyboardState {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor.borrow_mut().take() {
            // SAFETY: a monitor `addLocalMonitorForEventsMatchingMask`
            // returned.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    }
}

/// Which event a [`KeyEvent`] was made of: its object, type, number and
/// time. AppKit passes the same `NSEvent` object to the monitor and then
/// down the responder chain; this recognizes that second arrival.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Mark {
    event: usize,
    kind: NSEventType,
    number: isize,
    time: f64,
}

impl Mark {
    fn of(event: &NSEvent) -> Self {
        Mark {
            event: event as *const NSEvent as usize,
            kind: event.r#type(),
            number: event.eventNumber(),
            time: event.timestamp(),
        }
    }
}

/// Starts the surface's key monitor. It is kept until the surface drops.
pub(crate) fn install(surface: &Rc<Surface>) {
    let key = surface.key();
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit passes a valid event.
        let borrowed = unsafe { event.as_ref() };
        match Surface::with_key(key) {
            Some(surface) if press(&surface, borrowed) => std::ptr::null_mut(),
            _ => event.as_ptr(),
        }
    });
    let mask = NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::FlagsChanged;
    // SAFETY: the block returns the event it got, or null (consumed); the
    // monitor is removed with the surface.
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    *surface.keyboard.monitor.borrow_mut() = monitor;
    surface
        .keyboard
        .caps
        .set(NSEvent::modifierFlags_class().contains(NSEventModifierFlags::CapsLock));
}

/// The monitor's decision: whether `event` is consumed.
fn press(surface: &Rc<Surface>, event: &NSEvent) -> bool {
    match event.r#type() {
        NSEventType::KeyDown => {
            // The app's shortcuts keep their precedence: a binding that
            // fires consumes the key before any view or menu sees it.
            if crate::keymap::pressed(surface, event) {
                return true;
            }
            // The dev host's reload keeps the key it has always had, as
            // the keymap leaves it to the reload monitor.
            if cfg!(feature = "dev") && crate::keymap::reloads(event) {
                return false;
            }
            key_down(surface, event)
        }
        NSEventType::KeyUp => key_up(surface, event),
        NSEventType::FlagsChanged => {
            flags_changed(surface, event);
            false
        }
        _ => false,
    }
}

/// A key went down on the focused view: reports its `KeyDown` and returns
/// whether the mounted capture rules consumed it (then no default, menu or
/// text follows). Called by the monitor and by `NshView::keyDown:`.
pub(crate) fn key_down(surface: &Surface, event: &NSEvent) -> bool {
    reconcile(surface, event.modifierFlags(), None);
    let Some(view) = focused(surface, event) else {
        return false;
    };
    if !listens(&view) || seen(surface, event) {
        return false;
    }
    // Before the key acts: the focus it moves shows (`focus-visible:`).
    surface.control(|c| c.set_input_modality(InputModality::Keyboard));
    let mut key = key_event(event, Phase::Down);
    let captured = captures(&view, event);
    key.captured = captured;
    remember(surface, event);
    notify(surface, &view, EventKind::KeyDown, key);
    captured
}

/// A key came up: reports its `KeyUp` to the focused view, with the
/// capture decision of this transition. Returns whether it was captured
/// (nothing follows a release either way).
pub(crate) fn key_up(surface: &Surface, event: &NSEvent) -> bool {
    reconcile(surface, event.modifierFlags(), None);
    let Some(view) = focused(surface, event) else {
        return false;
    };
    if !listens(&view) || seen(surface, event) {
        return false;
    }
    let mut key = key_event(event, Phase::Up);
    let captured = captures(&view, event);
    key.captured = captured;
    remember(surface, event);
    notify(surface, &view, EventKind::KeyUp, key);
    captured
}

/// A modifier key changed: tracks which ones are held and reports the
/// transition as a named `KeyDown` / `KeyUp` on the focused view. Named
/// keys of AppKit have no `keyDown`/`keyUp` of their own.
pub(crate) fn flags_changed(surface: &Surface, event: &NSEvent) {
    // The monitor sees every window of the application: only the surface
    // whose window got the transition tracks it (one tracker per surface,
    // shared by that surface's windows).
    let Some(window) = event.window(surface.mtm) else {
        return;
    };
    if crate::window::root_in(surface, &window).is_none() {
        return;
    }
    // The monitor reports it first; the responder then gets the same event
    // (its transition is already accounted for).
    if seen(surface, event) {
        return;
    }
    let transition = modifier_transition(surface, event);
    remember(surface, event);
    let Some((name, down)) = transition else {
        return;
    };
    let Some(view) = focused(surface, event) else {
        return;
    };
    if !listens(&view) {
        return;
    }
    let mut key = KeyEvent::default();
    key.key = SharedString::new_static(name);
    key.physical = Some(PhysicalKey::Native(u32::from(event.keyCode())));
    key.modifiers = crate::input::modifiers(event.modifierFlags());
    key.captured = captures(&view, event);
    let kind = if down {
        EventKind::KeyDown
    } else {
        EventKind::KeyUp
    };
    notify(surface, &view, kind, key);
}

/// The app stopped, or its surface went away: nothing is held any more
/// and no release is made up.
pub(crate) fn reset(surface: &Surface) {
    surface.keyboard.modifiers.borrow_mut().clear();
    surface.keyboard.last.set(None);
}

/// The app lost the keyboard focus (another app is in front, or it hid):
/// the releases of the keys held may never come, so they are not held any
/// more. No event is made up for them.
pub(crate) fn deactivated(surface: &Surface) {
    reset(surface);
    // Caps Lock's toggle is the system's, not a held key: re-read it.
    surface
        .keyboard
        .caps
        .set(NSEvent::modifierFlags_class().contains(NSEventModifierFlags::CapsLock));
}

/// The NSH view the key is for: the first responder of the event's window
/// when it is one of this surface's focusable views. A field, a hosted
/// view, the root, a blocked window (under a modal) and another surface's
/// views are none.
fn focused(surface: &Surface, event: &NSEvent) -> Option<NshView> {
    let window = event.window(surface.mtm)?;
    let root = crate::window::root_in(surface, &window)?;
    let own = root.ns() != surface.root.ns();
    if crate::window::blocked(surface, own.then_some(&root)) {
        return None;
    }
    let responder = window.firstResponder()?;
    let ns = responder.downcast_ref::<NSView>()?;
    if !ns.isDescendantOf(root.ns()) {
        return None;
    }
    let view = NshView::from_ns(ns)?;
    if view.id() == ROOT_ID {
        return None;
    }
    {
        let state = view.state();
        if !state
            .props
            .focus_stop(state.events.contains(EventKind::Press))
        {
            return None;
        }
    }
    if !view.surface().is_some_and(|s| s.key() == surface.key()) {
        return None;
    }
    Some(view)
}

/// Whether `view` or an ancestor hears a raw key event or captures keys:
/// only then is a [`KeyEvent`] built. A view that hears none takes the
/// existing path.
fn listens(view: &NshView) -> bool {
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

/// Whether the focused view or one of its enabled ancestors captures this
/// transition under the mounted rule ([`KeyCapture`]). A disabled view
/// captures nothing.
///
/// The key is matched as the existing shortcut vocabulary names it
/// ([`crate::keymap::keystroke`]: the virtual key code for named and
/// function keys, the layout's character for the others, the US layout's
/// only where the layout types no ASCII), never by a second parser over
/// the logical name.
fn captures(view: &NshView, event: &NSEvent) -> bool {
    let stroke = crate::keymap::keystroke(event);
    let mut current = Some(view.clone());
    while let Some(view) = current {
        let state = view.state();
        if !state.props.accessibility.state.disabled
            && !state.props.key_capture.is_none()
            && state.props.key_capture.captures_key(stroke.as_ref())
        {
            return true;
        }
        drop(state);
        current = view.parent_view();
    }
    false
}

/// The transition being reported.
enum Phase {
    Down,
    Up,
}

/// The [`KeyEvent`] of a key press or release.
///
/// The logical key is what the source reports: AppKit's named keys in
/// their DOM spellings (`Enter`, `ArrowLeft`, `F5`, `Shift`, ...) and the
/// characters the key types with the modifiers held, case and layout
/// preserved (`"A"` for Shift+A, `"å"` for Option+A on the US layout).
/// The physical key is AppKit's virtual key code, always present (code 0
/// is the ANSI `A`). `text` is a preview of what the press types, not an
/// input method's commit.
fn key_event(event: &NSEvent, phase: Phase) -> KeyEvent {
    let down = matches!(phase, Phase::Down);
    let modifiers = crate::input::modifiers(event.modifierFlags());
    let mut key = KeyEvent::default();
    key.key = key_name(event);
    key.text = (down
        && types_text(event.keyCode())
        && !modifiers.ctrl
        && !modifiers.alt
        && !modifiers.cmd)
        .then(|| printed(event))
        .flatten()
        .map(SharedString::from);
    key.physical = Some(PhysicalKey::Native(u32::from(event.keyCode())));
    key.modifiers = modifiers;
    key.repeat = down && event.isARepeat();
    key
}

/// The logical name of the key.
fn key_name(event: &NSEvent) -> SharedString {
    let code = event.keyCode();
    if let Some(name) = modifier_name(code) {
        return SharedString::new_static(name);
    }
    if let Some(name) = named_name(code) {
        return SharedString::new_static(name);
    }
    if code == SPACE {
        return SharedString::new_static(" ");
    }
    printed(event)
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::new_static("Unidentified"))
}

/// The characters `event` types, keeping case and layout: with the
/// modifiers held (Shift, Option, Caps Lock), without Control's control
/// characters, and without the function keys' private-use code points.
/// `None` for a key that types no character (a dead key before its
/// composition).
fn printed(event: &NSEvent) -> Option<String> {
    if let Some(characters) = printable(event.characters()) {
        return Some(characters);
    }
    // Control turns letters into control characters: the key itself is the
    // one Shift would type.
    let shift = NSEventModifierFlags::Shift;
    let flags = if event.modifierFlags().contains(shift) {
        shift
    } else {
        NSEventModifierFlags::empty()
    };
    if let Some(characters) = event
        .charactersByApplyingModifiers(flags)
        .and_then(|characters| printable(Some(characters)))
    {
        return Some(characters);
    }
    printable(event.charactersIgnoringModifiers())
}

/// `characters` as typed text, or `None` when it is empty, all control
/// characters, or all AppKit private-use code points (the arrow and
/// function keys' characters).
fn printable(characters: Option<Retained<NSString>>) -> Option<String> {
    let text = characters?.to_string();
    let usable = |c: char| !c.is_control() && !('\u{e000}'..='\u{f8ff}').contains(&c);
    (!text.is_empty() && text.chars().any(usable)).then_some(text)
}

/// Whether a key types text: character and space keys, not named keys,
/// function keys or the modifier keys themselves.
fn types_text(code: u16) -> bool {
    modifier_name(code).is_none() && named_name(code).is_none()
}

/// AppKit's virtual key code of the space bar.
const SPACE: u16 = 49;

/// AppKit's virtual key code of Caps Lock (`kVK_CapsLock`).
const CAPS_LOCK: u16 = 57;

/// The app's named keys in their DOM spellings (`KeyboardEvent.key`).
fn named_name(code: u16) -> Option<&'static str> {
    Some(match code {
        36 | 76 => "Enter",
        48 => "Tab",
        51 => "Backspace",
        53 => "Escape",
        117 => "Delete",
        // Help, where PC keyboards have Insert.
        114 => "Help",
        115 => "Home",
        119 => "End",
        116 => "PageUp",
        121 => "PageDown",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => return function_name(code),
    })
}

/// F1 to F20, by virtual key code.
fn function_name(code: u16) -> Option<&'static str> {
    const NAMES: [&str; 20] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
        "F15", "F16", "F17", "F18", "F19", "F20",
    ];
    let n = crate::keymap::function_key(code)?;
    NAMES.get(usize::from(n).checked_sub(1)?).copied()
}

/// The name of a modifier key's virtual key code: the DOM's spelling of
/// each (`Alt` is Option, `Meta` is Command).
fn modifier_name(code: u16) -> Option<&'static str> {
    Some(match code {
        54 | 55 => "Meta",
        56 | 60 => "Shift",
        57 => "CapsLock",
        58 | 61 => "Alt",
        59 | 62 => "Control",
        63 => "Fn",
        _ => return None,
    })
}

/// Records `event`'s modifier transition and returns the key's name and
/// whether it went down: a code not held is a press, a held one its
/// release. AppKit reports the modifier keys only this way.
fn modifier_transition(surface: &Surface, event: &NSEvent) -> Option<(&'static str, bool)> {
    let code = event.keyCode();
    let name = modifier_name(code)?;
    let flags = event.modifierFlags();
    // A modifier released while a native menu owned the events (local
    // monitors skip nested loops) is no longer held: the flags say so.
    reconcile(surface, flags, Some(code));
    let down = if code == CAPS_LOCK {
        // The key toggles: the event that changes the flag is its press,
        // and one that does not is its release.
        let on = flags.contains(NSEventModifierFlags::CapsLock);
        on != surface.keyboard.caps.replace(on)
    } else {
        let mut held = surface.keyboard.modifiers.borrow_mut();
        if held.remove(&code) {
            false
        } else {
            held.insert(code);
            true
        }
    };
    Some((name, down))
}

/// The modifier flag a held key stands for (`None` for Caps Lock, which
/// toggles instead of holding).
fn modifier_flag(code: u16) -> Option<NSEventModifierFlags> {
    Some(match code {
        54 | 55 => NSEventModifierFlags::Command,
        56 | 60 => NSEventModifierFlags::Shift,
        58 | 61 => NSEventModifierFlags::Option,
        59 | 62 => NSEventModifierFlags::Control,
        63 => NSEventModifierFlags::Function,
        _ => return None,
    })
}

/// Drops the modifier keys held whose flag `flags` no longer has, except
/// `except` (the key of the transition being read: a release's flag is
/// gone, and it still has to be seen as a release).
fn reconcile(surface: &Surface, flags: NSEventModifierFlags, except: Option<u16>) {
    let mut held = surface.keyboard.modifiers.borrow_mut();
    held.retain(|&code| {
        except == Some(code) || modifier_flag(code).is_none_or(|flag| flags.contains(flag))
    });
}

/// Reports `key` at `view` (the engine bubbles it to the ancestors) as
/// `kind`.
fn notify(surface: &Surface, view: &NshView, kind: EventKind, key: KeyEvent) {
    let id = view.id();
    if !key.captured {
        log::debug!(
            target: "nsh::keyboard",
            "{} {} on {id} (physical {:?})",
            kind.prop_name(),
            key.key,
            key.physical
        );
    }
    surface.event(id, kind, EventPayload::Key(key));
}

/// Whether this event was already reported (the monitor reported it, and
/// AppKit then handed it to the responder too).
fn seen(surface: &Surface, event: &NSEvent) -> bool {
    surface.keyboard.last.get() == Some(Mark::of(event))
}

fn remember(surface: &Surface, event: &NSEvent) {
    surface.keyboard.last.set(Some(Mark::of(event)));
}
