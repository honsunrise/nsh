//! The app's keyboard on a hardware keyboard (a tablet's, a Chromebook's,
//! an emulator's): the `nsh/keymap` shortcuts and the raw
//! `on_key_down` / `on_key_up` notifications of a focused custom view.
//!
//! `NshView.dispatchKeyEvent` asks here before its focused descendant gets
//! a key (`Native.keyDown` / `Native.keyUp`, on the UI thread): a key that
//! fires a binding is consumed (the focused input never sees it: no caret
//! move, no line break, no typed character) and reported to JS with the
//! root's `KeyCommand` event; JS runs the handler afterwards. Its release
//! is consumed too. The input method sees keys before the view hierarchy
//! does, so a composition keeps its keys. `NshView` holds the focus when
//! no input does, so keys reach it wherever the focus is in it; a text
//! input or hosted view (a web view, a native component) having the focus
//! is "in text": its keys stay its own and no raw event is reported.
//! Android repeats a held key's events itself (their repeat count): a
//! binding fires on them only when it asks to (`repeat`), and a raw event
//! says `repeat: true`.
//!
//! The table lives here rather than in Java (`MountItem::Keymap`); a
//! `capture_keys` policy is marshalled through the existing `ViewProps`
//! batch (`Mounter.java` keeps it) and matched on the UI thread with
//! `KeyCaptures.java`, before Android's defaults: the event carries
//! `captured: true` and `dispatchKeyEvent` returns `true` (no Tab
//! traversal, back, menu, activation or typed character). A release of a
//! key whose press was consumed is consumed too, and carries the same
//! `captured`; releasing, cancelling or losing the window forgets the
//! held keys (no leaked release state).
//!
//! Managed views take real focus, not only the root: `focusable={true}`,
//! or unset on a view that takes presses, is an Android focus stop
//! (`focusable={false}` and disabled views are not). Tab and Shift+Tab
//! walk the managed stops and inputs in native tree order, around the
//! ends; a press on a stop focuses it (while a text input or hosted native
//! view has the focus, only an explicit `focusable` stop takes it, so a
//! tap on a button leaves the input editing); `ref.focus()` /
//! `ref.blur()` focus and blur them; and `FOCUSED`, `onFocus` and
//! `onBlur` reach the core as for an input.
//! `NshView` itself is the root stop when no managed view has the focus
//! (the event goes to [`nsh_core::ROOT_ID`]). An `EditText` or hosted
//! native view (a web view, a video, a native component) keeps its own
//! keys and its own internal focus, and a press on it does not become a
//! managed focus.
//!
//! The raw event is the shared [`nsh_core::KeyEvent`]: `key` is the actual
//! reported Unicode character (preserving case and non-US layouts) or the
//! DOM spelling of a named key (`Enter`, `ArrowLeft`, `Backspace`,
//! `PageUp`, `F5`, `" "`), `text` is its preview (never committed input
//! method text), `physical` is `PhysicalKey::Native(scanCode)` only when
//! Android supplies a scan code, `modifiers` uses the core's wire names
//! and `repeat` is Android's auto-repeat. The event goes to the focused
//! managed view's node id, or [`nsh_core::ROOT_ID`] when `NshView` itself
//! has the focus.
//!
//! Checking a change on an emulator needs a hardware-keyboard-capable AVD
//! and a dev host:
//!
//! ```sh
//! adb shell input keyevent 29               # a: a raw KeyDown and KeyUp
//! adb shell input keyevent --longpress 29   # auto-repeat KeyDowns
//! adb shell input keyevent 61               # Tab: native focus stays default
//! adb shell input keyevent 4                # back: the app's back handling
//! nsh test app.tsx --target android <<'EOF'
//! app.pressKeys("shift+a")                  # KeyEvent with META_SHIFT_ON
//! app.pressKeys("ctrl+tab")                 # keymap first, then capture
//! app.pressKeys("tab")                      # next managed stop or input
//! app.pressKeys("shift+tab")                # previous
//! app.pressKeys("enter")                    # the focused view's Return
//! EOF
//! ```
//!
//! A test app with `focusable` views checks `ref.focus()` / `ref.blur()`
//! and the `FOCUSED`/`onFocus`/`onBlur` reports through the same steps
//! (`app.find(…).focus()` in TSX or the Rust `ElementRef::focus`).
//!
//! The emulator's `input keyevent` supplies no scan code (physical is
//! null; capture still matches from the Android key code), and no input
//! method composes here: the composition path stays InputBox's own.

