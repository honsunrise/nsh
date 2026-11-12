//! Keyboard focus for custom views, and the hardware key events they hear.
//!
//! Windows gives the keyboard to a text field NSH hosts or to a web view's
//! window; nothing else in a surface could take it. This module adds the
//! third case: a mounted view that is a focus stop
//! ([`nsh_core::ViewProps::focus_stop`]: `focusable`, or a view that takes
//! presses) takes the keyboard focus with Tab, a click, `ref.focus()` or
//! UI Automation. While it has the focus, the window's `WM_KEYDOWN` /
//! `WM_KEYUP` messages become [`nsh_core::KeyEvent`]s
//! ([`crate::surface::Surface::event`]): the engine bubbles both to the
//! view's listening ancestors within its native window.
//!
//! The logical key and its text come from the current keyboard layout
//! (`ToUnicodeEx`), never from a US layout guess; the physical key is the
//! message's scan code with its extended bit. The keys are the same
//! shortcut keys [`crate::keymap`] matches (virtual keys, US letters under
//! non-Latin layouts), so a captured key is one the app can also bind.
//!
//! Capture rules ([`nsh_core::KeyCapture`]) are read from the mounted
//! props of the focused view and its enabled ancestors — walking
//! [`nsh_core::ViewProps::key_parent`] when core set one for a view a
//! native surface portals at a root it does not logically live under —
//! before the native defaults of [`crate::input::key`] (the application menu, a focused
//! field's editing, navigation, scrolling, Tab's traversal). A captured
//! key is still reported, with `captured` set, and runs none of those
//! defaults. A captured key is never translated into characters:
//! [`suppress_translation`] runs from the thread's `WH_GETMESSAGE` hook
//! (and the standalone loop) before `TranslateMessage`, and turns the key
//! down into a private message the window procedure handles as the key
//! down it was. Nothing of the message queue is inspected, moved or
//! restamped, so a character queued around the key keeps its place and
//! its delivery.
//!
//! Focus is per native window (the main window's root, or the root of a
//! window of the app's own): a window in the background keeps its view, and
//! the keys of the window in front only reach its own. A modal presented
//! over a window saves the custom view with the focus, as it saves a
//! text field, and gives it back when it closes if that view is still a
//! shown, enabled stop. A view that is removed, concealed or disabled
//! loses the focus without a synthetic key release; releases are reported
//! when the source sends them, on whatever view has the focus then, and
//! repeats are what the message's previous-key-state bit says.

use std::cell::RefCell;

use nsh_core::{
    EventKind, EventPayload, NodeId, PhysicalKey, PointerEvents, ViewKind,
    element::SharedString,
    keymap::{Key, Keystroke},
};
use rustc_hash::FxHashMap;
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        GetFocus, GetKeyState, GetKeyboardLayout, GetKeyboardState, HKL, MAPVK_VK_TO_CHAR,
        MapVirtualKeyExW, SetFocus, ToUnicodeEx, VK_NUMLOCK,
    },
    WindowsAndMessaging::{MSG, WM_KEYDOWN, WM_SYSKEYDOWN},
};
#[cfg(test)]
use windows::Win32::UI::Input::KeyboardAndMouse::SetKeyboardState;

use crate::{app_window::window_of, input::KeyEvent, surface::Surface, view::NshView};

/// `ToUnicodeEx`'s "keyboard state is not changed" flag (Windows 10
/// 1607+): translating a key for an event must not store the dead key,
/// ligature or Alt+numpad state it reads.
const NO_STATE_CHANGE: u32 = 0x4;

/// A surface's keyboard focus: the view with it in each of its windows.
#[derive(Default)]
pub(crate) struct Keyboard {
    /// The focused custom view of each native window, by the window's root
    /// node ([`ROOT_ID`](nsh_core::ROOT_ID) for the main window).
    focus: RefCell<FxHashMap<NodeId, NshView>>,
}

/// The app stopped: its views are gone with it.
pub(crate) fn reset(surface: &Surface) {
    surface.keyboard.focus.borrow_mut().clear();
}

/// Whether Tab stops at `view`: it takes presses or is `focusable`, and
/// the pointer could reach it. A text field is a stop of its own
/// ([`crate::text_input`]) and not one of these.
pub(crate) fn is_stop(view: &NshView) -> bool {
    {
        let state = view.state();
        if state.kind == ViewKind::Input
            || !state
                .props
                .focus_stop(state.events.contains(EventKind::Press))
        {
            return false;
        }
    }
    targetable(view)
}

/// Whether the pointer may reach `view` (it and its ancestors are shown,
/// targetable and not leaving).
fn targetable(view: &NshView) -> bool {
    let mut current = Some(view.clone());
    while let Some(view) = current {
        {
            let state = view.state();
            if state.style.hidden
                || state.concealed
                || state.props.pointer_events == PointerEvents::None
                || state.live.as_ref().is_some_and(|live| live.dying())
            {
                return false;
            }
        }
        current = view.parent_view();
    }
    view.attached()
}

/// Whether `view` is the root of a window of the app's own: the keyboard
/// boundary its ancestors do not cross.
fn window_root(surface: &Surface, view: &NshView) -> bool {
    view.kind() == ViewKind::Modal && crate::app_window::is_window(surface, view.id())
}

