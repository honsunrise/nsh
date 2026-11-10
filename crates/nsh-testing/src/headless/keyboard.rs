//! The step `keyboard.press(<keys>)`: a key with its modifiers, as a
//! hardware keyboard gives it, on every platform (`open`'s `platform`: the
//! phones and tablets have one too, a tablet's keyboard, a Chromebook's).
//! The app's shortcuts (`nsh/keymap`) take it first, as the platforms'
//! keymaps do: the newest binding of the keystroke that fires where the
//! focus is reports `onKeyCommand`. Then, on a desktop, the application
//! menu's item with that shortcut is chosen (`onMenu`). Else Tab moves the
//! keyboard focus between the inputs and the views that are its stops
//! (those that take presses, and `focusable` ones that take none) where the
//! platform does (macOS, the web), and, on Linux, a view that has the
//! keyboard focus hears the press (its `on_key_down` and its ancestors'),
//! the capture rule of it or of an enabled view around it deciding whether
//! the platform keeps it (`capture_keys`); Return and Space press the
//! pressable it is on, Shift+F10 asks it for its context menu (the web,
//! Linux), the focused input takes the keys that edit it, and Escape goes
//! back.

use nsh_core::{
    Command, EventKind, EventPayload, Key, KeyEvent, MountBatch, MountItem, NamedKey, NodeId,
    PayloadValue, ROOT_ID, TextInputEvent,
    desktop::{MenuItem, Shortcut},
    keymap::{self, KeyMatch, Keystroke},
    platform::Os,
    tailwind::state::FOCUSED,
};

use super::Headless;

impl Headless {
    /// The steps `keyboard.down(<keys>)` / `keyboard.up(<keys>)`: holds or
    /// lets go of modifier keys (`"mod"`, `"shift"`, `"alt"`, `"ctrl"`,
    /// `"meta"`, joined by `+`), which the presses, drags and hovers until
    /// then carry, as a hand on the keyboard does while the pointer acts.
    pub fn hold_keys(&mut self, keys: &str, down: bool) -> Result<(), String> {
        self.step(format!(
            "keyboard.{}({keys})",
            if down { "down" } else { "up" }
        ));
        let apple = matches!(self.environment.os, Os::MacOs | Os::Ios);
        for name in keys.split('+') {
            let flag = match name.trim().to_ascii_lowercase().as_str() {
                "mod" if apple => &mut self.held.cmd,
                "mod" | "ctrl" | "control" => &mut self.held.ctrl,
                "alt" | "option" => &mut self.held.alt,
                "shift" => &mut self.held.shift,
                "meta" | "cmd" | "command" => &mut self.held.cmd,
                other => {
                    return Err(format!(
                        "`{other}` is no modifier key: mod, shift, alt, ctrl or meta"
                    ));
                }
            };
            *flag = down;
        }
        self.done()
    }

