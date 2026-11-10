//! The app's keyboard shortcuts (`nsh/keymap`): the table the engine
//! mounts ([`MountItem::Keymap`](nsh_core::MountItem::Keymap)) becomes a
//! global-scope `gtk::ShortcutController` on the root view of each of the
//! app's windows (the main one and its own), one shortcut per binding,
//! newest first (GTK tries a controller's shortcuts in order).
//!
//! GTK matches the triggers, also on the keys of the layout's Latin group
//! under other layouts, as it does the application's accelerators. A
//! binding's action decides on the spot whether it fires (where the focus
//! is: `KeyBinding::in_text`), reports it to JS with the root's
//! `KeyCommand` event and stops the key; JS runs the handler afterwards.
//! A binding that does not fire, or fires and passes the key through
//! (`KeyBinding::pass_through`), lets GTK try the next shortcut and then
//! the focus widget.
//!
//! A view that hears the raw keys ([`crate::keyboard`]) runs the bindings
//! itself, once, before it is notified ([`raw`]): the newest eligible
//! binding reports its `KeyCommand` there, and its shortcut's action does
//! not report the same press again. Other views keep the controller's
//! path unchanged.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use gtk4::{self as gtk, glib, prelude::*};
use nsh_core::{
    EventKind, EventPayload, NodeId, PayloadValue, ROOT_ID,
    keymap::{Key, KeyBinding, Keymap, Keystroke},
    platform::Os,
};

use crate::{surface::Surface, view::NshView};

/// A surface's shortcut controllers, while it has bindings.
#[derive(Default)]
pub(crate) struct KeymapState {
    /// The bindings.
    keymap: RefCell<Option<Arc<Keymap>>>,
    /// A controller on each window's root view.
    controllers: RefCell<Vec<(glib::WeakRef<NshView>, gtk::ShortcutController)>>,
    /// Each binding's trigger and the binding, newest first, for [`claims`]
    /// (a binding passing its key through claims none) and for the shortcut
    /// dispatch a raw view runs itself ([`raw`]).
    triggers: RefCell<Vec<(gtk::ShortcutTrigger, KeyBinding)>>,
    /// The binding whose key is held down (GDK does not mark auto-repeats):
    /// it does not fire again until a key is released.
    held: Rc<Cell<Option<u32>>>,
    /// The windows' key controllers that see releases, and their windows.
    releases: RefCell<Vec<(glib::WeakRef<gtk::Window>, gtk::EventControllerKey)>>,
    /// A raw view's dispatch ran the app's bindings for the press in
    /// flight ([`raw`]): no shortcut reports or consumes it again, whichever
    /// binding the controller reaches.
    raw_fired: Cell<bool>,
}

/// `MountItem::Keymap`: replaces the surface's shortcuts.
pub(crate) fn set(surface: &Rc<Surface>, keymap: Arc<Keymap>) {
    clear(surface);
    if keymap.is_empty() {
        return;
    }
    // What a press of the menus' shortcuts gives way to (`claims`) and how
    // a raw view's own dispatch runs them ([`raw`]).
    let display = surface.root.display();
    let mut triggers = Vec::new();
    for binding in keymap.bindings().iter().rev() {
        if skipped(binding) {
            continue;
        }
        if let Some(trigger) = trigger(&binding.keystroke, |key| shifted(&display, key))
            .and_then(|t| gtk::ShortcutTrigger::parse_string(&t))
        {
            triggers.push((trigger, *binding));
        }
    }
    *surface.keymap.triggers.borrow_mut() = triggers;
    *surface.keymap.keymap.borrow_mut() = Some(keymap);
    attach(surface, &surface.root);
    for (_, root) in crate::window::roots(surface) {
        attach(surface, &root);
    }
}

/// The dev host's Ctrl+R reloads, whatever the app binds.
fn skipped(binding: &nsh_core::keymap::KeyBinding) -> bool {
    cfg!(feature = "dev") && binding.keystroke.reloads(Os::Linux)
}