/// Walks `view` and its keyboard ancestry within its native window, with
/// `f` deciding; true when `f` does.
///
/// The next ancestor is [`nsh_core::ViewProps::key_parent`] when core set
/// one — a view a native surface portals at a root it does not logically
/// live under (an `Overlay`): the mounted parent there is not proof of
/// logical ancestry — and the mounted parent otherwise. The walk ends
/// after the root of a window of the app's own: keys do not cross native
/// windows. Ordinary focus, hit testing and layout keep their mounted
/// parents; only this interest/capture walk follows the override.
fn walk_keyboard(surface: &Surface, view: &NshView, mut f: impl FnMut(&NshView) -> bool) -> bool {
    let window = window_of(surface, view);
    let mut current = Some(view.clone());
    // Bounded: a routing override is core's, but a broken one must not hang
    // the window.
    for _ in 0..1024 {
        let Some(view) = current else {
            return false;
        };
        if f(&view) {
            return true;
        }
        if window_root(surface, &view) {
            return false;
        }
        let logical = view
            .state()
            .props
            .key_parent
            .and_then(|id| surface.view(id))
            // A native window is a keyboard boundary even across a
            // routing override.
            .filter(|parent| window_of(surface, parent) == window);
        current = logical.or_else(|| view.parent_view());
    }
    false
}

/// Whether `view` or a keyboard ancestor within its window hears
/// `KeyDown` / `KeyUp` or captures keys. A view that does neither takes
/// the original path of [`crate::input::key`], with no event built.
pub(crate) fn listens(surface: &Surface, view: &NshView) -> bool {
    walk_keyboard(surface, view, |view| {
        let state = view.state();
        state.events.contains(EventKind::KeyDown)
            || state.events.contains(EventKind::KeyUp)
            || !state.props.key_capture.is_none()
    })
}

/// Whether the focused `view` or one of its enabled keyboard ancestors
/// captures the key of `stroke` (`capture_keys`;
/// [`nsh_core::KeyCapture`]). Disabled views and the views outside the
/// window capture nothing.
pub(crate) fn captures(surface: &Surface, view: &NshView, stroke: Option<&Keystroke>) -> bool {
    walk_keyboard(surface, view, |view| {
        let state = view.state();
        !state.props.accessibility.state.disabled && state.props.key_capture.captures_key(stroke)
    })
}

/// The focused custom view of window `window`, if it is still a stop
/// (a view that lost `focusable`, was hidden or was removed loses the
/// focus quietly).
pub(crate) fn focused_in(surface: &Surface, window: NodeId) -> Option<NshView> {
    let view = surface.keyboard.focus.borrow().get(&window).cloned()?;
    if !valid(surface, &view) || window_of(surface, &view) != window {
        clear(surface, window);
        return None;
    }
    Some(view)
}

/// The focused custom view of the input window (or of any window when that
/// one has none): UI Automation's focus, per window.
pub(crate) fn focused_any(surface: &Surface) -> Option<NshView> {
    let windows: Vec<NodeId> = surface.keyboard.focus.borrow().keys().copied().collect();
    let input = surface.input_root().id();
    if windows.contains(&input)
        && let Some(view) = focused_in(surface, input)
    {
        return Some(view);
    }
    for window in windows {
        if let Some(view) = focused_in(surface, window) {
            return Some(view);
        }
    }
    None
}

/// Whether the focus held for `view` is still valid: a stop, and not
/// given to a text field of the same window (a field and a custom view
/// never both have the keyboard of one window).
fn valid(surface: &Surface, view: &NshView) -> bool {
    if !is_stop(view) {
        return false;
    }
    let Some(field) = crate::text_input::focused(surface) else {
        return true;
    };
    window_of(surface, &field) != window_of(surface, view)
}

/// `ref.focus()` on a view that is not a field: the keyboard focus moves
/// to it if it is a stop, as Tab moves it there. False when it is not.
pub(crate) fn focus_control(surface: &std::rc::Rc<Surface>, view: &NshView) -> bool {
    if view.kind() == ViewKind::Input || view.id() == nsh_core::ROOT_ID || !is_stop(view) {
        return false;
    }
    focus_view(surface, view, true);
    true
}

/// A press at `view`: the focus stop it belongs to is the view itself or
/// its nearest focusable ancestor within the same window.
pub(crate) fn focus_target(surface: &Surface, view: &NshView) -> Option<NshView> {
    let window = window_of(surface, view);
    let mut current = Some(view.clone());
    while let Some(view) = current {
        if is_stop(&view) {
            return Some(view);
        }
        if window_of(surface, &view) != window {
            return None;
        }
        current = view.parent_view();
    }
    None
}

/// A press focused `view`: as [`focus_control`], without scrolling it into
/// view (the pointer is on it).
pub(crate) fn focus_pointer(surface: &std::rc::Rc<Surface>, view: &NshView) {
    if is_stop(view) {
        focus_view(surface, view, false);
    }
}

/// Moves the keyboard focus to `view` in its window, reporting the blur of
/// the view that had it and the focus of the new one.
fn focus_view(surface: &std::rc::Rc<Surface>, view: &NshView, reveal: bool) {
    let window = window_of(surface, view);
    if surface
        .keyboard
        .focus
        .borrow()
        .get(&window)
        .is_some_and(|held| held == view)
    {
        return;
    }
    // A field of this window that edits leaves the focus to the custom
    // view, as Tab does; another window's field keeps its own.
    if let Some(field) = crate::text_input::focused(surface)
        && window_of(surface, &field) == window
    {
        crate::text_input::blur(&field);
    }
    clear(surface, window);
    take_keyboard(surface, view);
    surface
        .keyboard
        .focus
        .borrow_mut()
        .insert(window, view.clone());
    crate::input::set_state(surface, view, nsh_core::tailwind::state::FOCUSED, true);
    surface.event(view.id(), EventKind::Focus, EventPayload::None);
    // Focused stops report it to assistive technologies, as fields do.
    crate::access::focus_changed(surface, view);
    if reveal {
        crate::scroll::reveal(surface, view);
    }
    view.changed();
}

/// Gives the window showing `view` the keyboard focus, so the next
/// `WM_KEY*` message comes to it rather than to a child a page took it
/// from.
fn take_keyboard(surface: &Surface, view: &NshView) {
    let hwnd = crate::app_window::hwnd_showing(surface, view);
    if hwnd.is_invalid() {
        return;
    }
    // SAFETY: plain calls on a window of this thread.
    unsafe {
        if GetFocus() != hwnd {
            let _ = SetFocus(Some(hwnd));
        }
    }
}