    /// The step `keyboard.press(<keys>)`: presses `keys`, a keystroke as
    /// `nsh/keymap` names it (`"mod+k"`, `"shift+tab"`, `"escape"`; `mod` is
    /// the app's platform's command key).
    ///
    /// The app's shortcuts take the key first (`onKeyCommand`), as the
    /// platforms' keymaps do: those of the focused window and those of no
    /// window, and one that passes the key through lets it go on. Then, on
    /// Linux, a view that has the keyboard focus (`focusable(true)`) hears
    /// the press, and so do its ancestors (`on_key_down`); a capture rule of
    /// it or of an enabled view around it (`capture_keys`), read off the
    /// mounted props, says whether the platform keeps the press: a captured
    /// press types nothing and runs no default. The view hears the `KeyUp`
    /// for its down after the platform has chosen what the key does (the
    /// defaults below, or none when captured), as a platform sends it. An
    /// uncaptured press goes on to the platform's defaults: the
    /// application menu's shortcuts, a window's Escape, the context-menu
    /// keys, Tab, Enter and Space pressing the pressable, text
    /// (`on_text_input`) where the view listens for it, and Escape going
    /// back. Elsewhere the focused input
    /// takes a character, Backspace or the return key, and Escape goes back
    /// as the desktops' back key does when there is something to go back
    /// from.
    pub fn press_key(&mut self, keys: &str) -> Result<(), String> {
        self.step(format!("keyboard.press({keys})"));
        let stroke = Keystroke::parse(keys, self.environment.os).map_err(|e| e.to_string())?;
        let editing = self.screen.focused().is_some();
        // The window the key is pressed in: the focused one, as the
        // desktops match their bindings.
        let window = self
            .windows
            .as_ref()
            .and_then(|w| w.focused())
            .unwrap_or(ROOT_ID);
        // The app's shortcuts take the key first, as the platforms' keymaps
        // do: those of the focused window and those of no window, as the
        // desktops match them. A binding that passes the key through leaves
        // it to the view and the defaults below.
        let found = match self.screen.keymap() {
            Some(keymap) => keymap.find_in(window, stroke, editing, false),
            None => KeyMatch::None,
        };
        if let Some(id) = found.fired() {
            self.engine.control().dispatch_event(
                ROOT_ID,
                EventKind::KeyCommand,
                EventPayload::Value(PayloadValue::Number(f64::from(id))),
            );
        }
        // A binding passing the key through leaves it to what follows, as a
        // platform does: on the screen as it is when the key comes.
        if found.consumed() {
            return self.done();
        }
        // The view the press went down on, with the decision its down
        // carried: the release goes to it once the platform has chosen what
        // the key does, as a platform sends it (a press that reads the key
        // as held must see it down).
        let mut release = None;
        let mut heard = None;
        if !editing
            && self.delivers_raw_input()
            && let Some(view) = self.screen.key_focus()
            && self.screen.accepts_key_focus(view)
        {
            if self.screen.hosts_native(view) {
                return self.done();
            }
            let mut event = view_key_event(&stroke, true);
            let captured = self.screen.captures_key(view, &event);
            event.captured = captured;
            // The platform sends the key whatever the view reports itself:
            // the core bubbles it to its ancestors, so a listener above a
            // view that has none still hears it (the driver's `send` would
            // drop it before that).
            self.engine.control().dispatch_event(
                view,
                EventKind::KeyDown,
                EventPayload::Key(event),
            );
            release = Some((view, captured));
            if !captured {
                heard = Some(view);
            }
        }
        let m = stroke.modifiers;
        // What the platform does with an uncaptured key, in the order the
        // platforms run it; the release follows below.
        if release.is_none_or(|(_, captured)| !captured) {
            'defaults: {
                // Then the application menu's shortcuts choose its items,
                // on a desktop, as the platforms' menus do after the
                // keymap; an item that leaves the key to the focused field
                // does not take it.
                let apple = matches!(self.environment.os, Os::MacOs | Os::Ios);
                if self.environment.desktop()
                    && let Ok(pressed) = Shortcut::parse(keys, apple)
                    && let Some((id, fires)) = self.screen.app_menu().and_then(|items| {
                        MenuItem::find_shortcut(items, &pressed)
                            .map(|a| (a.id, !editing || a.fires_while_editing()))
                    })
                    && fires
                {
                    // A role item (id 0: Quit, Hide) is the system's on a
                    // platform: here it takes the key and does nothing.
                    if id != 0 {
                        self.engine.control().dispatch_event(
                            ROOT_ID,
                            EventKind::Menu,
                            EventPayload::Map(vec![("id", PayloadValue::Number(f64::from(id)))]),
                        );
                    }
                    break 'defaults;
                }
                let plain = !(m.ctrl || m.alt || m.cmd);
                // The key goes to the focused window: Escape closes one of
                // the app's own that says so (`closeOnEscape`), natively.
                if stroke.key == keymap::Key::Escape
                    && plain
                    && !m.shift
                    && self.windows.as_mut().is_some_and(|w| w.escape())
                {
                    self.report_windows();
                    break 'defaults;
                }
                // Shift+F10 asks the focused view for its context menu
                // where views take the keyboard focus and the key is the
                // platform's: the web and Linux (macOS has no such key;
                // Windows focuses fields only). The nearest view around it
                // listening hears `onContextMenu`, with no pointer, at its
                // bottom-start corner; a focused input keeps the key for
                // its own edit menu.
                if matches!(self.environment.os, Os::Web | Os::Linux)
                    && nsh_core::input::opens_context_menu(false, Some(stroke.key), m)
                {
                    if self.screen.focused().is_none()
                        && let Some(from) = self.screen.key_focus()
                    {
                        self.keyboard_context_menu(from, m);
                    }
                    break 'defaults;
                }
                if stroke.key == keymap::Key::Tab && plain && self.tabs_to_views() {
                    self.tab(m.shift);
                    break 'defaults;
                }
                // A pressable stop is pressed by Enter and Space, as the
                // platforms activate it before any text: its keys reach it
                // through the raw notification and the keymap, above.
                if let Some(view) = self.screen.key_focus()
                    && self.screen.focused().is_none()
                    && self.screen.hears(view, EventKind::Press)
                    && plain
                    && !m.shift
                    && self.tabs_to_views()
                    && matches!(stroke.key, keymap::Key::Enter | keymap::Key::Char(' '))
                {
                    for kind in [EventKind::PressDown, EventKind::PressUp, EventKind::Press] {
                        self.send(view, kind, EventPayload::None);
                    }
                    break 'defaults;
                }
                // Text goes to the focused view that listens for it, once
                // no shortcut, capture or activation kept the press: what
                // the key types.
                if let Some(view) = heard
                    && self.screen.hears(view, EventKind::TextInput)
                    && let Some(text) = typed_text(&stroke)
                {
                    self.send(
                        view,
                        EventKind::TextInput,
                        EventPayload::TextInput(TextInputEvent::new(text)),
                    );
                    break 'defaults;
                }
                if let Some(input) = self.screen.focused() {
                    match stroke.key {
                        keymap::Key::Enter if plain => self.return_key(input),
                        keymap::Key::Backspace if plain => self.backspace(input),
                        keymap::Key::Char(c) if plain => {
                            let c = if m.shift { shifted(c) } else { c };
                            self.key(input, c);
                        }
                        _ => {}
                    }
                    break 'defaults;
                }
                if stroke.key == keymap::Key::Escape
                    && plain
                    && !m.shift
                    && self.screen.back_target() != crate::screen::Back::Nothing
                {
                    self.step("back");
                    self.go_back(true);
                }
            }
        }
        // The release follows what the key did, as a platform sends it.
        if let Some((view, captured)) = release {
            let mut up = view_key_event(&stroke, false);
            up.captured = captured;
            self.engine
                .control()
                .dispatch_event(view, EventKind::KeyUp, EventPayload::Key(up));
        }
        self.done()
    }

    /// The keyboard asked view `from` (the keyboard focus) for its context
    /// menu: the nearest view around it listening hears `onContextMenu`
    /// at its bottom-start corner (left to right: the screen keeps no
    /// direction), with no pointer.
    fn keyboard_context_menu(&mut self, from: NodeId, modifiers: nsh_core::keymap::Modifiers) {
        let Some(id) = self.screen.context_menu_target(from) else {
            return;
        };
        let Some(bounds) = self.screen.bounds_in_root(id) else {
            return;
        };
        let position = nsh_core::input::context_menu_anchor(bounds.size, false);
        let event =
            nsh_core::ContextMenuEvent::new(position, bounds.origin + position, modifiers, None);
        self.send(id, EventKind::ContextMenu, EventPayload::ContextMenu(event));
    }

    /// Whether the platform gives a view its own input — a focused view its
    /// keys, a view under the wheel its scrolling: Linux, outside a terminal
    /// (the terminal host reads and delivers its own).
    pub(super) fn delivers_raw_input(&self) -> bool {
        self.environment.os == Os::Linux
            && self.environment.idiom != nsh_core::platform::Idiom::Terminal
    }

    /// Whether Tab stops at views (not only inputs), as on macOS, the web,
    /// Linux and in a terminal (the other platforms move it between fields only).
    fn tabs_to_views(&self) -> bool {
        matches!(self.environment.os, Os::MacOs | Os::Web | Os::Linux)
            || self.environment.idiom == nsh_core::platform::Idiom::Terminal
    }

    /// Tab (`back`: Shift+Tab): the keyboard focus moves to the next stop
    /// ([`crate::screen::Screen::tab_stops`]), around from the last.
    fn tab(&mut self, back: bool) {
        let stops = self.screen.tab_stops();
        let current = self.screen.focused().or(self.screen.key_focus());
        let at = current.and_then(|id| stops.iter().position(|s| *s == id));
        let Some(next) = nsh_core::input::tab_stop(at, stops.len(), back, true) else {
            return;
        };
        self.move_focus(Some(stops[next]));
    }

    /// Moves the keyboard focus to `to` (an input or a view), telling
    /// the view it leaves and the one it reaches.
    ///
    /// The view it reaches reports its `focus:` state before the one it
    /// leaves: the engine may commit between the two messages, and the
    /// core takes a view's focus as the focus whatever came before but
    /// a blur of a view that no longer has it as nothing, so the move is
    /// one change of `focus-within` however the commits fall (a blur
    /// first could leave the views both chains share for a commit).
    pub(super) fn move_focus(&mut self, to: Option<NodeId>) {
        let from = self.screen.focused().or(self.screen.key_focus());
        if from == to {
            return;
        }
        self.screen.set_focused(None);
        self.screen.set_key_focus(None);
        if let Some(new) = to {
            if self.screen.input(new).is_some() {
                self.screen.set_focused(Some(new));
            } else {
                self.screen.set_key_focus(Some(new));
            }
            self.set_focused_state(new, true);
        }
        if let Some(old) = from {
            self.set_focused_state(old, false);
            self.send(old, EventKind::Blur, EventPayload::None);
        }
        if let Some(new) = to {
            self.send(new, EventKind::Focus, EventPayload::None);
        }
    }

    /// The app moved the focus (`focus()`, `blur()`) in `batch`, applied
    /// with input `focused` focused before it: platforms tell the views as
    /// when the user moves it. The screen applied the calls on inputs;
    /// where Tab stops at views that take presses, a call on one of those
    /// moves the keyboard focus as Tab does (the batch's last call counts).
    pub(super) fn focus_calls(&mut self, batch: &MountBatch, focused: Option<NodeId>) {
        let now = self.screen.focused();
        if now != focused {
            // As in `move_focus`: the view the focus reaches reports first.
            let key = now.and_then(|_| self.screen.key_focus());
            if let Some(new) = now {
                self.set_focused_state(new, true);
            }
            if let Some(old) = focused {
                self.set_focused_state(old, false);
                self.send(old, EventKind::Blur, EventPayload::None);
            }
            if let Some(new) = now {
                // The pressable that had the keyboard focus loses it.
                if let Some(key) = key {
                    self.screen.set_key_focus(None);
                    self.set_focused_state(key, false);
                    self.send(key, EventKind::Blur, EventPayload::None);
                }
                self.send(new, EventKind::Focus, EventPayload::None);
            }
        }
        if !self.tabs_to_views() {
            return;
        }
        let mut key = self.screen.key_focus();
        let mut to = None;
        for item in &batch.items {
            let MountItem::Command { id, command } = item else {
                continue;
            };
            let id = *id;
            match command {
                Command::Focus if self.screen.input(id).is_some() => {
                    to = None;
                    key = None;
                }
                Command::Focus if id != ROOT_ID && self.screen.tab_stops().contains(&id) => {
                    to = Some(Some(id));
                    key = Some(id);
                }
                Command::Blur if id != ROOT_ID && key == Some(id) => {
                    to = Some(None);
                    key = None;
                }
                _ => {}
            }
        }
        if let Some(to) = to {
            self.move_focus(to);
        }
    }

    /// A view's or an input's `focus:` state, which follows the keyboard
    /// focus: reported whatever the view tracks, as the platforms report
    /// it (`nsh_core::input::reported_states`; the core works out
    /// `focus-within` from it).
    pub(super) fn set_focused_state(&mut self, id: NodeId, on: bool) {
        if let Some(states) = self.screen.set_state(id, FOCUSED, on) {
            self.engine.control().set_states(id, states);
        }
    }

    /// Backspace in input `id`: its last character goes.
    fn backspace(&mut self, id: nsh_core::NodeId) {
        self.key_press(id, "Backspace");
        let Some(input) = self.screen.input_mut(id) else {
            return;
        };
        if input.text.pop().is_none() {
            return;
        }
        input.count += 1;
        let payload = EventPayload::Text {
            text: input.text.clone(),
            count: input.count,
        };
        self.engine
            .control()
            .dispatch_event(id, EventKind::ChangeText, payload);
        self.report_selection(id);
    }
}