/// Gives the root view of a window (`root`) the app's shortcuts: a window
/// of the app's own as it opens, every window as the bindings change.
pub(crate) fn attach(surface: &Rc<Surface>, root: &NshView) {
    let keymap = surface.keymap.keymap.borrow().clone();
    let Some(keymap) = keymap else {
        return;
    };
    let mut controllers = surface.keymap.controllers.borrow_mut();
    controllers.retain(|(r, _)| r.upgrade().is_some());
    if controllers
        .iter()
        .any(|(r, _)| r.upgrade().as_ref() == Some(root))
    {
        return;
    }
    let controller = gtk::ShortcutController::new();
    controller.set_scope(gtk::ShortcutScope::Global);
    // The window runs global controllers in their phase: in the bubble
    // phase the focused field (and its own capture controller) would see
    // the key first, and `when: "always"` could not take Ctrl+A from it.
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let display = root.display();
    // Only the window's own bindings (and JS's, of no window).
    let window = root.id();
    for binding in keymap.bindings().iter().rev() {
        if skipped(binding) || binding.window.is_some_and(|w| w != window) {
            continue;
        }
        let Some(trigger) = trigger(&binding.keystroke, |key| shifted(&display, key)) else {
            continue;
        };
        let weak = Rc::downgrade(surface);
        let (id, in_text, repeat) = (binding.id, binding.in_text, binding.repeat);
        let pass_through = binding.pass_through;
        // The widget is the root view of the window the key is in.
        let action = gtk::CallbackAction::new(move |widget, _| match weak.upgrade() {
            Some(surface) if fires(widget, in_text) => {
                if let Some(window) = widget.root().and_downcast::<gtk::Window>() {
                    watch_releases(&surface, &window);
                }
                if !controller_acts(&surface.keymap) {
                    // The raw view's own lookup ran the newest eligible
                    // binding already: no shortcut reports (or consumes)
                    // this press again.
                    return glib::Propagation::Proceed;
                }
                let held = &surface.keymap.held;
                if repeat || held.get() != Some(id) {
                    held.set(Some(id));
                    surface.event(
                        ROOT_ID,
                        EventKind::KeyCommand,
                        EventPayload::Value(PayloadValue::Number(id as f64)),
                    );
                }
                if pass_through {
                    glib::Propagation::Proceed
                } else {
                    glib::Propagation::Stop
                }
            }
            _ => glib::Propagation::Proceed,
        });
        let Some(trigger) = gtk::ShortcutTrigger::parse_string(&trigger) else {
            continue;
        };
        controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
    }
    root.add_controller(controller.clone());
    controllers.push((root.downgrade(), controller));
}

/// Removes the surface's shortcuts (the app stopped).
pub(crate) fn clear(surface: &Surface) {
    for (root, controller) in surface.keymap.controllers.borrow_mut().drain(..) {
        if let Some(root) = root.upgrade() {
            root.remove_controller(&controller);
        }
    }
    for (window, keys) in surface.keymap.releases.borrow_mut().drain(..) {
        if let Some(window) = window.upgrade() {
            window.remove_controller(&keys);
        }
    }
    surface.keymap.held.set(None);
    surface.keymap.raw_fired.set(false);
    surface.keymap.triggers.borrow_mut().clear();
    surface.keymap.keymap.borrow_mut().take();
}

/// A key pressed without a GDK event (GTK takes no made-up ones; `nsh test
/// --target linux`): the newest binding of `stroke` that fires where the
/// focus is reports it, as its shortcut's action would. Whether it took
/// the key: false if none fired or the one that did passes it through.
#[cfg(feature = "dev")]
pub(crate) fn press(surface: &Surface, stroke: Keystroke) -> bool {
    let keymap = surface.keymap.keymap.borrow().clone();
    let Some(keymap) = keymap else {
        return false;
    };
    let root = crate::window::key_root(surface);
    let fired = keymap.bindings().iter().rev().find(|b| {
        b.keystroke == stroke
            && b.window.is_none_or(|w| w == root.id())
            && fires(root.upcast_ref(), b.in_text)
    });
    if let Some(binding) = fired {
        surface.event(
            ROOT_ID,
            EventKind::KeyCommand,
            EventPayload::Value(PayloadValue::Number(binding.id as f64)),
        );
    }
    fired.is_some_and(|b| !b.pass_through)
}