/// `ref.blur()` on a custom view that has the focus. False when it does
/// not.
pub(crate) fn blur(surface: &Surface, view: &NshView) -> bool {
    let window = window_of(surface, view);
    let holds = surface
        .keyboard
        .focus
        .borrow()
        .get(&window)
        .is_some_and(|held| held == view);
    if !holds {
        return false;
    }
    surface.keyboard.focus.borrow_mut().remove(&window);
    blur_view(surface, view);
    true
}

/// Blurs the focused custom view of window `window` (a modal is presented
/// over it).
pub(crate) fn blur_window(surface: &Surface, window: NodeId) {
    clear(surface, window);
}

/// Blurs the focus held inside `ancestor` (it is being concealed).
pub(crate) fn blur_within(surface: &Surface, ancestor: &NshView) {
    let affected: Vec<(NodeId, NshView)> = {
        let focus = surface.keyboard.focus.borrow();
        focus
            .iter()
            .filter(|(_, view)| within(view, ancestor))
            .map(|(window, view)| (*window, view.clone()))
            .collect()
    };
    for (window, view) in affected {
        surface.keyboard.focus.borrow_mut().remove(&window);
        blur_view(surface, &view);
    }
}

/// `view` (or one of its views) is gone: forget the focus without an
/// event (a removed view has no one to tell).
pub(crate) fn forget(surface: &Surface, view: &NshView) {
    surface
        .keyboard
        .focus
        .borrow_mut()
        .retain(|_, focused| &*focused != view && !within(focused, view));
}

/// The window showing dialog root `window` was destroyed.
pub(crate) fn forget_window(surface: &Surface, window: NodeId) {
    surface.keyboard.focus.borrow_mut().remove(&window);
}

/// Whether `view` is `ancestor` or inside it.
fn within(view: &NshView, ancestor: &NshView) -> bool {
    let mut current = Some(view.clone());
    while let Some(view) = current {
        if view == *ancestor {
            return true;
        }
        current = view.parent_view();
    }
    false
}

/// Reports the blur of `view`: its `focus:` state goes off and a `Blur`
/// event follows.
fn blur_view(surface: &Surface, view: &NshView) {
    crate::input::set_state(surface, view, nsh_core::tailwind::state::FOCUSED, false);
    surface.event(view.id(), EventKind::Blur, EventPayload::None);
    view.changed();
}

/// Clears the focus held in `window`, reporting the blur.
fn clear(surface: &Surface, window: NodeId) {
    let view = surface.keyboard.focus.borrow_mut().remove(&window);
    if let Some(view) = view {
        blur_view(surface, &view);
    }
}

/// The key a key message went down or up on, as the layout names it: a
/// named key by its DOM spelling (`Enter`, `ArrowLeft`, `F5`), a
/// character by the character it types (its actual case, with Shift and
/// Caps Lock), and `Unidentified` for a key the layout cannot name (a dead
/// key, `VK_PROCESSKEY`).
fn key_name(event: &KeyEvent) -> SharedString {
    // With Num Lock off, the keypad's keys are the navigation keys they
    // are labeled with, as they are to the system (`WM_CHAR` comes for
    // none of them).
    if !num_lock()
        && let Some(name) = keypad_navigation(event.vk)
    {
        return SharedString::new_static(name);
    }
    let named = match event.vk {
        0x03 => Some("Cancel"),
        0x08 => Some("Backspace"),
        0x09 => Some("Tab"),
        0x0C => Some("Clear"),
        0x0D => Some("Enter"),
        0x13 => Some("Pause"),
        0x14 => Some("CapsLock"),
        0x1B => Some("Escape"),
        0x20 => Some(" "),
        0x21 => Some("PageUp"),
        0x22 => Some("PageDown"),
        0x23 => Some("End"),
        0x24 => Some("Home"),
        0x25 => Some("ArrowLeft"),
        0x26 => Some("ArrowUp"),
        0x27 => Some("ArrowRight"),
        0x28 => Some("ArrowDown"),
        0x29 => Some("Select"),
        0x2A => Some("Print"),
        0x2B => Some("Execute"),
        0x2C => Some("PrintScreen"),
        0x2D => Some("Insert"),
        0x2E => Some("Delete"),
        0x2F => Some("Help"),
        0x5D => Some("ContextMenu"),
        0x5F => Some("Sleep"),
        0x6A => Some("Multiply"),
        0x6B => Some("Add"),
        0x6C => Some("Separator"),
        0x6D => Some("Subtract"),
        0x6E => Some("Decimal"),
        0x6F => Some("Divide"),
        // The Windows, Command/Super key.
        0x5B | 0x5C => Some("Meta"),
        // The modifier keys themselves (the left and right ones too).
        0x10 | 0xA0 | 0xA1 => Some("Shift"),
        0x11 | 0xA2 | 0xA3 => Some("Control"),
        0x12 | 0xA4 | 0xA5 => Some("Alt"),
        0x90 => Some("NumLock"),
        0x91 => Some("ScrollLock"),
        0xA6 => Some("BrowserBack"),
        0xA7 => Some("BrowserForward"),
        0xA8 => Some("BrowserRefresh"),
        0xA9 => Some("BrowserStop"),
        0xAA => Some("BrowserSearch"),
        0xAB => Some("BrowserFavorites"),
        0xAC => Some("BrowserHome"),
        0xAD => Some("AudioVolumeMute"),
        0xAE => Some("AudioVolumeDown"),
        0xAF => Some("AudioVolumeUp"),
        0xB0 => Some("MediaTrackNext"),
        0xB1 => Some("MediaTrackPrevious"),
        0xB2 => Some("MediaStop"),
        0xB3 => Some("MediaPlayPause"),
        0xB4 => Some("LaunchMail"),
        0xB5 => Some("LaunchMediaPlayer"),
        0xB6 => Some("LaunchApplication1"),
        0xB7 => Some("LaunchApplication2"),
        _ => None,
    };
    if let Some(named) = named {
        return SharedString::new_static(named);
    }
    if (0x70..=0x87).contains(&event.vk) {
        let n = event.vk - 0x6F;
        return SharedString::from(format!("F{n}"));
    }
    match logical_char(event) {
        Some(c) => SharedString::from(c.to_string()),
        None => SharedString::new_static("Unidentified"),
    }
}

