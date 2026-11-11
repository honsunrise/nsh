//! The keys and text of a focused view: the `on_key_down` / `on_key_up`
//! events a view and its ancestors hear, text generation through an input
//! method (`on_text_input`; its preedit is `on_composition_change`) and
//! where the input method shows its candidates (`imeCursorArea`).
//!
//! The focus and the keys themselves are [`crate::key_focus`]'s: its one
//! key controller per window decides, in GTK's capture phase, whether the
//! key is the app's ([`crate::keymap::claims`]), captured by the focused
//! view (`capture_keys`), a default (Tab's traversal, Return and Space
//! pressing, a context menu, scrolling), or text for the input method.
//! This module only does what that decision needs: it builds the
//! [`KeyEvent`], matches the focus's capture rules ([`captures`]), reports
//! the event to the engine as the plain `KeyDown` / `KeyUp` events
//! ([`notify`], [`released`]), and runs the input method ([`text`]). No
//! engine answer is waited for: what a listener does is reported
//! afterwards, in the app's frame.
//!
//! A captured key still notifies the focused view and its ancestors (the
//! engine bubbles it), with `KeyEvent::captured` set; it generates no text
//! and runs no default (no traversal, activation or context menu). A key
//! that only *listens* is not captured: the app hears it and the default
//! still runs.
//!
//! The focus itself stays [`crate::key_focus`]'s: a stop reports
//! `FOCUSED`, `onFocus` / `onBlur` there, and this module follows the
//! focus it reports ([`focus_changed`]) to attach and detach the input
//! method, ending a composition it leaves pending.
//!
//! A view that hears none of the raw keys and captures none takes
//! [`crate::key_focus`]'s original path: [`listens`] is a few mask
//! checks, with no raw [`KeyEvent`] or down-target allocation. Modifier
//! transitions remain tracked across focus moves, including from native inputs.

use std::{cell::RefCell, rc::Rc};

use gtk4::{self as gtk, gdk, glib, prelude::*};
use nsh_core::{Bounds, EventKind, EventPayload, Key, KeyEvent, NamedKey, Points, TextInputEvent};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{surface::Surface, view::NshView};

/// A surface's keyboard state.
#[derive(Default)]
pub(crate) struct Keyboard {
    /// The view GTK last gave the focus to (none: a text field, a hosted
    /// widget or nothing).
    focused: RefCell<Option<glib::WeakRef<NshView>>>,
    /// The hardware keycodes held down, and the window each was pressed
    /// in, for auto-repeat detection (GDK does not mark repeats).
    pressed: RefCell<FxHashMap<u32, glib::WeakRef<gtk::Window>>>,
    /// The view each held key went down on, its key up returning there,
    /// and whether the press was captured (the key up says the same).
    down_targets: RefCell<FxHashMap<u32, (glib::WeakRef<NshView>, bool)>>,
    /// The input method attached to the focused view, while it hears text.
    ime: RefCell<Option<Ime>>,
    /// The preedit the input method last reported (to send each change
    /// once). It runs even when only committed text is listened to.
    preedit: RefCell<String>,
    /// The physical modifier keys held (by keycode) and the modifier each
    /// stands for: a key event's modifier mask comes from these, not from
    /// its pre-key state, so releasing one Shift keeps the other's.
    modifiers: RefCell<FxHashMap<u32, gdk::ModifierType>>,
    /// The surface, for the input method's signals (which outlive the call
    /// that attaches it).
    weak: RefCell<Option<std::rc::Weak<Surface>>>,
}

/// An input method context of a focused view.
struct Ime {
    context: gtk::IMMulticontext,
    /// The view it is attached to (`client_widget`).
    widget: gtk::Widget,
    /// The keycodes whose press this context filtered: their release is the
    /// context's too, whatever view the raw `KeyUp` goes to. Dropped with
    /// the context.
    downs: FxHashSet<u32>,
}

/// Remembers the surface, for the input method's signals. The key
/// controller itself is [`crate::key_focus`]'s.
pub(crate) fn install(surface: &Rc<Surface>) {
    *surface.keyboard.weak.borrow_mut() = Some(Rc::downgrade(surface));
}

/// The app stopped: forgets the focus, the keys held and the input method.
pub(crate) fn reset(surface: &Surface) {
    end_composition(surface);
    detach_ime(surface);
    surface.keyboard.pressed.borrow_mut().clear();
    surface.keyboard.modifiers.borrow_mut().clear();
    surface.keyboard.down_targets.borrow_mut().clear();
    surface.keyboard.focused.borrow_mut().take();
    surface.keyboard.preedit.borrow_mut().clear();
}

