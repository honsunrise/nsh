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
//! props of the focused view and its enabled ancestors, before the native
//! defaults of [`crate::input::key`] (the application menu, a focused
//! field's editing, navigation, scrolling, Tab's traversal). A captured
//! key is still reported, with `captured` set, and runs none of those
//! defaults. The `WM_CHAR` messages `TranslateMessage` queued for a
//! captured key are removed from the window's queue
//! ([`crate::keymap::drop_chars`]), so no text is typed; nothing is
//! remembered afterwards, so the next key's characters are untouched.
//!
//! Focus is per native window (the main window's root, or the root of a
//! window of the app's own): a window in the background keeps its view, and
//! the keys of the window in front only reach its own. A view that is
//! removed, concealed or disabled loses the focus without a synthetic key
//! release; releases are reported when the source sends them, on whatever
//! view has the focus then, and repeats are what the message's
//! previous-key-state bit says.

use std::cell::RefCell;

use nsh_core::{
    EventKind, EventPayload, NodeId, PhysicalKey, PointerEvents, ViewKind,
    element::SharedString,
    keymap::{Key, Keystroke},
};
use rustc_hash::FxHashMap;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, GetKeyboardLayout, GetKeyboardState, HKL, MAPVK_VK_TO_CHAR,
    MapVirtualKeyExW, SetFocus, ToUnicodeEx, VK_NUMLOCK,
};

use crate::{app_window::window_of, input::KeyEvent, surface::Surface, view::NshView};

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

/// Whether `view` or an ancestor within its window hears `KeyDown` /
/// `KeyUp` or captures keys. A view that does neither takes the original
/// path of [`crate::input::key`], with no event built.
pub(crate) fn listens(surface: &Surface, view: &NshView) -> bool {
    let mut current = Some(view.clone());
    while let Some(view) = current {
        let heard = {
            let state = view.state();
            state.events.contains(EventKind::KeyDown)
                || state.events.contains(EventKind::KeyUp)
                || !state.props.key_capture.is_none()
        };
        if heard {
            return true;
        }
        if window_root(surface, &view) {
            return false;
        }
        current = view.parent_view();
    }
    false
}