/// The character `event`'s key types on the current layout, as DOM's
/// `key` reports it: the source's own translation with every modifier and
/// lock held (AltGr's `@`, Shift's `A`). A Control/Alt/Windows combination
/// that only makes a control character (`Ctrl+A`'s 0x01) is no key name:
/// then the key without them is used (`a`), with Shift and Caps Lock kept
/// (a shortcut reports the character it is on). A dead key has none (it
/// composes with the next key), and neither has a key that types nothing.
fn logical_char(event: &KeyEvent) -> Option<char> {
    if dead_key(event.vk) {
        return None;
    }
    // The source's own translation first: AltGr's `@`, Shift's `?` and
    // Caps Lock's `A` are the character the key is, even though the
    // modifiers held are Control and Alt. A Control/Alt combination that
    // makes a control character (`Ctrl+A`'s 0x01) is not a key name: then
    // the key without them (`a`), which is what a shortcut is on.
    if let Some(c) = translated(event, &actual_state()) {
        return Some(c);
    }
    translated(event, &logical_state()).or_else(|| {
        // A virtual key with no character (an arrow, a function key).
        mapped_char(event.vk).filter(|c| !c.is_control())
    })
}

/// `event`'s key translated with `state` (`ToUnicodeEx`): the first
/// character, when the source writes one and it is printable. A dead key
/// is never translated (`dead_key`) and written nothing.
///
/// Every translation passes [`NO_STATE_CHANGE`], so a dead key in a layer
/// the virtual key's base character does not show (an AltGr dead key) is
/// read without being stored: the layout's pending dead key, ligatures and
/// Alt+numpad entry are left as `TranslateMessage` left them.
fn translated(event: &KeyEvent, state: &[u8; 256]) -> Option<char> {
    let mut buffer = [0u16; 4];
    // SAFETY: plain calls; `buffer` is a local out value and `state` a
    // full keyboard-state array.
    let written = unsafe {
        ToUnicodeEx(
            event.vk as u32,
            event.scan as u32,
            state,
            &mut buffer,
            NO_STATE_CHANGE,
            Some(layout()),
        )
    };
    if written <= 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..written as usize]);
    text.chars().next().filter(|c| !c.is_control())
}

/// The keyboard state as the message found it: what the character is with
/// every modifier and lock the user holds (AltGr's `@`, Shift's `A`).
fn actual_state() -> [u8; 256] {
    let mut state = [0u8; 256];
    // SAFETY: a full keyboard-state array as an out value.
    let _ = unsafe { GetKeyboardState(&mut state) };
    state
}

/// Whether Num Lock is on (its toggle, as of the key being handled).
fn num_lock() -> bool {
    // SAFETY: a plain query of this thread's key state.
    let state = unsafe { GetKeyState(VK_NUMLOCK.0 as i32) };
    state & 1 != 0
}

/// The key a keypad key is with Num Lock off.
fn keypad_navigation(vk: u16) -> Option<&'static str> {
    Some(match vk {
        0x60 => "Insert",
        0x61 => "End",
        0x62 => "ArrowDown",
        0x63 => "PageDown",
        0x64 => "ArrowLeft",
        0x65 => "Clear",
        0x66 => "ArrowRight",
        0x67 => "Home",
        0x68 => "ArrowUp",
        0x69 => "PageUp",
        0x6E => "Delete",
        _ => return None,
    })
}

/// The keyboard state with Control, Alt and the Windows key up: what a
/// shortcut's key is, without the control character they make. Shift and
/// Caps Lock stay, so the key keeps its actual case; `ToUnicodeEx` ignores
/// Num Lock and Scroll Lock (a keypad key with Num Lock off is named as
/// its navigation key by [`key_name`] before this).
fn logical_state() -> [u8; 256] {
    let mut state = [0u8; 256];
    // SAFETY: a full keyboard-state array as an out value.
    if unsafe { GetKeyboardState(&mut state) }.is_err() {
        return state;
    }
    for vk in [0x11, 0xA2, 0xA3, 0x12, 0xA4, 0xA5, 0x5B, 0x5C] {
        state[vk] = 0;
    }
    state
}

/// Whether `vk` is a dead key (it composes with the next key on the
/// current layout). `ToUnicodeEx` would then change the layout's pending
/// dead key, so its callers do not ask it.
fn dead_key(vk: u16) -> bool {
    let value = unsafe { MapVirtualKeyExW(vk as u32, MAPVK_VK_TO_CHAR, Some(layout())) };
    value & 0x8000_0000 != 0
}

/// The character `vk` maps to (`MapVirtualKeyEx`), when the layout has
/// one: letters come as the US layout's upper case here (the source's own
/// limit), which [`logical_char`] refines through `ToUnicodeEx`.
fn mapped_char(vk: u16) -> Option<char> {
    let value = unsafe { MapVirtualKeyExW(vk as u32, MAPVK_VK_TO_CHAR, Some(layout())) };
    if value & 0x8000_0000 != 0 {
        return None;
    }
    let unit = (value & 0xFFFF) as u16;
    if unit == 0 {
        return None;
    }
    char::from_u32(unit as u32)
}

/// The current thread's keyboard layout (the one its window's keys come
/// from).
fn layout() -> HKL {
    // SAFETY: a plain query for this thread's layout.
    unsafe { GetKeyboardLayout(0) }
}