use std::sync::Arc;

use jni::{
    JNIEnv,
    objects::JClass,
    sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong},
};
use nsh_core::{
    EventKind, EventPayload, KeyEvent, NodeId, PayloadValue, ROOT_ID,
    element::SharedString,
    keymap::{Key, Keymap, Keystroke, Modifiers},
};

/// `KeyEvent.META_*_ON`.
const META_SHIFT_ON: i32 = 0x1;
const META_ALT_ON: i32 = 0x2;
const META_CTRL_ON: i32 = 0x1000;
const META_META_ON: i32 = 0x10000;

/// A view's table, and the keys it consumed that are still down.
#[derive(Default)]
pub(crate) struct KeymapState {
    table: Option<Arc<Keymap>>,
    /// Key codes whose press the table consumed.
    held: Vec<i32>,
    /// Key codes whose press a mounted `capture_keys` rule captured.
    captured: Vec<i32>,
    /// Key codes whose raw `KeyDown` was delivered (their release is too).
    raw: Vec<i32>,
}

/// What a release (or a cancel) found for its key code.
#[derive(Clone, Copy, Default)]
struct Released {
    /// The press was consumed (by the table or a capture rule).
    consumed: bool,
    /// The press was captured.
    captured: bool,
    /// A raw `KeyDown` was delivered.
    raw: bool,
}

impl KeymapState {
    /// `MountItem::Keymap`: replaces the table.
    pub(crate) fn set(&mut self, keymap: Arc<Keymap>) {
        self.table = (!keymap.is_empty()).then_some(keymap);
    }

    /// A new app: no bindings yet.
    pub(crate) fn clear(&mut self) {
        *self = KeymapState::default();
    }

    /// Forgets the keys held (the window lost the focus, the view went
    /// away, or the app restarted): their releases are no press of a key
    /// any more.
    pub(crate) fn reset_keys(&mut self) {
        self.held.clear();
        self.captured.clear();
        self.raw.clear();
    }

    /// Records that the table consumed key `code`.
    fn consume(&mut self, code: i32) {
        push_once(&mut self.held, code);
    }

    /// Records that `code`'s `KeyDown` was captured.
    fn capture(&mut self, code: i32) {
        push_once(&mut self.captured, code);
    }

    /// Records that `code`'s raw `KeyDown` was delivered.
    fn deliver(&mut self, code: i32) {
        push_once(&mut self.raw, code);
    }

    /// Removes key `code` from every held set, reporting what its press
    /// did. A repeated down keeps the state (one release ends every
    /// repeat).
    fn release(&mut self, code: i32) -> Released {
        let released = Released {
            consumed: remove(&mut self.held, code) || self.captured.contains(&code),
            captured: self.captured.contains(&code),
            raw: remove(&mut self.raw, code),
        };
        remove(&mut self.captured, code);
        released
    }

    /// Forgets key `code` without reporting a release: `true` when its
    /// press was consumed.
    fn cancel(&mut self, code: i32) -> bool {
        let held = remove(&mut self.held, code);
        let captured = remove(&mut self.captured, code);
        remove(&mut self.raw, code);
        held || captured
    }
}

fn push_once(keys: &mut Vec<i32>, code: i32) {
    if !keys.contains(&code) {
        keys.push(code);
    }
}

fn remove(keys: &mut Vec<i32>, code: i32) -> bool {
    match keys.iter().position(|held| *held == code) {
        Some(i) => {
            keys.remove(i);
            true
        }
        None => false,
    }
}

/// The keystroke of a key event: key code `code`, `unicode` the character
/// it types without modifiers (`KeyEvent.getUnicodeChar(0)`, 0 for none),
/// `meta` its meta state.
pub(crate) fn keystroke(code: i32, unicode: i32, meta: i32) -> Option<Keystroke> {
    let modifiers = modifiers(meta);
    let key = match Key::from_android_keycode(code)? {
        us @ Key::Char(c) if c != ' ' => match char::from_u32(unicode as u32) {
            Some(c) if c.is_ascii_graphic() => Key::Char(c.to_ascii_lowercase()),
            _ => us,
        },
        named => named,
    };
    Some(Keystroke { modifiers, key })
}