/// Whether `kind` is heard by `view`.
fn hears(view: &NshView, kind: EventKind) -> bool {
    view.state().events.contains(kind)
}

/// Whether `view` is the view the input method is for (the focus the
/// keyboard follows): a view of another, inactive window changing does not
/// repoint it.
fn owner(surface: &Surface, view: &NshView) -> bool {
    surface
        .keyboard
        .focused
        .borrow()
        .as_ref()
        .and_then(|weak| weak.upgrade())
        .is_some_and(|focused| focused == *view)
}

/// Whether the input method attached to `view` is composing (its preedit
/// is not empty): the keys then go to it before the platform's defaults.
pub(crate) fn composing(surface: &Surface, view: &NshView) -> bool {
    attached_to(surface, view) && !surface.keyboard.preedit.borrow().is_empty()
}

/// Whether the surface's input method context is attached to `view`.
fn attached_to(surface: &Surface, view: &NshView) -> bool {
    surface
        .keyboard
        .ime
        .borrow()
        .as_ref()
        .is_some_and(|ime| ime.widget == view.clone().upcast::<gtk::Widget>())
}

/// `MountItem::ViewProps`: where the input method puts its candidate
/// window may have moved, or the view may have stopped hearing text.
pub(crate) fn props_changed(surface: &Surface, view: &NshView) {
    events_changed(surface, view);
}

/// `MountItem::Events`: the focused view started (or stopped) hearing text.
pub(crate) fn events_changed(surface: &Surface, view: &NshView) {
    if !owner(surface, view) {
        return;
    }
    if wants_text(view) && crate::key_focus::is_stop(view) {
        attach_ime(surface, view);
    } else {
        end_composition(surface);
        detach_ime(surface);
    }
}

/// The focus moved (`key_focus::moved`): the input method follows it. The
/// keys held are not released by a focus move (a window-level controller
/// still sees their releases): they stay held, and their repeats keep their
/// original down target.
pub(crate) fn focus_changed(surface: &Surface, window: &gtk::Window, next: Option<&NshView>) {
    // Each window retains its local focus. Setting it while the window is
    // inactive must not end the active window's composition.
    if next.is_some() && !window.is_active() {
        return;
    }
    let tracked = surface
        .keyboard
        .focused
        .borrow()
        .as_ref()
        .and_then(|weak| weak.upgrade());
    match next {
        Some(view) if tracked.as_ref() != Some(view) => {
            // The keys held are not released by a focus move: their
            // repeats and their original down targets stay (a window-level
            // controller still sees the release).
            end_composition(surface);
            detach_ime(surface);
            attach_ime(surface, view);
            *surface.keyboard.focused.borrow_mut() = Some(view.downgrade());
        }
        Some(_) => {}
        None => {
            // This window lost its focus: forget it only when the view
            // tracked was in it (another window's view keeps the input
            // method). Keys held stay too: the window may still have the
            // OS focus and send their releases.
            let in_window = tracked.is_some_and(|view| {
                view.root().and_downcast::<gtk::Window>().as_ref() == Some(window)
            });
            if in_window {
                end_composition(surface);
                detach_ime(surface);
                surface.keyboard.focused.borrow_mut().take();
            }
        }
    }
}

/// `window` became active: its focus widget is the keyboard's focus again.
/// GTK keeps each window's focus widget while it is inactive, and no
/// `focus-widget` change tells of the switch back, so the input method is
/// pointed at that view (and its composition ended for the one it left).
pub(crate) fn window_activated(surface: &Surface, window: &gtk::Window) {
    let Some(focus) = GtkWindowExt::focus(window) else {
        return;
    };
    let Some(view) = focus.downcast::<NshView>().ok() else {
        return;
    };
    if !crate::window::holds(surface, &view) {
        return;
    }
    focus_changed(surface, window, Some(&view));
}

/// Whether `view` hears what an input method context is needed for.
fn wants_text(view: &NshView) -> bool {
    hears(view, EventKind::TextInput) || hears(view, EventKind::CompositionChange)
}