/// The text `event`'s key types with the modifiers held (a preview, not
/// committed input-method text), and the UTF-16 units `TranslateMessage`
/// queued for it: the characters a capture removes.
fn typed_text(event: &KeyEvent) -> Option<SharedString> {
    if dead_key(event.vk) {
        return None;
    }
    let mut state = [0u8; 256];
    // SAFETY: a full keyboard-state array as an out value.
    if unsafe { GetKeyboardState(&mut state) }.is_err() {
        return None;
    }
    let mut buffer = [0u16; 8];
    // SAFETY: plain calls; `buffer` is a local out value.
    let written = unsafe {
        ToUnicodeEx(
            event.vk as u32,
            event.scan as u32,
            &state,
            &mut buffer,
            NO_STATE_CHANGE,
            Some(layout()),
        )
    };
    if written <= 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..written as usize]);
    let modifiers = event.modifiers;
    if modifiers.ctrl
        || modifiers.alt
        || modifiers.meta
        || text.is_empty()
        || text.chars().any(char::is_control)
    {
        return None;
    }
    Some(SharedString::from(text))
}

/// The message's physical key: its scan code, with `0x100` set for an
/// extended key. `None` when the message carries no scan code (input the
/// system injected as text, `VK_PACKET`).
fn physical(event: &KeyEvent) -> Option<PhysicalKey> {
    (event.scan != 0).then_some(PhysicalKey::Native(
        u32::from(event.scan) | (u32::from(event.extended) << 8),
    ))
}

/// The capture rule's keystroke: the same shortcut key [`crate::keymap`]
/// matches (virtual keys, US letters under non-Latin layouts), or `None`
/// for a key no shortcut names (a modifier key, an unidentified one) —
/// only [`nsh_core::KeyCapture::All`] takes those.
fn capture_keystroke(event: &KeyEvent) -> Option<Keystroke> {
    Some(Keystroke {
        modifiers: event.modifiers.core(),
        key: Key::from_virtual_key(event.vk)?,
    })
}

/// The core event of `event`, and the UTF-16 units it would type.
#[allow(
    clippy::field_reassign_with_default,
    reason = "nsh_core::KeyEvent is #[non_exhaustive]: its default, then its fields"
)]
fn core_event(event: &KeyEvent, down: bool, captured: bool) -> nsh_core::KeyEvent {
    let mut key = nsh_core::KeyEvent::default();
    key.key = key_name(event);
    key.text = if down { typed_text(event) } else { None };
    key.physical = physical(event);
    key.modifiers = event.modifiers.core();
    key.repeat = down && event.repeat;
    key.captured = captured;
    key
}

/// A key down on the focused custom view: reports it and says whether a
/// capture consumed it (no native default follows).
pub(crate) fn press(surface: &std::rc::Rc<Surface>, event: &KeyEvent) -> bool {
    let window = surface.input_root().id();
    let Some(view) = focused_in(surface, window) else {
        return false;
    };
    if !listens(surface, &view) {
        return false;
    }
    let stroke = capture_keystroke(event);
    let captured = captures(surface, &view, stroke.as_ref());
    let key = core_event(event, true, captured);
    surface.event(view.id(), EventKind::KeyDown, EventPayload::Key(key));
    // A captured key was never translated into characters
    // (`suppress_translation`), so no text follows it; no queue is
    // touched here, and a character the queue already held stays where it
    // is.
    captured
}

/// A key up: the focused custom view hears it (whichever view has the
/// focus now), with the capture rule that matches the release.
pub(crate) fn release(surface: &std::rc::Rc<Surface>, event: &KeyEvent) {
    let window = surface.input_root().id();
    let Some(view) = focused_in(surface, window) else {
        return;
    };
    if !listens(surface, &view) {
        return;
    }
    // A dialog over the app owns the keyboard: the view under it hears
    // neither the presses nor their releases.
    if crate::system::dialog_shown(surface) {
        return;
    }
    let stroke = capture_keystroke(event);
    let captured = captures(surface, &view, stroke.as_ref());
    let key = core_event(event, false, captured);
    surface.event(view.id(), EventKind::KeyUp, EventPayload::Key(key));
}

/// A key down that a mounted capture consumes must not be translated into
/// characters: this changes such a message to
/// [`crate::window::WM_NSH_KEYDOWN`] / [`crate::window::WM_NSH_SYSKEYDOWN`],
/// which `TranslateMessage` ignores, and the window procedure handles as
/// the key down it was.
///
/// The thread's `WH_GETMESSAGE` hook runs this before the caller's
/// `TranslateMessage` (the standalone loop calls it too), so a captured key
/// never queues a `WM_CHAR` / `WM_SYSCHAR`: nothing of the queue is
/// inspected, moved or restamped, and a character queued around the key
/// keeps its place. A key no mounted capture takes is left alone and
/// translates as usual.
pub(crate) fn suppress_translation(msg: &mut MSG) -> bool {
    let sys = msg.message == WM_SYSKEYDOWN;
    if !sys && msg.message != WM_KEYDOWN {
        return false;
    }
    let Some(surface) = crate::window::surface_of(msg.hwnd) else {
        return false;
    };
    let bits = msg.lParam.0 as usize;
    let event = KeyEvent {
        vk: msg.wParam.0 as u16,
        down: true,
        repeat: (bits >> 30) & 1 == 1,
        modifiers: crate::window::modifiers(),
        scan: ((bits >> 16) & 0xFF) as u16,
        extended: (bits >> 24) & 1 == 1,
        sys,
        translated: false,
    };
    let window = crate::window::app_window_root(msg.hwnd).unwrap_or(nsh_core::ROOT_ID);
    if !captured_now(&surface, window, &event) {
        return false;
    }
    msg.message = if sys {
        crate::window::WM_NSH_SYSKEYDOWN
    } else {
        crate::window::WM_NSH_KEYDOWN
    };
    true
}