/// Whether a binding fires for `event` where the focus is: the application
/// menu's shortcuts (`chrome.rs`), whose controller the window runs first,
/// give way to it, as menus' key equivalents do on macOS.
pub(crate) fn claims(surface: &Surface, event: &gtk::gdk::Event) -> bool {
    // The root view of the window the key is in.
    let root = event
        .surface()
        .and_then(|s| crate::window::root_at(surface, &s))
        .unwrap_or_else(|| surface.root.clone());
    surface
        .keymap
        .triggers
        .borrow()
        .iter()
        .any(|(trigger, binding)| {
            !binding.pass_through
                && trigger.trigger(event, false) != gtk::gdk::KeyMatch::None
                && binding.window.is_none_or(|w| w == root.id())
                && fires(root.upcast_ref(), binding.in_text)
        })
}

/// Listens to the releases of `window` (once per window), so that a held
/// key's binding fires again after it is released.
fn watch_releases(surface: &Surface, window: &gtk::Window) {
    let mut releases = surface.keymap.releases.borrow_mut();
    releases.retain(|(w, _)| w.upgrade().is_some());
    if releases
        .iter()
        .any(|(w, _)| w.upgrade().as_ref() == Some(window))
    {
        return;
    }
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let held = surface.keymap.held.clone();
    keys.connect_key_released(move |_, _, _, _| held.set(None));
    window.add_controller(keys.clone());
    releases.push((window.downgrade(), keys));
}

/// What the app's bindings did with a key a raw view hears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawFire {
    /// No binding takes it: the raw view, its capture rule and the
    /// platform's defaults.
    None,
    /// A binding reported it and took it: nothing hears the key after.
    Consumed,
    /// A binding reported it and passes the key on: the raw view, text and
    /// the defaults still get it.
    Passed,
}

/// Forgets the press a previous raw dispatch ran the bindings for, so the
/// shortcut controller acts on the next press again.
pub(crate) fn forget(surface: &Surface) {
    surface.keymap.raw_fired.set(false);
}

/// Runs the app's bindings for a key a raw view hears, before the view is
/// notified: the newest binding whose trigger matches the event and whose
/// window is the key's (its `pass_through` respected), as the shortcut
/// controller would. Reports it once (`KeyCommand`) and says what it did;
/// the controller then leaves the whole press alone ([`controller_acts`]),
/// so an older binding of the same keystroke cannot report or consume it
/// either.
pub(crate) fn raw(surface: &Surface, event: &gtk::gdk::Event, repeat: bool) -> RawFire {
    // This path makes the entire shortcut decision, even when a passing
    // binding's repeat reports nothing. The controller must not retry an
    // older match after it.
    surface.keymap.raw_fired.set(true);
    // The root view of the window the key is in.
    let root = event
        .surface()
        .and_then(|s| crate::window::root_at(surface, &s))
        .unwrap_or_else(|| surface.root.clone());
    let (fired, binding) = {
        let triggers = surface.keymap.triggers.borrow();
        pick(&triggers[..], root.id(), repeat, |trigger| {
            trigger.trigger(event, false) != gtk::gdk::KeyMatch::None
        })
    };
    let Some(binding) = binding else {
        return fired;
    };
    if let Some(window) = root.root().and_downcast::<gtk::Window>() {
        watch_releases(surface, &window);
    }
    surface.keymap.held.set(Some(binding.id));
    surface.event(
        ROOT_ID,
        EventKind::KeyCommand,
        EventPayload::Value(PayloadValue::Number(f64::from(binding.id))),
    );
    fired
}

/// The newest binding of `triggers` (newest first) whose window is `window`
/// and whose trigger `matches`, and what its `pass_through` does with the
/// key; `repeat` is the auto-repeat of a key whose binding fires once
/// (`KeyMatch`'s rules).
fn pick(
    triggers: &[(gtk::ShortcutTrigger, KeyBinding)],
    window: NodeId,
    repeat: bool,
    matches: impl Fn(&gtk::ShortcutTrigger) -> bool,
) -> (RawFire, Option<KeyBinding>) {
    let Some(binding) = triggers
        .iter()
        .filter(|(trigger, binding)| binding.window.is_none_or(|w| w == window) && matches(trigger))
        .map(|(_, binding)| *binding)
        .next()
    else {
        return (RawFire::None, None);
    };
    if repeat && !binding.repeat && binding.pass_through {
        // Its auto-repeat goes on as if there were no binding.
        return (RawFire::None, None);
    }
    if repeat && !binding.repeat {
        // Consumed, reported once already.
        return (RawFire::Consumed, None);
    }
    let fired = if binding.pass_through {
        RawFire::Passed
    } else {
        RawFire::Consumed
    };
    (fired, Some(binding))
}