/// `window` lost the OS focus (`is-active` off): its releases may never
/// come, so the keys held there are no press of a key any more and the down
/// targets of its views are forgotten (another window's stay). Its input
/// method leaves the view it had.
pub(crate) fn window_deactivated(surface: &Surface, window: &gtk::Window) {
    focus_changed(surface, window, None);
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

/// Whether `view` or an ancestor hears a raw key or text event, or
/// captures keys: only then does a key need a [`KeyEvent`], its capture
/// rules and the per-key state. A view that hears none takes
/// [`crate::key_focus`]'s original path, with nothing built.
pub(crate) fn listens(view: &NshView) -> bool {
    let mut current = Some(view.clone());
    while let Some(view) = current {
        {
            let state = view.state();
            let raw = state.events.contains(EventKind::KeyDown)
                || state.events.contains(EventKind::KeyUp)
                || state.events.contains(EventKind::TextInput)
                || state.events.contains(EventKind::CompositionChange);
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

/// A key came up: tell the view the key went down on (that one heard the
/// down, whatever has the focus now), and offer the release to the input
/// method that took the press — never to another view's (the focus may
/// have moved since).
pub(crate) fn released(
    surface: &Surface,
    window: Option<&gtk::Window>,
    keyval: gdk::Key,
    keycode: u32,
    state: gdk::ModifierType,
    group: i32,
    event: Option<&gdk::Event>,
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
    if !held_here {
        return;
    }
    surface.keyboard.pressed.borrow_mut().remove(&keycode);
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
    // GDK's input methods see the release too (`IMContext::filter_keypress`
    // handles both), but only the context that filtered its press: the
    // current one may be another view's (the focus moved) or may only have
    // seen repeats (a capture policy changed). The raw key up above goes to
    // the view the key went down on regardless, even if it is gone.
    let saw_press = surface
        .keyboard
        .ime
        .borrow_mut()
        .as_mut()
        .is_some_and(|ime| ime.downs.remove(&keycode));
    if saw_press
        && let Some(event) = event
        && let Some(ime) = surface.keyboard.ime.borrow().as_ref()
    {
        ime.context.filter_keypress(event);
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
    // Remove character case/Shift, not NumLock or layout-level selectors:
    // an unshifted numeric keypad key is still a digit with NumLock on.
    let unshifted = state.difference(gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::LOCK_MASK);
    let base = display
        .translate_key(keycode, state, group)
        .and_then(|(_, group, _, _)| display.translate_key(keycode, unshifted, group))
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

/// Offers the key to the focused view's input method context; true when it
/// took it (its commit or preedit arrives through the signals). Also
/// attaches the context when the view started hearing text after it got
/// the focus.
pub(crate) fn text(
    surface: &Surface,
    view: &NshView,
    keycode: u32,
    event: Option<&gdk::Event>,
) -> bool {
    if !wants_text(view) {
        return false;
    }
    if !attached_to(surface, view) {
        // The key went to `view`: a context still bound to the view it
        // replaced (the focus moved between windows without a
        // `focus-widget` change) would take its text and put its candidates
        // there. End what it composed, then bind it here.
        end_composition(surface);
        *surface.keyboard.focused.borrow_mut() = Some(view.downgrade());
        attach_ime(surface, view);
    }
    let Some(event) = event else {
        return false;
    };
    // The context filters this press (its commits and preedits arrive
    // through the signals); its release belongs to it too.
    let handled = match surface.keyboard.ime.borrow().as_ref() {
        Some(ime) => ime.context.filter_keypress(event),
        None => return false,
    };
    if let Some(ime) = surface.keyboard.ime.borrow_mut().as_mut() {
        ime.downs.insert(keycode);
    }
    handled
}

/// Attaches an input method context to `view` when it hears text (or
/// replaces the one it has).
fn attach_ime(surface: &Surface, view: &NshView) {
    if !wants_text(view) || !crate::key_focus::is_stop(view) {
        detach_ime(surface);
        return;
    }
    let same = surface
        .keyboard
        .ime
        .borrow()
        .as_ref()
        .is_some_and(|ime| ime.widget == view.clone().upcast::<gtk::Widget>());
    if same {
        update_ime_area(surface, view);
        return;
    }
    detach_ime(surface);
    let context = gtk::IMMulticontext::new();
    context.set_client_widget(Some(view));
    // The signals outlive this call: they find the surface and the view
    // through weak references.
    let weak_surface = match surface.keyboard.weak.borrow().clone() {
        Some(weak) => weak,
        None => return,
    };
    let weak = view.downgrade();
    let signals = weak_surface.clone();
    context.connect_commit(move |context, text| {
        let (Some(surface), Some(view)) = (signals.upgrade(), weak.upgrade()) else {
            return;
        };
        if !crate::key_focus::is_stop(&view)
            || !surface
                .keyboard
                .ime
                .borrow()
                .as_ref()
                .is_some_and(|ime| ime.context == *context)
        {
            return;
        }
        let had_preedit = !surface.keyboard.preedit.borrow().is_empty();
        surface.keyboard.preedit.borrow_mut().clear();
        if hears(&view, EventKind::TextInput) {
            surface.event(
                view.id(),
                EventKind::TextInput,
                EventPayload::TextInput(TextInputEvent::new(text.to_owned())),
            );
        }
        if had_preedit && hears(&view, EventKind::CompositionChange) {
            surface.event(
                view.id(),
                EventKind::CompositionChange,
                composition(false, ""),
            );
        }
    });
    let weak = view.downgrade();
    let signals = weak_surface;
    context.connect_preedit_changed(move |context| {
        let (Some(surface), Some(view)) = (signals.upgrade(), weak.upgrade()) else {
            return;
        };
        if !crate::key_focus::is_stop(&view)
            || !surface
                .keyboard
                .ime
                .borrow()
                .as_ref()
                .is_some_and(|ime| ime.context == *context)
        {
            return;
        }
        let (text, _, _) = context.preedit_string();
        let text = text.to_string();
        if *surface.keyboard.preedit.borrow() == text {
            return;
        }
        *surface.keyboard.preedit.borrow_mut() = text;
        // Preedit is native input state even when the application only
        // subscribes to committed text. Only the notification is optional.
        if hears(&view, EventKind::CompositionChange) {
            let text = surface.keyboard.preedit.borrow();
            surface.event(
                view.id(),
                EventKind::CompositionChange,
                composition(!text.is_empty(), &text),
            );
        }
    });
    *surface.keyboard.ime.borrow_mut() = Some(Ime {
        context: context.clone(),
        widget: view.clone().upcast(),
        downs: FxHashSet::default(),
    });
    context.focus_in();
    update_ime_area_with(view, &context);
}

/// Removes the focused view's input method context.
fn detach_ime(surface: &Surface) {
    let context = surface
        .keyboard
        .ime
        .borrow()
        .as_ref()
        .map(|ime| ime.context.clone());
    if let Some(context) = context {
        // An eligible view may receive a final commit from focus_out.
        // Keep that context current until its synchronous signals finish.
        context.focus_out();
        context.set_client_widget(None::<&gtk::Widget>);
        surface.keyboard.ime.borrow_mut().take();
    }
    surface.keyboard.preedit.borrow_mut().clear();
}

/// Tells the input method where the focused view's candidate window goes
/// (`imeCursorArea`).
fn update_ime_area(surface: &Surface, view: &NshView) {
    if let Some(ime) = surface.keyboard.ime.borrow().as_ref() {
        update_ime_area_with(view, &ime.context);
    }
}

fn update_ime_area_with(view: &NshView, context: &gtk::IMMulticontext) {
    let rectangle = ime_rectangle(view.state().props.ime_cursor_area);
    context.set_cursor_location(&rectangle);
}

/// Where the candidate window goes: `ime_cursor_area` as it is, in the
/// view's own coordinates (GTK's `set_cursor_location` is relative to the
/// client widget, which is the view), or the default at its origin: a
/// point-sized rectangle there, never a stale one from before the area was
/// cleared.
fn ime_rectangle(area: Option<Bounds<Points>>) -> gdk::Rectangle {
    let Some(area) = area else {
        return gdk::Rectangle::new(0, 0, 1, 1);
    };
    gdk::Rectangle::new(
        area.origin.x.0.round() as i32,
        area.origin.y.0.round() as i32,
        area.size.width.0.round().max(1.0) as i32,
        area.size.height.0.round().max(1.0) as i32,
    )
}

/// Whether an input method context is attached (`gtk_test`).
#[cfg(test)]
pub(crate) fn ime_attached(surface: &Surface) -> bool {
    surface.keyboard.ime.borrow().is_some()
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

/// A composition left pending when the focus goes: the view that was
/// composing hears it end (its `on_composition_change`), and the preedit is
/// cleared.
fn end_composition(surface: &Surface) {
    if surface.keyboard.preedit.borrow().is_empty() {
        return;
    }
    surface.keyboard.preedit.borrow_mut().clear();
    let view = surface
        .keyboard
        .focused
        .borrow()
        .as_ref()
        .and_then(|weak| weak.upgrade());
    if let Some(view) = view
        && hears(&view, EventKind::CompositionChange)
    {
        surface.event(
            view.id(),
            EventKind::CompositionChange,
            composition(false, ""),
        );
    }
}

/// A composition payload (`EventKind::CompositionChange`).
fn composition(composing: bool, text: &str) -> EventPayload {
    EventPayload::Map(vec![
        ("composing", nsh_core::PayloadValue::Bool(composing)),
        ("text", nsh_core::PayloadValue::Str(text.to_owned())),
    ])
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
    use nsh_core::{
        EventKind, EventMask, MountItem, Points, ROOT_ID, ViewKind, ViewProps,
        geometry::{bounds, point, size},
    };

    use super::*;
    use crate::gtk_test::{apply, frame, run, show, surface, view, wait_until};

    /// A focusable view that hears text at `(0, y)`.
    fn text_view(surface: &Rc<Surface>, id: nsh_core::NodeId, y: f32) {
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
                    frame: frame(0.0, y, 100.0, 40.0),
                },
            ],
        );
        let props = ViewProps {
            focusable: Some(true),
            ..Default::default()
        };
        apply(
            surface,
            &[MountItem::ViewProps {
                id,
                props: Box::new(props),
            }],
        );
        let mut events = EventMask::default();
        events.set(EventKind::TextInput, true);
        apply(surface, &[MountItem::Events { id, events }]);
    }

    /// The input method is bound to the view the key goes to, even when
    /// another window's retained focus left it elsewhere, and neither an
    /// inactive view's props nor its events repoint it.
    #[test]
    fn the_input_method_follows_the_views_key_target() {
        run(|| {
            let surface = surface();
            text_view(&surface, 2, 0.0);
            text_view(&surface, 3, 40.0);
            let (a, b) = (view(&surface, 2), view(&surface, 3));
            let window = show(&surface);
            assert!(wait_until(|| a.is_mapped()));
            crate::key_focus::focus(&a);
            assert!(wait_until(|| super::attached_to(&surface, &a)));
            // The same surface's focus moved to b (b's context), while the
            // window kept a as its focus widget: a's key must not go
            // through b's context.
            super::focus_changed(&surface, &window, Some(&b));
            assert!(super::attached_to(&surface, &b));
            assert!(!super::attached_to(&surface, &a));
            assert!(!super::text(&surface, &a, 0, None));
            assert!(
                super::attached_to(&surface, &a),
                "rebound to the view the key went to"
            );
            // Activation points the input method at that window's retained
            // focus widget again, though no focus-widget change came.
            super::focus_changed(&surface, &window, Some(&b));
            super::window_activated(&surface, &window);
            assert!(
                super::attached_to(&surface, &a),
                "activation rebinds to the window's focus"
            );
            // A view that is not the input method's changing leaves it be.
            super::props_changed(&surface, &b);
            super::events_changed(&surface, &b);
            assert!(
                super::attached_to(&surface, &a),
                "another view does not repoint the context"
            );
            window.destroy();
        });
    }

    /// A cleared area returns the candidate window to the view's origin,
    /// not to the rectangle it had before.
    #[test]
    fn the_cursor_area_is_the_views_own_and_clears_to_the_origin() {
        let area = bounds(
            point(Points(12.0), Points(34.0)),
            size(Points(5.0), Points(6.0)),
        );
        let rectangle = super::ime_rectangle(Some(area));
        assert_eq!(
            (
                rectangle.x(),
                rectangle.y(),
                rectangle.width(),
                rectangle.height()
            ),
            (12, 34, 5, 6)
        );
        let none = super::ime_rectangle(None);
        assert_eq!(
            (none.x(), none.y(), none.width(), none.height()),
            (0, 0, 1, 1)
        );
    }

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
            assert_eq!(numeric.key, Key::Character("1".into()));
            assert_eq!(numeric.text.as_deref(), Some("1"));
            let capture =
                nsh_core::KeyCapture::keys([
                    nsh_core::keymap::Keystroke::parse_with("1", false).unwrap()
                ]);
            assert!(capture.captures(&numeric));
            let navigation = key_event(
                &surface,
                &view,
                gdk::Key::KP_End,
                keycode,
                gdk::ModifierType::empty(),
                KeyPhase::Down { repeat: false },
                0,
            );
            assert_eq!(navigation.key, Key::Named(NamedKey::End));
            assert_eq!(navigation.text, None);
            assert!(!capture.captures(&navigation));
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