/// Whether the key down on window `window`'s focused custom view is
/// consumed by a mounted capture rule: the decision [`press`] makes when
/// the message is dispatched.
fn captured_now(surface: &Surface, window: NodeId, event: &KeyEvent) -> bool {
    let Some(view) = focused_in(surface, window) else {
        return false;
    };
    if !listens(surface, &view) {
        return false;
    }
    let stroke = capture_keystroke(event);
    captures(surface, &view, stroke.as_ref())
}

#[cfg(test)]
mod tests {
    use nsh_core::{
        EventMask, MountItem, ROOT_ID, ViewProps,
        keymap::{Keystroke, Modifiers as CoreModifiers},
    };

    use super::*;
    use crate::{geom::rect, input::Modifiers};

    fn apply(surface: &std::rc::Rc<Surface>, items: &[MountItem]) {
        for item in items {
            surface.mounter.borrow_mut().apply(surface, item);
        }
    }

    fn surface() -> std::rc::Rc<Surface> {
        let surface = Surface::new(nsh_core::platform::Environment::default());
        surface.root.state_mut().frame = rect(0.0, 0.0, 400.0, 400.0);
        surface
    }

    /// A focusable view `id` under the root.
    fn focusable(surface: &std::rc::Rc<Surface>, id: NodeId) -> NshView {
        apply(
            surface,
            &[
                MountItem::Create {
                    id,
                    kind: ViewKind::View,
                },
                MountItem::Insert {
                    parent: ROOT_ID,
                    child: id,
                    before: None,
                },
                MountItem::Frame {
                    id,
                    frame: rect(0.0, 0.0, 100.0, 40.0),
                },
            ],
        );
        let view = surface.view(id).expect("the view");
        let mut props: ViewProps = (*view.state().props).clone();
        props.focusable = Some(true);
        apply(surface, &[MountItem::ViewProps { id, props: Box::new(props) }]);
        view
    }

    fn key(vk: u16) -> KeyEvent {
        KeyEvent {
            vk,
            down: true,
            repeat: false,
            modifiers: Modifiers::default(),
            scan: 0,
            extended: false,
            sys: false,
            translated: false,
        }
    }

    fn stroke(text: &str) -> Keystroke {
        Keystroke::parse_with(text, false).expect(text)
    }

    #[test]
    fn named_keys_follow_dom_spellings() {
        for (vk, name) in [
            (0x0D, "Enter"),
            (0x09, "Tab"),
            (0x08, "Backspace"),
            (0x1B, "Escape"),
            (0x20, " "),
            (0x2E, "Delete"),
            (0x25, "ArrowLeft"),
            (0x26, "ArrowUp"),
            (0x27, "ArrowRight"),
            (0x28, "ArrowDown"),
            (0x24, "Home"),
            (0x23, "End"),
            (0x21, "PageUp"),
            (0x22, "PageDown"),
            (0x10, "Shift"),
            (0x11, "Control"),
            (0x12, "Alt"),
            (0x5B, "Meta"),
            (0x70, "F1"),
            (0x74, "F5"),
            (0x87, "F24"),
        ] {
            assert_eq!(key_name(&key(vk)).as_str(), name, "{vk:#x}");
        }
        // A key the system means to a text service has no name of its own.
        assert_eq!(key_name(&key(0xE5)).as_str(), "Unidentified");
    }

    #[test]
    fn the_physical_key_is_the_scan_code_and_extended_bit() {
        let mut down = key(0x25);
        down.scan = 0x4B;
        down.extended = true;
        assert!(
            matches!(physical(&down), Some(PhysicalKey::Native(0x14B))),
            "the extended bit is 0x100"
        );
        down.extended = false;
        down.scan = 0x1E;
        assert!(matches!(physical(&down), Some(PhysicalKey::Native(0x1E))));
        // Input injected as text has no scan code.
        down.scan = 0;
        assert!(physical(&down).is_none());
    }

    #[test]
    fn capture_walks_enabled_ancestors_by_their_shortcuts() {
        let surface = surface();
        let outer = focusable(&surface, 2);
        let inner = focusable(&surface, 4);
        apply(
            &surface,
            &[MountItem::Insert {
                parent: 2,
                child: 4,
                before: None,
            }],
        );
        let escape = stroke("escape");
        assert!(!listens(&surface, &inner), "no listeners and no rules");
        let mut props = outer.state().props.clone();
        props.key_capture = nsh_core::KeyCapture::keys([escape]);
        apply(&surface, &[MountItem::ViewProps { id: 2, props }]);
        assert!(listens(&surface, &inner), "a capture rule is a raw path");
        let mut mask = EventMask::default();
        mask.set(EventKind::KeyDown, true);
        apply(
            &surface,
            &[MountItem::Events {
                id: 2,
                events: mask,
            }],
        );
        assert!(listens(&surface, &inner));
        assert!(captures(&surface, &inner, Some(&escape)));
        let enter = stroke("enter");
        assert!(!captures(&surface, &inner, Some(&enter)));
        // Only `All` takes a key no shortcut names.
        assert!(!captures(&surface, &inner, None));
        let mut props = outer.state().props.clone();
        props.key_capture = nsh_core::KeyCapture::All;
        apply(&surface, &[MountItem::ViewProps { id: 2, props }]);
        assert!(captures(&surface, &inner, None));
        // A disabled view captures nothing.
        let mut props = outer.state().props.clone();
        props.accessibility.state.disabled = true;
        apply(&surface, &[MountItem::ViewProps { id: 2, props }]);
        assert!(!captures(&surface, &inner, None));
    }

    #[test]
    fn the_capture_uses_the_keymaps_shortcut_key() {
        let mut event = key(0x41);
        assert_eq!(
            capture_keystroke(&event),
            Some(Keystroke {
                modifiers: CoreModifiers::default(),
                key: Key::Char('a'),
            })
        );
        event.modifiers.ctrl = true;
        assert_eq!(
            capture_keystroke(&event),
            Some(Keystroke {
                modifiers: CoreModifiers {
                    ctrl: true,
                    ..Default::default()
                },
                key: Key::Char('a'),
            })
        );
        // A modifier key has no shortcut.
        assert!(capture_keystroke(&key(0x10)).is_none());
    }