/// Whether the shortcut controller's action acts on this press at all: not
/// after a raw view's dispatch ran the bindings for it ([`raw`]), whatever
/// binding (`claim`) the controller reaches — the newest eligible one was
/// already chosen, and an older duplicate must not report or consume the
/// key either.
fn controller_acts(state: &KeymapState) -> bool {
    !state.raw_fired.get()
}

/// Whether a binding fires where the focus of the window of root view
/// `root` is: never while it is in another surface, in a text field or
/// hosted widget only when the binding may (`in_text`).
fn fires(root: &gtk::Widget, in_text: bool) -> bool {
    let Some(focus) = root.root().and_then(|r| r.focus()) else {
        return true;
    };
    if !(focus.is_ancestor(root) || &focus == root) {
        return false;
    }
    // A field's text widget, or a hosted widget (a web view).
    let editing = !focus.is::<crate::view::NshView>();
    in_text || !editing
}

/// The keyval that `key` types with Shift on the display's layout (`slash`
/// → `question`, `Tab` → `ISO_Left_Tab`), if it differs.
fn shifted(display: &gtk::gdk::Display, key: gtk::gdk::Key) -> Option<gtk::gdk::Key> {
    let keys = display.map_keyval(key)?;
    let base = keys
        .iter()
        .filter(|k| k.level() == 0)
        .min_by_key(|k| k.group())?;
    let (shifted, ..) = display.translate_key(
        base.keycode(),
        gtk::gdk::ModifierType::SHIFT_MASK,
        base.group(),
    )?;
    (shifted != key && shifted != gtk::gdk::Key::VoidSymbol).then_some(shifted)
}