/// The core's modifiers for Android's meta state.
fn modifiers(meta: i32) -> Modifiers {
    Modifiers {
        ctrl: meta & META_CTRL_ON != 0,
        alt: meta & META_ALT_ON != 0,
        shift: meta & META_SHIFT_ON != 0,
        cmd: meta & META_META_ON != 0,
    }
}

/// The capture-key id of a shortcut key, as `KeyCaptures.java` reads it:
/// a non-negative Unicode scalar is a character (the canonical unshifted
/// one), and a negative id names a key (`-1` Enter, `-2` Escape, `-3`
/// Tab, `-4` Backspace, `-5` Delete, `-6` Insert, `-7` Up, `-8` Down,
/// `-9` Left, `-10` Right, `-11` Home, `-12` End, `-13` PageUp,
/// `-14` PageDown, `-100 - n` F(n)).
pub(crate) fn capture_key(key: Key) -> i32 {
    match key {
        Key::Char(c) => c as i32,
        Key::Enter => -1,
        Key::Escape => -2,
        Key::Tab => -3,
        Key::Backspace => -4,
        Key::Delete => -5,
        Key::Insert => -6,
        Key::Up => -7,
        Key::Down => -8,
        Key::Left => -9,
        Key::Right => -10,
        Key::Home => -11,
        Key::End => -12,
        Key::PageUp => -13,
        Key::PageDown => -14,
        Key::F(n) => -100 - i32::from(n),
    }
}

/// A capture rule's modifiers, as `KeyCaptures.java` reads them.
pub(crate) fn capture_modifiers(m: Modifiers) -> i32 {
    (m.ctrl as i32) | (m.alt as i32) << 1 | (m.shift as i32) << 2 | (m.cmd as i32) << 3
}

/// A key went down (`repeat`: an auto-repeat of a held key): whether the
/// app's shortcuts, or a `capture_keys` rule of the focused view, take it.
/// `captured` and `raw` are what `KeyCaptures.java` decided
/// on the UI thread; `focused` is the node that has the focus
/// (`ROOT_ID` for `NshView`).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
extern "system" fn Java_com_longbridge_nsh_Native_keyDown(
    _: JNIEnv<'_>,
    _: JClass<'_>,
    handle: jlong,
    code: jint,
    unicode: jint,
    text: jint,
    meta: jint,
    scan_code: jint,
    repeat: jboolean,
    editing: jboolean,
    focused: jint,
    captured: jboolean,
    raw: jboolean,
) -> jboolean {
    // SAFETY: Java passes its live handle.
    let host = unsafe { crate::host::host(handle) };
    note_key(host, code);
    if shortcut(host, code, unicode, meta, editing != 0, repeat != 0) {
        return JNI_TRUE;
    }
    if raw == 0 || focused <= 0 {
        return JNI_FALSE;
    }
    let captured = captured != 0;
    let event = key_event(code, unicode, text, meta, scan_code, repeat != 0, true, captured);
    {
        let mut state = host.keymap.lock().unwrap_or_else(|e| e.into_inner());
        state.deliver(code);
        if captured {
            state.capture(code);
        }
    }
    host.dispatch(
        focused.max(0) as NodeId,
        EventKind::KeyDown,
        EventPayload::Key(event),
    );
    if captured { JNI_TRUE } else { JNI_FALSE }
}

/// A key came up (`focused`: the node focused now, 0 when a native child
/// owns the keys): the raw `KeyUp` goes there when the press was raw.
#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_keyUp(
    _: JNIEnv<'_>,
    _: JClass<'_>,
    handle: jlong,
    code: jint,
    unicode: jint,
    text: jint,
    meta: jint,
    scan_code: jint,
    focused: jint,
) -> jboolean {
    // SAFETY: Java passes its live handle.
    let host = unsafe { crate::host::host(handle) };
    let released = {
        let mut state = host.keymap.lock().unwrap_or_else(|e| e.into_inner());
        state.release(code)
    };
    if released.raw && focused > 0 {
        let event = key_event(
            code,
            unicode,
            text,
            meta,
            scan_code,
            false,
            false,
            released.captured,
        );
        host.dispatch(
            focused as NodeId,
            EventKind::KeyUp,
            EventPayload::Key(event),
        );
    }
    if released.consumed { JNI_TRUE } else { JNI_FALSE }
}