    #[test]
    fn a_capture_consumes_the_press_and_a_listener_cannot() {
        let surface = surface();
        let view = focusable(&surface, 2);
        let mut props = view.state().props.clone();
        props.key_capture = nsh_core::KeyCapture::All;
        apply(&surface, &[MountItem::ViewProps { id: 2, props }]);
        assert!(focus_control(&surface, &view));
        assert!(
            press(&surface, &key(0x41)),
            "a captured key is consumed before the defaults"
        );
        // Listening does not consume.
        let mut props = view.state().props.clone();
        props.key_capture = nsh_core::KeyCapture::None;
        apply(&surface, &[MountItem::ViewProps { id: 2, props }]);
        let mut mask = EventMask::default();
        mask.set(EventKind::KeyDown, true);
        apply(
            &surface,
            &[MountItem::Events {
                id: 2,
                events: mask,
            }],
        );
        assert!(listens(&surface, &view));
        assert!(!press(&surface, &key(0x41)));
    }

    #[test]
    fn the_event_reports_the_transition() {
        let mut down = key(0x41);
        down.repeat = true;
        let event = core_event(&down, true, true);
        assert!(event.captured);
        assert!(event.repeat, "the message's previous-key-state bit");
        let event = core_event(&down, false, false);
        assert!(!event.captured);
        assert!(!event.repeat, "a release is never an auto-repeat");
        assert!(event.text.is_none(), "a release types no text");
        // Control makes a shortcut, not text.
        let mut ctrl = key(0x41);
        ctrl.modifiers.ctrl = true;
        let event = core_event(&ctrl, true, false);
        assert!(event.text.is_none());
    }

    /// A modal root `id`, presented without animation.
    fn modal(surface: &std::rc::Rc<Surface>, id: NodeId) -> NshView {
        use nsh_core::{ModalAnimation, ModalConfig};
        let view = NshView::new(id, ViewKind::Modal);
        view.state_mut().frame = rect(0.0, 0.0, 400.0, 400.0);
        crate::navigation::modal_config(
            surface,
            &view,
            &ModalConfig {
                animation: ModalAnimation::None,
                ..Default::default()
            },
        );
        view
    }

    /// A view `id` under `parent`.
    fn under(
        surface: &std::rc::Rc<Surface>,
        parent: NodeId,
        id: NodeId,
        focusable: bool,
    ) -> NshView {
        apply(
            surface,
            &[
                MountItem::Create {
                    id,
                    kind: ViewKind::View,
                },
                MountItem::Insert {
                    parent,
                    child: id,
                    before: None,
                },
                MountItem::Frame {
                    id,
                    frame: rect(0.0, 0.0, 100.0, 40.0),
                },
            ],
        );
        let view = surface.view(id).expect("the view");
        let mut props: ViewProps = (*view.state().props).clone();
        props.focusable = focusable.then_some(true);
        apply(surface, &[MountItem::ViewProps { id, props: Box::new(props) }]);
        view
    }

    /// A view a native surface mounted at a root it does not logically live
    /// under (an `Overlay`, portaled by core) follows `key_parent` for its
    /// keyboard interest and capture: only its logical ancestors count, not
    /// the mounted parent or a sibling portal's views.
    #[test]
    fn a_portaled_views_keys_follow_its_logical_ancestors() {
        let surface = surface();
        // The logical ancestor, not the root, hears keys and captures one.
        let ancestor = under(&surface, ROOT_ID, 2, false);
        let mut mask = EventMask::default();
        mask.set(EventKind::KeyDown, true);
        apply(
            &surface,
            &[MountItem::Events {
                id: 2,
                events: mask,
            }],
        );
        let escape = stroke("escape");
        let mut props = ancestor.state().props.clone();
        props.key_capture = nsh_core::KeyCapture::keys([escape]);
        apply(&surface, &[MountItem::ViewProps { id: 2, props }]);
        // The portal: mounted at the root, logically under the ancestor.
        let portal = under(&surface, ROOT_ID, 4, false);
        let mut props = portal.state().props.clone();
        props.key_parent = Some(2);
        apply(&surface, &[MountItem::ViewProps { id: 4, props }]);
        let child = under(&surface, 4, 6, true);
        assert_eq!(
            portal.parent_view().map(|view| view.id()),
            Some(ROOT_ID),
            "the mounted parent is the root, not the logical one"
        );
        assert!(focus_control(&surface, &child));
        assert!(
            listens(&surface, &child),
            "the logical ancestor's listener is heard"
        );
        assert!(
            captures(&surface, &child, Some(&escape)),
            "the logical ancestor captures"
        );
        assert!(press(&surface, &key(0x1B)));
        // The raw payload keeps the source's key and physical code.
        let reported = core_event(&key(0x1B), true, true);
        assert_eq!(reported.key.as_str(), "Escape");
        assert!(reported.text.is_none());
        assert!(reported.captured);
        assert!(reported.physical.is_none());
        // An unrelated sibling portal has no such ancestor: nothing is
        // heard or captured, and a key there is not consumed.
        let sibling = under(&surface, ROOT_ID, 8, false);
        let other = under(&surface, 8, 10, true);
        assert!(sibling.parent_view().is_some());
        assert!(!listens(&surface, &other));
        assert!(!captures(&surface, &other, Some(&escape)));
        assert!(focus_control(&surface, &other));
        assert!(!press(&surface, &key(0x1B)));
    }