/// The key event of `stroke`, up or down, as a platform reports it on a
/// focused view.
fn view_key_event(stroke: &Keystroke, down: bool) -> KeyEvent {
    let m = stroke.modifiers;
    let key = match stroke.key {
        keymap::Key::Char(' ') => Key::Named(NamedKey::Space),
        keymap::Key::Char(c) => Key::character(c),
        keymap::Key::Enter => Key::Named(NamedKey::Enter),
        keymap::Key::Escape => Key::Named(NamedKey::Escape),
        keymap::Key::Tab => Key::Named(NamedKey::Tab),
        keymap::Key::Backspace => Key::Named(NamedKey::Backspace),
        keymap::Key::Delete => Key::Named(NamedKey::Delete),
        keymap::Key::Insert => Key::Named(NamedKey::Insert),
        keymap::Key::Up => Key::Named(NamedKey::ArrowUp),
        keymap::Key::Down => Key::Named(NamedKey::ArrowDown),
        keymap::Key::Left => Key::Named(NamedKey::ArrowLeft),
        keymap::Key::Right => Key::Named(NamedKey::ArrowRight),
        keymap::Key::Home => Key::Named(NamedKey::Home),
        keymap::Key::End => Key::Named(NamedKey::End),
        keymap::Key::PageUp => Key::Named(NamedKey::PageUp),
        keymap::Key::PageDown => Key::Named(NamedKey::PageDown),
        keymap::Key::F(n) => Key::Named(NamedKey::F(n)),
    };
    let text = if down {
        typed_text(stroke).map(nsh_core::element::SharedString::from)
    } else {
        None
    };
    KeyEvent::new(
        key,
        text,
        keymap::Modifiers {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            cmd: m.cmd,
        },
        u32::from(stroke.key.hid_usage().unwrap_or(0)),
        false,
    )
}