/// A key's release was cancelled (the system took the gesture): forget it
/// without reporting a `KeyUp`; `true` when its press was consumed.
#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_keyCancel(
    _: JNIEnv<'_>,
    _: JClass<'_>,
    handle: jlong,
    code: jint,
) -> jboolean {
    // SAFETY: Java passes its live handle.
    let host = unsafe { crate::host::host(handle) };
    let consumed = host
        .keymap
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cancel(code);
    if consumed { JNI_TRUE } else { JNI_FALSE }
}

/// The window lost the focus or the app went away: no held key has a
/// release any more.
#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_keyReset(
    _: JNIEnv<'_>,
    _: JClass<'_>,
    handle: jlong,
) {
    // SAFETY: Java passes its live handle.
    let host = unsafe { crate::host::host(handle) };
    host.keymap
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .reset_keys();
}

/// The user gave the surface input with a pointer (a press that focuses a
/// managed view): the focus it moves shows as a pointer's, not the
/// keyboard's.
#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_pointerInput(
    _: JNIEnv<'_>,
    _: JClass<'_>,
    handle: jlong,
) {
    // SAFETY: Java passes its live handle.
    let host = unsafe { crate::host::host(handle) };
    host.input_with(nsh_core::platform::InputModality::Pointer);
}

/// A key with Control comes as a shortcut before the key event (the
/// window's input stage offers it first): only the app's shortcuts, and
/// no raw event.
#[unsafe(no_mangle)]
extern "system" fn Java_com_longbridge_nsh_Native_keyShortcut(
    _: JNIEnv<'_>,
    _: JClass<'_>,
    handle: jlong,
    code: jint,
    unicode: jint,
    meta: jint,
    repeat: jboolean,
    editing: jboolean,
) -> jboolean {
    // SAFETY: Java passes its live handle.
    let host = unsafe { crate::host::host(handle) };
    note_key(host, code);
    if shortcut(host, code, unicode, meta, editing != 0, repeat != 0) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

/// Every key down passes here before it acts: the focus it moves shows
/// (`focus-visible:`). A modifier alone (Alt, Shift, Sym, Control, Caps
/// Lock, Meta, Function: `KeyEvent.KEYCODE_*`) does not count.
fn note_key(host: &crate::host::Host, code: i32) {
    if !matches!(code, 57..=60 | 63 | 113..=119) {
        host.input_with(nsh_core::platform::InputModality::Keyboard);
    }
}

/// Whether the table takes key `code` (firing its binding).
fn shortcut(
    host: &crate::host::Host,
    code: i32,
    unicode: i32,
    meta: i32,
    editing: bool,
    repeat: bool,
) -> bool {
    let Some(stroke) = keystroke(code, unicode, meta) else {
        return false;
    };
    let found = {
        let mut state = host.keymap.lock().unwrap_or_else(|e| e.into_inner());
        let Some(table) = state.table.clone() else {
            return false;
        };
        let found = table.find(stroke, editing, repeat);
        if found.consumed() {
            state.consume(code);
        }
        found
    };
    if let Some(id) = found.fired() {
        host.dispatch(
            ROOT_ID,
            EventKind::KeyCommand,
            EventPayload::Value(PayloadValue::Number(f64::from(id))),
        );
    }
    found.consumed()
}

/// The shared [`KeyEvent`] of a press or release. Android's key code names
/// the non-printing keys; `unicode` is the unshifted character the layout
/// types on it and `text` the character with Shift (what the key reports
/// and previews), so a non-US or shifted character is never forced onto a
/// US layout.
#[allow(clippy::too_many_arguments)]
pub(crate) fn key_event(
    code: i32,
    unicode: i32,
    text: i32,
    meta: i32,
    scan_code: i32,
    repeat: bool,
    down: bool,
    captured: bool,
) -> KeyEvent {
    let modifiers = modifiers(meta);
    let shortcut = modifiers.ctrl || modifiers.alt || modifiers.cmd;
    let character = char::from_u32(text.max(0) as u32)
        .filter(|c| !c.is_control())
        .or_else(|| char::from_u32(unicode.max(0) as u32).filter(|c| !c.is_control()));
    let key = match character {
        // With Control, Alt or Command the key is the unshifted one (the
        // combination is a shortcut, not the character it types).
        Some(c) if !shortcut => c.to_string(),
        _ => match char::from_u32(unicode.max(0) as u32).filter(|c| !c.is_control()) {
            Some(c) => c.to_string(),
            // No character from the layout: the key code's US character
            // (keypad digits and punctuation), else its DOM name.
            None => match Key::from_android_keycode(code) {
                Some(Key::Char(c)) => c.to_string(),
                _ => named_key(code).unwrap_or_else(|| "Unidentified".into()),
            },
        },
    };
    let preview = (down && !shortcut)
        .then_some(character)
        .flatten()
        .map(|c| SharedString::from(c.to_string()));
    let mut event = KeyEvent::default();
    event.key = SharedString::from(key);
    event.text = preview;
    event.modifiers = modifiers;
    event.physical = (scan_code > 0).then_some(nsh_core::PhysicalKey::Native(scan_code as u32));
    event.repeat = repeat;
    event.captured = captured;
    event
}

/// A key that types no character, by Android's key code (DOM spellings).
fn named_key(code: i32) -> Option<String> {
    Some(match code {
        3 => "Home".into(),
        4 => "GoBack".into(),
        19 => "ArrowUp".into(),
        20 => "ArrowDown".into(),
        21 => "ArrowLeft".into(),
        22 => "ArrowRight".into(),
        23 => "Enter".into(),
        24 => "AudioVolumeUp".into(),
        25 => "AudioVolumeDown".into(),
        57 | 58 => "Alt".into(),
        59 | 60 => "Shift".into(),
        61 => "Tab".into(),
        66 | 160 => "Enter".into(),
        67 => "Backspace".into(),
        82 => "ContextMenu".into(),
        85 => "MediaPlayPause".into(),
        86 => "MediaStop".into(),
        87 => "MediaTrackNext".into(),
        88 => "MediaTrackPrevious".into(),
        89 => "MediaRewind".into(),
        90 => "MediaFastForward".into(),
        92 => "PageUp".into(),
        93 => "PageDown".into(),
        111 => "Escape".into(),
        112 => "Delete".into(),
        113 | 114 => "Control".into(),
        115 => "CapsLock".into(),
        116 => "ScrollLock".into(),
        117 | 118 => "Meta".into(),
        119 => "Fn".into(),
        120 => "PrintScreen".into(),
        121 => "Pause".into(),
        122 => "Home".into(),
        123 => "End".into(),
        124 => "Insert".into(),
        126 => "MediaPlay".into(),
        127 => "MediaPause".into(),
        131..=142 => format!("F{}", code - 130),
        143 => "NumLock".into(),
        164 => "AudioVolumeMute".into(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keys_by_what_they_type() {
        let parse = |t| Keystroke::parse(t, nsh_core::platform::Os::Android).ok();
        assert_eq!(keystroke(20, 0, 0), parse("down"));
        assert_eq!(keystroke(66, 0, 0), parse("enter"));
        assert_eq!(keystroke(39, 'k' as i32, META_CTRL_ON), parse("mod+k"));
        assert_eq!(keystroke(76, '/' as i32, META_SHIFT_ON), parse("shift+/"));
        // A layout typing no ASCII there: the US layout's key.
        assert_eq!(keystroke(39, 'л' as i32, 0), parse("k"));
        // A layout typing another letter there (AZERTY's A on Q's key).
        assert_eq!(keystroke(45, 'a' as i32, 0), parse("a"));
        assert_eq!(keystroke(59, 0, META_SHIFT_ON), None);
    }

    #[test]
    fn reports_actual_unicode_named_keys_and_preview_text() {
        let shifted = key_event(29, 'a' as i32, 'A' as i32, META_SHIFT_ON, 0, false, true, false);
        assert_eq!(&*shifted.key, "A");
        assert_eq!(shifted.text.as_deref(), Some("A"));
        assert!(shifted.modifiers.shift && !shifted.modifiers.ctrl);

        // A shifted punctuation key: the character it reports (`?`), while
        // its shortcut key stays `/` (`keystroke`, above).
        let question = key_event(76, '/' as i32, '?' as i32, META_SHIFT_ON, 0, false, true, false);
        assert_eq!(&*question.key, "?");
        assert_eq!(question.text.as_deref(), Some("?"));

        // A non-US layout keeps its character both ways.
        let cyrillic = key_event(39, 'л' as i32, 'Л' as i32, META_SHIFT_ON, 0, false, true, false);
        assert_eq!(&*cyrillic.key, "Л");
        assert_eq!(cyrillic.text.as_deref(), Some("Л"));

        let left = key_event(21, 0, 0, 0, 0, false, true, false);
        assert_eq!(&*left.key, "ArrowLeft");
        assert_eq!(left.text, None);

        let enter = key_event(66, '\n' as i32, '\n' as i32, 0, 0, true, true, false);
        assert_eq!(&*enter.key, "Enter");
        assert_eq!(enter.text, None);
        assert!(enter.repeat);

        // A shortcut reports the unshifted key and no text.
        let all = key_event(29, 'a' as i32, 'A' as i32, META_CTRL_ON | META_SHIFT_ON, 0, false, true, true);
        assert_eq!(&*all.key, "a");
        assert_eq!(all.text, None);
        assert!(all.modifiers.ctrl && all.modifiers.shift && all.captured);

        // The scan code is the physical key only when Android supplies one.
        assert_eq!(key_event(29, 'a' as i32, 'a' as i32, 0, 30, false, true, false).physical,
            Some(nsh_core::PhysicalKey::Native(30)));
        assert_eq!(key_event(29, 'a' as i32, 'a' as i32, 0, 0, false, true, false).physical, None);

        // A release previews nothing and is no auto-repeat.
        let up = key_event(29, 'a' as i32, 'A' as i32, META_SHIFT_ON, 0, false, false, true);
        assert_eq!(&*up.key, "A");
        assert_eq!(up.text, None);
        assert!(!up.repeat && up.captured);
    }

    #[test]
    fn a_release_forgets_only_its_key() {
        let mut state = KeymapState::default();
        state.consume(29);
        state.capture(30);
        state.deliver(30);
        state.deliver(31);

        let released = state.release(30);
        assert!(released.consumed && released.captured && released.raw);
        assert!(state.held.contains(&29));
        assert!(state.raw.contains(&31));

        let other = state.release(29);
        assert!(other.consumed && !other.captured && !other.raw);
        assert!(!state.held.contains(&29));

        // A repeated down does not duplicate the state, and a cancel
        // reports what its press did without a `KeyUp`.
        state.deliver(31);
        state.capture(31);
        assert_eq!(state.raw.iter().filter(|c| **c == 31).count(), 1);
        assert!(state.cancel(31));
        assert!(!state.release(31).consumed);

        state.consume(32);
        state.capture(33);
        state.deliver(33);
        state.reset_keys();
        assert!(state.held.is_empty() && state.captured.is_empty() && state.raw.is_empty());
    }

    #[test]
    fn capture_ids_match_the_java_reader() {
        assert_eq!(capture_key(Key::Char('a')), 'a' as i32);
        assert_eq!(capture_key(Key::Char(' ')), ' ' as i32);
        assert_eq!(capture_key(Key::Enter), -1);
        assert_eq!(capture_key(Key::Escape), -2);
        assert_eq!(capture_key(Key::Tab), -3);
        assert_eq!(capture_key(Key::Backspace), -4);
        assert_eq!(capture_key(Key::Delete), -5);
        assert_eq!(capture_key(Key::Insert), -6);
        assert_eq!(capture_key(Key::Up), -7);
        assert_eq!(capture_key(Key::Down), -8);
        assert_eq!(capture_key(Key::Left), -9);
        assert_eq!(capture_key(Key::Right), -10);
        assert_eq!(capture_key(Key::Home), -11);
        assert_eq!(capture_key(Key::End), -12);
        assert_eq!(capture_key(Key::PageUp), -13);
        assert_eq!(capture_key(Key::PageDown), -14);
        assert_eq!(capture_key(Key::F(1)), -101);
        assert_eq!(capture_key(Key::F(24)), -124);

        let mods = Modifiers {
            ctrl: true,
            alt: true,
            shift: true,
            cmd: true,
        };
        assert_eq!(capture_modifiers(mods), 0b1111);
        assert_eq!(capture_modifiers(Modifiers::default()), 0);
    }
}