    #[test]
    fn a_modal_restores_the_custom_focus_it_took() {
        let surface = surface();
        let view = focusable(&surface, 2);
        assert!(focus_control(&surface, &view));
        let dialog = modal(&surface, 4);
        crate::navigation::present(&surface, &dialog);
        assert!(
            focused_in(&surface, ROOT_ID).is_none(),
            "the modal takes the keyboard from the view behind it"
        );
        crate::navigation::dismiss(&surface, &dialog);
        assert_eq!(
            focused_in(&surface, ROOT_ID).as_ref(),
            Some(&view),
            "closing the modal gives the focus back, as a field's"
        );
        // A view that is gone, disabled or no longer a stop restores
        // nothing.
        let dialog = modal(&surface, 6);
        crate::navigation::present(&surface, &dialog);
        let mut props: ViewProps = (*view.state().props).clone();
        props.accessibility.state.disabled = true;
        apply(&surface, &[MountItem::ViewProps { id: 2, props: Box::new(props) }]);
        crate::navigation::dismiss(&surface, &dialog);
        assert!(focused_in(&surface, ROOT_ID).is_none());
        let mut props: ViewProps = (*view.state().props).clone();
        props.accessibility.state.disabled = false;
        apply(&surface, &[MountItem::ViewProps { id: 2, props: Box::new(props) }]);
        assert!(focus_control(&surface, &view));
        let dialog = modal(&surface, 8);
        crate::navigation::present(&surface, &dialog);
        apply(&surface, &[MountItem::Delete { id: 2 }]);
        crate::navigation::dismiss(&surface, &dialog);
        assert!(focused_in(&surface, ROOT_ID).is_none());
    }

    #[test]
    fn keypad_keys_are_the_navigation_keys_with_num_lock_off() {
        for (vk, name) in [
            (0x60, "Insert"),
            (0x61, "End"),
            (0x62, "ArrowDown"),
            (0x63, "PageDown"),
            (0x64, "ArrowLeft"),
            (0x65, "Clear"),
            (0x66, "ArrowRight"),
            (0x67, "Home"),
            (0x68, "ArrowUp"),
            (0x69, "PageUp"),
            (0x6E, "Delete"),
        ] {
            assert_eq!(keypad_navigation(vk), Some(name), "{vk:#x}");
        }
        assert!(keypad_navigation(0x41).is_none());
    }

    /// AltGr (Windows reports it as Control+Alt) keeps the character the
    /// source translates, when it has one; a control character is not the
    /// key's name, so the key without Control/Alt is used.
    #[test]
    fn the_logical_key_prefers_the_sources_actual_character() {
        let mut event = key(0x41);
        event.scan = 0x1E;
        let mut state = [0u8; 256];
        // Control and Alt down, as AltGr is reported.
        state[0x11] = 0x80;
        state[0x12] = 0x80;
        // SAFETY: a full keyboard-state array for this thread.
        if unsafe { SetKeyboardState(&state) }.is_err() {
            // A thread that cannot set its key state has no AltGr state to
            // exercise here.
            return;
        }
        let actual = translated(&event, &actual_state());
        let base = translated(&event, &logical_state());
        let expected = actual
            .or(base)
            .or_else(|| mapped_char(event.vk).filter(|c| !c.is_control()));
        let reported = core_event(&event, true, false);
        match expected {
            Some(character) => {
                assert_eq!(reported.key.as_str(), character.to_string().as_str())
            }
            None => assert_eq!(reported.key.as_str(), "Unidentified"),
        }
        if let Some(character) = actual {
            assert_eq!(
                reported.key.as_str(),
                character.to_string().as_str(),
                "the source's actual character wins (AltGr)"
            );
        }
        let clear = [0u8; 256];
        // SAFETY: as above.
        let _ = unsafe { SetKeyboardState(&clear) };
    }

    /// `ToUnicodeEx` runs with its no-state-change flag: a dead key read
    /// for an event is not stored, so asking twice is still the dead key
    /// (a state-changing call would compose it with itself). The setup
    /// looks for a dead key on this layout; one without any checks nothing.
    /// An AltGr-layer-only dead key is covered by the same flag, though
    /// `MapVirtualKeyEx` cannot name it.
    #[test]
    fn translations_do_not_store_a_dead_key() {
        let Some(vk) = (0x20u16..=0xFF).find(|vk| dead_key(*vk)) else {
            return;
        };
        let event = key(vk);
        let state = actual_state();
        assert!(translated(&event, &state).is_none(), "{vk:#x} is dead");
        assert!(
            translated(&event, &state).is_none(),
            "{vk:#x} composed with itself: the dead key was stored"
        );
    }

    #[test]
    fn focus_and_tab_walk_the_focus_stops() {
        let surface = surface();
        let first = focusable(&surface, 2);
        let second = focusable(&surface, 4);
        assert!(focus_control(&surface, &first));
        assert_eq!(focused_in(&surface, ROOT_ID).as_ref(), Some(&first));
        assert!(!is_stop(&surface.root));
        // Tab moves to the next stop and Shift+Tab back.
        let tab = key(0x09);
        assert!(crate::input::tab(&surface, &tab));
        assert_eq!(focused_in(&surface, ROOT_ID).as_ref(), Some(&second));
        let back = KeyEvent {
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
            ..tab
        };
        assert!(crate::input::tab(&surface, &back));
        assert_eq!(focused_in(&surface, ROOT_ID).as_ref(), Some(&first));
        // A press on the top view focuses it.
        crate::input::pointer(
            &surface,
            crate::input::PointerEvent {
                kind: crate::input::PointerKind::Down,
                x: 10.0,
                y: 10.0,
                button: 0,
                buttons: 1,
                pointer: 1,
                touch: false,
                pen: false,
                time: 0.0,
                clicks: 1,
                modifiers: Modifiers::default(),
            },
        );
        assert_eq!(focused_in(&surface, ROOT_ID).as_ref(), Some(&second));
        // Removed: the focus goes with the view.
        apply(&surface, &[MountItem::Delete { id: 4 }]);
        assert!(focused_in(&surface, ROOT_ID).is_none());
    }
}