/// Whether the focused `view` or one of its enabled ancestors captures the
/// key of `stroke` (`capture_keys`; [`nsh_core::KeyCapture`]). Disabled
/// views and the views outside the window capture nothing.
pub(crate) fn captures(surface: &Surface, view: &NshView, stroke: Option<&Keystroke>) -> bool {
    let mut current = Some(view.clone());
    while let Some(view) = current {
        let captured = {
            let state = view.state();
            !state.props.accessibility.state.disabled
                && state.props.key_capture.captures_key(stroke)
        };
        if captured {
            return true;
        }
        if window_root(surface, &view) {
            return false;
        }
        current = view.parent_view();
    }
    false
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
    if surface.keyboard.focus.borrow().get(&window).is_some_and(|held| held == view) {
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
    crate::input::set_state(
        surface,
        view,
        nsh_core::tailwind::state::FOCUSED,
        true,
    );
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
    crate::input::set_state(
        surface,
        view,
        nsh_core::tailwind::state::FOCUSED,
        false,
    );
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

/// The character `event`'s key types on the current layout with Shift
/// and Caps Lock (DOM's `key`: `"A"` for Shift+A), Control, Alt and the
/// Windows key ignored (a shortcut reports the character it is on, not a
/// control character). A dead key has none (it composes with the next
/// key), and neither has a key that types nothing.
fn logical_char(event: &KeyEvent) -> Option<char> {
    if dead_key(event.vk) {
        return None;
    }
    let state = logical_state();
    let mut buffer = [0u16; 4];
    // SAFETY: plain calls; `buffer` is a local out value and `state` a
    // full keyboard-state array.
    let written = unsafe {
        ToUnicodeEx(
            event.vk as u32,
            event.scan as u32,
            &state,
            &mut buffer,
            0,
            Some(layout()),
        )
    };
    if written > 0 {
        let text = String::from_utf16_lossy(&buffer[..written as usize]);
        // A control character is no key name (`Cancel`, `Tab` and the
        // like are named above).
        return text.chars().next().filter(|c| !c.is_control());
    }
    if written < 0 {
        // A dead key the mapping below would have told us about.
        return None;
    }
    // A virtual key with no character (an arrow, a function key).
    mapped_char(event.vk).filter(|c| !c.is_control())
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
/// shortcut's key is, without the control character they make. Shift,
/// Caps Lock, Num Lock and the layout's own toggles stay, so the key keeps
/// its actual case and a keypad key keeps its digit with Num Lock on.
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
fn typed_text(event: &KeyEvent) -> (Option<SharedString>, usize) {
    if dead_key(event.vk) {
        return (None, 0);
    }
    let mut state = [0u8; 256];
    // SAFETY: a full keyboard-state array as an out value.
    if unsafe { GetKeyboardState(&mut state) }.is_err() {
        return (None, 0);
    }
    let mut buffer = [0u16; 8];
    // SAFETY: plain calls; `buffer` is a local out value.
    let written = unsafe {
        ToUnicodeEx(
            event.vk as u32,
            event.scan as u32,
            &state,
            &mut buffer,
            0,
            Some(layout()),
        )
    };
    if written <= 0 {
        return (None, 0);
    }
    let count = written as usize;
    let text = String::from_utf16_lossy(&buffer[..count]);
    let modifiers = event.modifiers;
    let preview = if !modifiers.ctrl
        && !modifiers.alt
        && !modifiers.meta
        && !text.is_empty()
        && !text.chars().any(char::is_control)
    {
        Some(SharedString::from(text))
    } else {
        None
    };
    (preview, count)
}

/// The message's physical key: its scan code, with `0x100` set for an
/// extended key. `None` when the message carries no scan code (input the
/// system injected as text, `VK_PACKET`).
fn physical(event: &KeyEvent) -> Option<PhysicalKey> {
    (event.scan != 0)
        .then_some(PhysicalKey::Native(u32::from(event.scan) | (u32::from(event.extended) << 8)))
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
fn core_event(event: &KeyEvent, down: bool, captured: bool) -> (nsh_core::KeyEvent, usize) {
    let (text, units) = if down {
        typed_text(event)
    } else {
        (None, 0)
    };
    let mut key = nsh_core::KeyEvent::default();
    key.key = key_name(event);
    key.text = text;
    key.physical = physical(event);
    key.modifiers = event.modifiers.core();
    key.repeat = down && event.repeat;
    key.captured = captured;
    (key, units)
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
    let (key, units) = core_event(event, true, captured);
    surface.event(view.id(), EventKind::KeyDown, EventPayload::Key(key));
    if captured {
        // No text follows the captured key: the characters it would have
        // typed go, and only those (a key that types none removes none).
        crate::keymap::drop_chars(surface, units);
        return true;
    }
    false
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
    let (key, _) = core_event(event, false, captured);
    surface.event(view.id(), EventKind::KeyUp, EventPayload::Key(key));
}

#[cfg(test)]
mod tests {
    use nsh_core::{
        EventMask, MountItem, ROOT_ID, ViewProps,
        keymap::{Keystroke, Modifiers as CoreModifiers},
    };

    use super::*;
    use crate::geom::rect;
    use crate::input::Modifiers;

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
        let mut props: ViewProps = view.state().props.clone();
        props.focusable = Some(true);
        apply(surface, &[MountItem::ViewProps { id, props }]);
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
        apply(&surface, &[MountItem::Events { id: 2, events: mask }]);
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
        apply(&surface, &[MountItem::Events { id: 2, events: mask }]);
        assert!(listens(&surface, &view));
        assert!(!press(&surface, &key(0x41)));
    }

    #[test]
    fn the_event_reports_the_transition() {
        let mut down = key(0x41);
        down.repeat = true;
        let (event, _) = core_event(&down, true, true);
        assert!(event.captured);
        assert!(event.repeat, "the message's previous-key-state bit");
        let (event, units) = core_event(&down, false, false);
        assert!(!event.captured);
        assert!(!event.repeat, "a release is never an auto-repeat");
        assert!(event.text.is_none(), "a release types no text");
        assert_eq!(units, 0);
        // Control makes a shortcut, not text.
        let mut ctrl = key(0x41);
        ctrl.modifiers.ctrl = true;
        let (event, _) = core_event(&ctrl, true, false);
        assert!(event.text.is_none());
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