/// The GTK trigger of `keystroke` (`<Control><Shift>p`). A key event
/// carries the keyval Shift typed, and Shift counts as consumed: a shifted
/// key other than a letter (`shift+/`, `mod+shift+=`, `shift+tab`) is
/// matched by the keyval `shifted` gives (`question`, `plus`,
/// `ISO_Left_Tab`), as GTK's own bindings name it.
fn trigger(
    keystroke: &Keystroke,
    shifted: impl Fn(gtk::gdk::Key) -> Option<gtk::gdk::Key>,
) -> Option<String> {
    let m = keystroke.modifiers;
    let mut out = String::new();
    for (on, name) in [
        (m.ctrl, "<Control>"),
        (m.alt, "<Alt>"),
        (m.shift, "<Shift>"),
        (m.cmd, "<Super>"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    let name = match keystroke.key {
        Key::Char(c) if c.is_ascii_alphanumeric() => c.to_string(),
        Key::Char(c) => match c {
            ' ' => "space",
            '-' => "minus",
            '=' => "equal",
            '[' => "bracketleft",
            ']' => "bracketright",
            '\\' => "backslash",
            ';' => "semicolon",
            '\'' => "apostrophe",
            ',' => "comma",
            '.' => "period",
            '/' => "slash",
            '`' => "grave",
            _ => return None,
        }
        .to_owned(),
        Key::Enter => "Return".to_owned(),
        Key::Escape => "Escape".to_owned(),
        Key::Tab => "Tab".to_owned(),
        Key::Backspace => "BackSpace".to_owned(),
        Key::Delete => "Delete".to_owned(),
        Key::Insert => "Insert".to_owned(),
        Key::Up => "Up".to_owned(),
        Key::Down => "Down".to_owned(),
        Key::Left => "Left".to_owned(),
        Key::Right => "Right".to_owned(),
        Key::Home => "Home".to_owned(),
        Key::End => "End".to_owned(),
        Key::PageUp => "Page_Up".to_owned(),
        Key::PageDown => "Page_Down".to_owned(),
        Key::F(n) => format!("F{n}"),
    };
    let letter = matches!(keystroke.key, Key::Char(c) if c.is_ascii_alphabetic());
    let name = match gtk::gdk::Key::from_name(&name) {
        Some(key) if m.shift && !letter => shifted(key)
            .and_then(|k| k.name())
            .map_or(name, |n| n.to_string()),
        _ => name,
    };
    out.push_str(&name);
    Some(out)
}

#[cfg(test)]
mod tests {
    use gtk4 as gtk;
    use nsh_core::{
        ROOT_ID,
        keymap::{KeyBinding, Keystroke, When},
        platform::Os,
    };

    use crate::gtk_test::run;

    /// The lookup a raw view runs: newest first, the window's own or any
    /// window's, `pass_through` respected, and a held key's auto-repeat
    /// once.
    #[test]
    fn pick_respects_windows_repeats_and_pass_through() {
        run(|| {
            let stroke = |stroke: &str| Keystroke::parse(stroke, Os::Linux).expect("a stroke");
            let trigger =
                |trigger: &str| gtk::ShortcutTrigger::parse_string(trigger).expect("a trigger");
            let triggers = vec![
                (
                    trigger("<Control>a"),
                    KeyBinding::new(1, stroke("mod+a"), When::Always, false).in_window(7),
                ),
                (
                    trigger("<Control>b"),
                    KeyBinding::new(2, stroke("mod+b"), When::Always, false).passing_through(true),
                ),
                (
                    trigger("<Control>c"),
                    KeyBinding::new(3, stroke("mod+c"), When::Always, false),
                ),
            ];
            // The window 7 binding is not the main window's; the newest of
            // the rest passes its key through.
            let (fired, binding) = super::pick(&triggers, ROOT_ID, false, |_| true);
            assert_eq!(fired, super::RawFire::Passed);
            assert_eq!(binding.map(|binding| binding.id), Some(2));
            // A repeat of a binding that fires once: consumed, reported
            // once already.
            let (fired, binding) = super::pick(&triggers, 7, true, |_| true);
            assert_eq!(fired, super::RawFire::Consumed);
            assert!(binding.is_none());
            // A passing binding lets the auto-repeat go on.
            let passing = vec![(
                trigger("<Control>b"),
                KeyBinding::new(2, stroke("mod+b"), When::Always, false).passing_through(true),
            )];
            let (fired, _) = super::pick(&passing, ROOT_ID, true, |_| true);
            assert_eq!(fired, super::RawFire::None);
            // Nothing matches: nothing is reported.
            let (fired, _) = super::pick(&triggers, ROOT_ID, false, |_| false);
            assert_eq!(fired, super::RawFire::None);
            // The click of a binding: the newest match decides, so an older
            // binding of the same keystroke (which the guard keeps the
            // controller from acting on) never takes the key.
            let duplicates = vec![
                (
                    trigger("<Control>a"),
                    KeyBinding::new(4, stroke("mod+a"), When::Always, false).passing_through(true),
                ),
                (
                    trigger("<Control>a"),
                    KeyBinding::new(5, stroke("mod+a"), When::Always, false),
                ),
            ];
            let (fired, binding) = super::pick(&duplicates, ROOT_ID, false, |_| true);
            assert_eq!(fired, super::RawFire::Passed, "the newest passes it on");
            assert_eq!(binding.map(|binding| binding.id), Some(4));
        });
    }

    #[test]
    fn triggers() {
        use gtk4::gdk::Key;
        // The US layout's shifted keys.
        let us = |key| match key {
            Key::slash => Some(Key::question),
            Key::equal => Some(Key::plus),
            Key::_1 => Some(Key::exclam),
            Key::Tab => Some(Key::ISO_Left_Tab),
            _ => None,
        };
        let t = |s| super::trigger(&Keystroke::parse(s, Os::Linux).unwrap(), us).unwrap();
        assert_eq!(t("mod+shift+p"), "<Control><Shift>p");
        assert_eq!(t("alt+enter"), "<Alt>Return");
        assert_eq!(t("cmd+["), "<Super>bracketleft");
        assert_eq!(t("f5"), "F5");
        assert_eq!(t("shift+/"), "<Shift>question");
        assert_eq!(t("mod+shift+="), "<Control><Shift>plus");
        assert_eq!(t("mod+shift+1"), "<Control><Shift>exclam");
        assert_eq!(t("shift+tab"), "<Shift>ISO_Left_Tab");
        assert_eq!(t("mod+="), "<Control>equal");
    }
}