/// The text `stroke` types on the US layout (Shift applied), when it types
/// any.
fn typed_text(stroke: &Keystroke) -> Option<String> {
    let m = stroke.modifiers;
    if m.ctrl || m.alt || m.cmd {
        return None;
    }
    match stroke.key {
        keymap::Key::Char(' ') => Some(" ".to_owned()),
        keymap::Key::Char(c) => Some(if m.shift { shifted(c) } else { c }.to_string()),
        _ => None,
    }
}

/// The character the key typing `c` types with Shift on the US layout.
fn shifted(c: char) -> char {
    const SHIFTED: &[(char, char)] = &[
        ('`', '~'),
        ('1', '!'),
        ('2', '@'),
        ('3', '#'),
        ('4', '$'),
        ('5', '%'),
        ('6', '^'),
        ('7', '&'),
        ('8', '*'),
        ('9', '('),
        ('0', ')'),
        ('-', '_'),
        ('=', '+'),
        ('[', '{'),
        (']', '}'),
        ('\\', '|'),
        (';', ':'),
        ('\'', '"'),
        (',', '<'),
        ('.', '>'),
        ('/', '?'),
    ];
    SHIFTED
        .iter()
        .find(|(plain, _)| *plain == c)
        .map_or(c.to_ascii_uppercase(), |(_, s)| *s)
}
