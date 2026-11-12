//! Raw key transitions for custom views: the `on_key_down` / `on_key_up`
//! notifications a focused view and its ancestors hear, and the capture
//! rules (`capture_keys`) of the views around it.
//!
//! The embedding host reports keys as [`crate::Input::Key`] and
//! [`crate::Input::KeyRepeat`]: a name, the modifiers the terminal
//! reports, and whether the key came up. A release and an auto-repeat are
//! only reported by a terminal speaking the kitty keyboard protocol, which
//! `nsh-tui-host` enables when the terminal answers for it: a terminal
//! without it reports presses alone, and those are presses — this module
//! never makes up a release, a repeat or a hardware key code. A terminal
//! reports no key positions at all, so [`KeyEvent::physical`] is always
//! `None` here.

use nsh_core::{
    EventKind, KeyEvent, NodeId, ViewKind,
    element::SharedString,
    keymap::{Key, Keystroke},
};

use crate::{Modifiers, mount::Mounter};

/// A key transition: the press (the first one, or one of the terminal's
/// auto-repeats) or the release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The key went down; `repeat` marks the terminal's auto-repeats
    /// (kitty's `REPORT_EVENT_TYPES`).
    Down { repeat: bool },
    /// The terminal reported the key coming up.
    Up,
}

impl Phase {
    /// The event kind the transition is reported as.
    pub(crate) fn kind(self) -> EventKind {
        match self {
            Phase::Down { .. } => EventKind::KeyDown,
            Phase::Up => EventKind::KeyUp,
        }
    }
}

/// The event of the key `Input::Key` or `Input::KeyRepeat` names, as the
/// terminal reports it: its logical name, the character it types (a press
/// only) and its modifiers. `stroke` is the key's shortcut keystroke, when
/// the keymap names one, which gives a named key its DOM spelling.
/// [`KeyEvent::captured`] is the caller's to decide from the mounted props;
/// [`KeyEvent::physical`] stays `None`.
pub(crate) fn event(
    key: &str,
    modifiers: Modifiers,
    stroke: Option<&Keystroke>,
    phase: Phase,
) -> KeyEvent {
    let mut event = KeyEvent::default();
    event.key = logical_name(key, stroke);
    event.text = match phase {
        Phase::Down { .. } => typed_text(key, modifiers),
        Phase::Up => None,
    };
    event.modifiers = modifiers;
    event.physical = None;
    event.repeat = matches!(phase, Phase::Down { repeat: true });
    event
}

/// The view a transition is reported on: the focused view, or the text view
/// of a focused link.
pub(crate) fn target(tree: &Mounter, focused: Option<NodeId>) -> Option<NodeId> {
    focused.map(|id| crate::keyboard::host(tree, id))
}

/// Whether `id` or an ancestor hears raw key events or captures keys. Only
/// then is a [`KeyEvent`] built and reported: a view that hears none takes
/// the plain shortcut path.
pub(crate) fn listens(tree: &Mounter, id: NodeId) -> bool {
    Ancestors::of(tree, id).any(|id| {
        tree.get(id).is_some_and(|view| {
            view.events.contains(EventKind::KeyDown)
                || view.events.contains(EventKind::KeyUp)
                || !view.props.key_capture.is_none()
        })
    })
}

/// Whether the enabled view `id` or an enabled ancestor captures the key of
/// `stroke` (`capture_keys`): the terminal then runs no default for it.
/// A rule of [`nsh_core::KeyCapture::All`] takes a key no shortcut names
/// too (`stroke` is `None` then).
pub(crate) fn captures(tree: &Mounter, id: NodeId, stroke: Option<&Keystroke>) -> bool {
    Ancestors::of(tree, id).any(|id| {
        tree.get(id).is_some_and(|view| {
            !view.props.accessibility.state.disabled
                && !view.props.key_capture.is_none()
                && view.props.key_capture.captures_key(stroke)
        })
    })
}

/// The view `id` and the views around it that a key notification reaches:
/// its ancestors up to the sheet it is in, nearest first. A terminal has one
/// window: a sheet (or other `Modal`) is the keyboard boundary its keys stay
/// in, as a window is on the desktops.
///
/// A portaled view (an `Overlay`) is mounted in a root view, but its
/// logical parent is where the app declared it: `ViewProps::key_parent`
/// names that, and interest and capture follow it rather than the mount
/// parent. The override is read through the mounter's view map, so a
/// logical parent is walked like any other.
struct Ancestors<'a> {
    tree: &'a Mounter,
    next: Option<NodeId>,
    /// A logical-parent override may point anywhere; a malformed cycle
    /// must not hang input. Every sound chain visits each view once, so
    /// the views themselves bound it — no per-key traversal of them.
    remaining: usize,
}

impl<'a> Ancestors<'a> {
    fn of(tree: &'a Mounter, id: NodeId) -> Self {
        Self {
            tree,
            next: Some(id),
            remaining: tree.views.len(),
        }
    }
}

impl Iterator for Ancestors<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        self.remaining = self.remaining.checked_sub(1)?;
        let id = self.next?;
        let view = self.tree.get(id)?;
        self.next = if view.kind == ViewKind::Modal {
            None
        } else {
            // The logical parent of a portaled view, else the mounted one.
            view.props.key_parent.or(view.parent)
        };
        Some(id)
    }
}

/// A key's logical name as the other platforms report it: a character exactly
/// as the terminal reports it (its case and the layout's Shift are the
/// source's), and a named key in its DOM spelling (`ArrowLeft` where the
/// keymap says `Left`). A name the keymap does not know stays as given.
fn logical_name(key: &str, stroke: Option<&Keystroke>) -> SharedString {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        // The character the terminal reported.
        (Some(_), None) => SharedString::from(key.to_owned()),
        _ => match stroke.map(|stroke| stroke.key) {
            Some(Key::Char(c)) => SharedString::from(c.to_string()),
            Some(named) => SharedString::from(dom_name(named)),
            None => SharedString::from(key.to_owned()),
        },
    }
}

/// A named key's DOM spelling (`event.key`).
fn dom_name(key: Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Enter => "Enter".into(),
        Key::Escape => "Escape".into(),
        Key::Tab => "Tab".into(),
        Key::Backspace => "Backspace".into(),
        Key::Delete => "Delete".into(),
        Key::Insert => "Insert".into(),
        Key::Up => "ArrowUp".into(),
        Key::Down => "ArrowDown".into(),
        Key::Left => "ArrowLeft".into(),
        Key::Right => "ArrowRight".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::PageUp => "PageUp".into(),
        Key::PageDown => "PageDown".into(),
        Key::F(n) => format!("F{n}"),
    }
}

/// The character a press types, a preview of the key: the terminal's own
/// character for the key (so a shifted one keeps the character it types on
/// the user's layout). Control, Alt and Command make a shortcut of the key,
/// which types nothing.
fn typed_text(key: &str, modifiers: Modifiers) -> Option<SharedString> {
    if modifiers.ctrl || modifiers.alt || modifiers.cmd {
        return None;
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Some(SharedString::from(c.to_string())),
        _ => None,
    }
}
