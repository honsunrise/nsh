//! App-level keyboard shortcuts (`nsh/keymap`): keystrokes parsed from
//! their text (`"mod+k"`, `"ctrl+shift+p"`, `"escape"`, `"f5"`) and the
//! table of a surface's bindings.
//!
//! The syntax is the one `nsh_core::desktop::Shortcut` (menus, `nsh/desktop`)
//! is moving to; the two will share this parser ([`Keystroke::parse`]).
//!
//! The main thread never waits for JS: JS sends the whole table to the
//! platform ([`crate::Prop::Keymap`] on the root, mounted as
//! [`crate::MountItem::Keymap`]), the platform matches each key against it
//! on the spot ([`Keymap::find`]) and consumes the keys it binds, then
//! tells JS which binding matched ([`crate::EventKind::KeyCommand`]).
//!
//! Keys are named by the character they type on the US layout without
//! Shift (`"/"`, `"["`, `"1"`); Shift is a modifier (`"shift+/"` is `?`).
//! Platforms match a key by the character it types without modifiers, and
//! on a layout that types no ASCII there (Cyrillic, Greek, ...) by the US
//! layout's character of the same key, as menus do, so shortcuts work on
//! any layout.

use std::fmt;

use crate::platform::Os;

/// The modifiers of a keystroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    /// Control.
    pub ctrl: bool,
    /// Option on Apple platforms, Alt elsewhere.
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// Command on Apple platforms; the Super (Windows) key elsewhere.
    pub cmd: bool,
}

/// A key, by what it is on the US layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A key typing a printable ASCII character without Shift: a lower
    /// case letter, a digit, punctuation, or `' '` (space).
    Char(char),
    /// Return / Enter (the keypad's too).
    Enter,
    /// Escape.
    Escape,
    /// Tab.
    Tab,
    /// Backspace (the Mac's Delete).
    Backspace,
    /// Forward delete.
    Delete,
    /// Insert (Help on Apple keyboards).
    Insert,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// A function key, F1 to F24.
    F(u8),
}

/// The US layout's punctuation keys' Windows virtual-key codes
/// (`VK_OEM_*`).
const OEM_KEYS: &[(u16, char)] = &[
    (0xBA, ';'),
    (0xBB, '='),
    (0xBC, ','),
    (0xBD, '-'),
    (0xBE, '.'),
    (0xBF, '/'),
    (0xC0, '`'),
    (0xDB, '['),
    (0xDC, '\\'),
    (0xDD, ']'),
    (0xDE, '\''),
];

impl Key {
    /// The key of Windows virtual-key code `vk` (`VK_*`, which the Windows
    /// and GPUI backends get); punctuation by the US layout's keys.
    pub fn from_virtual_key(vk: u16) -> Option<Key> {
        Some(match vk {
            0x08 => Key::Backspace,
            0x09 => Key::Tab,
            0x0D => Key::Enter,
            0x1B => Key::Escape,
            0x20 => Key::Char(' '),
            0x21 => Key::PageUp,
            0x22 => Key::PageDown,
            0x23 => Key::End,
            0x24 => Key::Home,
            0x25 => Key::Left,
            0x26 => Key::Up,
            0x27 => Key::Right,
            0x28 => Key::Down,
            0x2D => Key::Insert,
            0x2E => Key::Delete,
            // `0`-`9`, `A`-`Z`.
            0x30..=0x39 => Key::Char(char::from(vk as u8)),
            0x41..=0x5A => Key::Char(char::from(vk as u8).to_ascii_lowercase()),
            // The keypad's digits.
            0x60..=0x69 => Key::Char(char::from(b'0' + (vk - 0x60) as u8)),
            // F1 to F24.
            0x70..=0x87 => Key::F((vk - 0x6F) as u8),
            _ => {
                return OEM_KEYS
                    .iter()
                    .find(|(k, _)| *k == vk)
                    .map(|&(_, c)| Key::Char(c));
            }
        })
    }

    /// The Windows virtual-key code of this key (the main keyboard's, not
    /// the keypad's): what a key event for it carries.
    pub fn virtual_key(self) -> Option<u16> {
        Some(match self {
            Key::Backspace => 0x08,
            Key::Tab => 0x09,
            Key::Enter => 0x0D,
            Key::Escape => 0x1B,
            Key::Char(' ') => 0x20,
            Key::PageUp => 0x21,
            Key::PageDown => 0x22,
            Key::End => 0x23,
            Key::Home => 0x24,
            Key::Left => 0x25,
            Key::Up => 0x26,
            Key::Right => 0x27,
            Key::Down => 0x28,
            Key::Insert => 0x2D,
            Key::Delete => 0x2E,
            Key::Char(c @ '0'..='9') => c as u16,
            Key::Char(c @ 'a'..='z') => c.to_ascii_uppercase() as u16,
            Key::F(n @ 1..=24) => 0x6F + u16::from(n),
            Key::Char(c) => return OEM_KEYS.iter().find(|(_, k)| *k == c).map(|&(vk, _)| vk),
            Key::F(_) => return None,
        })
    }

    /// The key of USB HID keyboard usage `usage` (usage page 7, which
    /// UIKit's `UIKey.keyCode` is): the key's place on the US layout,
    /// whatever layout types on it; the keypad's digits and Enter too.
    pub fn from_hid_usage(usage: u16) -> Option<Key> {
        Some(match usage {
            4..=29 => Key::Char(char::from(b'a' + (usage - 4) as u8)),
            30..=38 => Key::Char(char::from(b'1' + (usage - 30) as u8)),
            39 => Key::Char('0'),
            40 | 88 => Key::Enter,
            41 => Key::Escape,
            42 => Key::Backspace,
            43 => Key::Tab,
            44 => Key::Char(' '),
            58..=69 => Key::F((usage - 57) as u8),
            73 => Key::Insert,
            74 => Key::Home,
            75 => Key::PageUp,
            76 => Key::Delete,
            77 => Key::End,
            78 => Key::PageDown,
            79 => Key::Right,
            80 => Key::Left,
            81 => Key::Down,
            82 => Key::Up,
            89..=97 => Key::Char(char::from(b'1' + (usage - 89) as u8)),
            98 => Key::Char('0'),
            104..=115 => Key::F((usage - 91) as u8),
            _ => {
                return HID_KEYS
                    .iter()
                    .find(|(u, _)| *u == usage)
                    .map(|&(_, c)| Key::Char(c));
            }
        })
    }

    /// The USB HID keyboard usage of this key (the main keyboard's, not
    /// the keypad's): what a hardware keyboard sends for it.
    pub fn hid_usage(self) -> Option<u16> {
        Some(match self {
            Key::Char(c @ 'a'..='z') => 4 + u16::from(c as u8 - b'a'),
            Key::Char(c @ '1'..='9') => 30 + u16::from(c as u8 - b'1'),
            Key::Char('0') => 39,
            Key::Enter => 40,
            Key::Escape => 41,
            Key::Backspace => 42,
            Key::Tab => 43,
            Key::Char(' ') => 44,
            Key::F(n @ 1..=12) => 57 + u16::from(n),
            Key::F(n @ 13..=24) => 91 + u16::from(n),
            Key::Insert => 73,
            Key::Home => 74,
            Key::PageUp => 75,
            Key::Delete => 76,
            Key::End => 77,
            Key::PageDown => 78,
            Key::Right => 79,
            Key::Left => 80,
            Key::Down => 81,
            Key::Up => 82,
            Key::Char(c) => return HID_KEYS.iter().find(|(_, k)| *k == c).map(|&(u, _)| u),
            Key::F(_) => return None,
        })
    }

    /// The key of Android key code `code` (`KeyEvent.KEYCODE_*`); the
    /// keypad's digits and Enter too.
    pub fn from_android_keycode(code: i32) -> Option<Key> {
        Some(match code {
            7..=16 => Key::Char(char::from(b'0' + (code - 7) as u8)),
            29..=54 => Key::Char(char::from(b'a' + (code - 29) as u8)),
            19 => Key::Up,
            20 => Key::Down,
            21 => Key::Left,
            22 => Key::Right,
            61 => Key::Tab,
            62 => Key::Char(' '),
            66 | 160 => Key::Enter,
            67 => Key::Backspace,
            92 => Key::PageUp,
            93 => Key::PageDown,
            111 => Key::Escape,
            112 => Key::Delete,
            122 => Key::Home,
            123 => Key::End,
            124 => Key::Insert,
            131..=142 => Key::F((code - 130) as u8),
            144..=153 => Key::Char(char::from(b'0' + (code - 144) as u8)),
            _ => {
                return ANDROID_KEYS
                    .iter()
                    .find(|(k, _)| *k == code)
                    .map(|&(_, c)| Key::Char(c));
            }
        })
    }

    /// The Android key code of this key (`KeyEvent.KEYCODE_*`, the main
    /// keyboard's): what a key event for it carries.
    pub fn android_keycode(self) -> Option<i32> {
        Some(match self {
            Key::Char(c @ '0'..='9') => 7 + i32::from(c as u8 - b'0'),
            Key::Char(c @ 'a'..='z') => 29 + i32::from(c as u8 - b'a'),
            Key::Up => 19,
            Key::Down => 20,
            Key::Left => 21,
            Key::Right => 22,
            Key::Tab => 61,
            Key::Char(' ') => 62,
            Key::Enter => 66,
            Key::Backspace => 67,
            Key::PageUp => 92,
            Key::PageDown => 93,
            Key::Escape => 111,
            Key::Delete => 112,
            Key::Home => 122,
            Key::End => 123,
            Key::Insert => 124,
            Key::F(n @ 1..=12) => 130 + i32::from(n),
            Key::Char(c) => return ANDROID_KEYS.iter().find(|(_, k)| *k == c).map(|&(k, _)| k),
            Key::F(_) => return None,
        })
    }

    /// The key of a Windows scan code (`code = scan | (extended << 8)`, as
    /// `WM_KEYDOWN`'s LPARAM has it: bits 16–23 and the `0xE0` bit 24), by
    /// the key's place on the US layout: what a shortcut matches on any
    /// layout. The keypad's digits and Enter too; not the virtual key.
    pub fn from_windows_scan_code(code: u16) -> Option<Key> {
        let extended = code & 0x100 != 0;
        let scan = code & 0xFF;
        if extended {
            return Some(match scan {
                0x1C => Key::Enter,
                0x47 => Key::Home,
                0x48 => Key::Up,
                0x49 => Key::PageUp,
                0x4B => Key::Left,
                0x4D => Key::Right,
                0x4F => Key::End,
                0x50 => Key::Down,
                0x51 => Key::PageDown,
                0x52 => Key::Insert,
                0x53 => Key::Delete,
                // Another layout's or a media key: the reported key names
                // it.
                _ => return None,
            });
        }
        Some(match scan {
            0x01 => Key::Escape,
            0x0E => Key::Backspace,
            0x0F => Key::Tab,
            0x1C => Key::Enter,
            0x39 => Key::Char(' '),
            0x3B..=0x44 => Key::F((scan - 0x3A) as u8),
            0x57 => Key::F(11),
            0x58 => Key::F(12),
            // The keypad's digits.
            0x47 => Key::Char('7'),
            0x48 => Key::Char('8'),
            0x49 => Key::Char('9'),
            0x4B => Key::Char('4'),
            0x4C => Key::Char('5'),
            0x4D => Key::Char('6'),
            0x4F => Key::Char('1'),
            0x50 => Key::Char('2'),
            0x51 => Key::Char('3'),
            0x52 => Key::Char('0'),
            _ => {
                return WINDOWS_SCAN_KEYS
                    .iter()
                    .find(|(scan_code, _)| *scan_code == scan)
                    .map(|&(_, c)| Key::Char(c));
            }
        })
    }

    /// The key of Linux scan code `code` (`input-event-codes.h`'s `KEY_*`:
    /// evdev's, which Android's `KeyEvent.getScanCode` reports), by the
    /// key's place on the US layout: what a shortcut matches on any layout.
    /// The keypad's digits and Enter too.
    pub fn from_scan_code(code: u16) -> Option<Key> {
        Some(match code {
            1 => Key::Escape,
            14 => Key::Backspace,
            15 => Key::Tab,
            28 | 96 => Key::Enter,
            102 => Key::Home,
            103 => Key::Up,
            104 => Key::PageUp,
            105 => Key::Left,
            106 => Key::Right,
            107 => Key::End,
            108 => Key::Down,
            109 => Key::PageDown,
            110 => Key::Insert,
            111 => Key::Delete,
            59..=68 => Key::F((code - 58) as u8),
            87 => Key::F(11),
            88 => Key::F(12),
            183..=194 => Key::F((code - 170) as u8),
            // The keypad's digits.
            71 => Key::Char('7'),
            72 => Key::Char('8'),
            73 => Key::Char('9'),
            75 => Key::Char('4'),
            76 => Key::Char('5'),
            77 => Key::Char('6'),
            79 => Key::Char('1'),
            80 => Key::Char('2'),
            81 => Key::Char('3'),
            82 => Key::Char('0'),
            57 => Key::Char(' '),
            _ => {
                return SCAN_KEYS
                    .iter()
                    .find(|(scan, _)| *scan == code)
                    .map(|&(_, c)| Key::Char(c));
            }
        })
    }

    /// The key of AppKit's virtual key code `code` (`kVK_ANSI_*`,
    /// `NSEvent.keyCode`), by the key's place on the US layout: what a
    /// shortcut matches on any layout. Code 0 is the ANSI `A` key; a code
    /// no key has (`0xffff`) gives `None`.
    pub fn from_appkit_keycode(code: u16) -> Option<Key> {
        Some(match code {
            36 | 76 => Key::Enter,
            48 => Key::Tab,
            49 => Key::Char(' '),
            51 => Key::Backspace,
            53 => Key::Escape,
            114 => Key::Insert,
            115 => Key::Home,
            116 => Key::PageUp,
            117 => Key::Delete,
            119 => Key::End,
            121 => Key::PageDown,
            123 => Key::Left,
            124 => Key::Right,
            125 => Key::Down,
            126 => Key::Up,
            _ => match APPKIT_FUNCTION_KEYS.iter().position(|&k| k == code) {
                Some(index) => Key::F((index + 1) as u8),
                None => {
                    Key::Char(*APPKIT_KEYS.get(usize::from(code)).filter(|&&c| c != 0)? as char)
                }
            },
        })
    }

    /// The key of the DOM's `KeyboardEvent.code` (`"KeyA"`, `"Digit1"`,
    /// `"ArrowLeft"`, `"F5"`), by the key's place on the US layout: what a
    /// shortcut matches on any layout. The keypad's digits too.
    pub fn from_code(code: &str) -> Option<Key> {
        Some(match code {
            "Enter" | "NumpadEnter" => Key::Enter,
            "Escape" => Key::Escape,
            "Tab" => Key::Tab,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            "Insert" => Key::Insert,
            "Home" => Key::Home,
            "End" => Key::End,
            "PageUp" => Key::PageUp,
            "PageDown" => Key::PageDown,
            "ArrowUp" => Key::Up,
            "ArrowDown" => Key::Down,
            "ArrowLeft" => Key::Left,
            "ArrowRight" => Key::Right,
            "Space" => Key::Char(' '),
            _ => return dom_code_key(code),
        })
    }

    /// The key of a raw logical key name as the sources report it, in the
    /// DOM's spelling for named keys (`"Enter"`, `"ArrowLeft"`, `"F5"`,
    /// `" "`) or a single printable ASCII character (letters in lower
    /// case): the fallback for a source that reports no usable physical
    /// code. It does not rewrite another layout's characters into ASCII.
    pub fn from_name(name: &str) -> Option<Key> {
        if let Some(&(_, key)) = NAMED
            .iter()
            .find(|(named, _)| name.eq_ignore_ascii_case(named))
        {
            return Some(key);
        }
        if name == " " {
            return Some(Key::Char(' '));
        }
        if let Some(rest) = name.strip_prefix('f').or_else(|| name.strip_prefix('F'))
            && !rest.starts_with('0')
            && let Ok(number) = rest.parse::<u8>()
            && (1..=24).contains(&number)
        {
            return Some(Key::F(number));
        }
        let mut chars = name.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_graphic() => Some(Key::Char(c.to_ascii_lowercase())),
            _ => None,
        }
    }
}

/// The key of the character, digit, function or punctuation key a DOM
/// `KeyboardEvent.code` names (`"KeyA"`, `"Digit1"`, `"F5"`), else none.
fn dom_code_key(code: &str) -> Option<Key> {
    if let Some(name) = code.strip_prefix("Key") {
        let mut chars = name.chars();
        return match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(Key::Char(c.to_ascii_lowercase())),
            _ => None,
        };
    }
    if let Some(digit) = code
        .strip_prefix("Digit")
        .or_else(|| code.strip_prefix("Numpad"))
        .filter(|d| d.len() == 1)
        .and_then(|d| d.chars().next())
        .filter(char::is_ascii_digit)
    {
        return Some(Key::Char(digit));
    }
    if let Some(number) = code
        .strip_prefix('F')
        .filter(|n| !n.starts_with('0'))
        .and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&number)
    {
        return Some(Key::F(number));
    }
    DOM_CODES
        .iter()
        .find(|(name, _)| *name == code)
        .map(|&(_, c)| Key::Char(c))
}

/// The US layout's keys by Windows scan code (set 1), the ones that type a
/// character without Shift.
const WINDOWS_SCAN_KEYS: &[(u16, char)] = &[
    (0x02, '1'),
    (0x03, '2'),
    (0x04, '3'),
    (0x05, '4'),
    (0x06, '5'),
    (0x07, '6'),
    (0x08, '7'),
    (0x09, '8'),
    (0x0A, '9'),
    (0x0B, '0'),
    (0x0C, '-'),
    (0x0D, '='),
    (0x10, 'q'),
    (0x11, 'w'),
    (0x12, 'e'),
    (0x13, 'r'),
    (0x14, 't'),
    (0x15, 'y'),
    (0x16, 'u'),
    (0x17, 'i'),
    (0x18, 'o'),
    (0x19, 'p'),
    (0x1A, '['),
    (0x1B, ']'),
    (0x1E, 'a'),
    (0x1F, 's'),
    (0x20, 'd'),
    (0x21, 'f'),
    (0x22, 'g'),
    (0x23, 'h'),
    (0x24, 'j'),
    (0x25, 'k'),
    (0x26, 'l'),
    (0x27, ';'),
    (0x28, '\''),
    (0x29, '`'),
    (0x2B, '\\'),
    (0x2C, 'z'),
    (0x2D, 'x'),
    (0x2E, 'c'),
    (0x2F, 'v'),
    (0x30, 'b'),
    (0x31, 'n'),
    (0x32, 'm'),
    (0x33, ','),
    (0x34, '.'),
    (0x35, '/'),
];

/// The US layout's keys by Linux scan code (`KEY_*`), the ones that type a
/// character without Shift.
const SCAN_KEYS: &[(u16, char)] = &[
    (2, '1'),
    (3, '2'),
    (4, '3'),
    (5, '4'),
    (6, '5'),
    (7, '6'),
    (8, '7'),
    (9, '8'),
    (10, '9'),
    (11, '0'),
    (12, '-'),
    (13, '='),
    (16, 'q'),
    (17, 'w'),
    (18, 'e'),
    (19, 'r'),
    (20, 't'),
    (21, 'y'),
    (22, 'u'),
    (23, 'i'),
    (24, 'o'),
    (25, 'p'),
    (26, '['),
    (27, ']'),
    (30, 'a'),
    (31, 's'),
    (32, 'd'),
    (33, 'f'),
    (34, 'g'),
    (35, 'h'),
    (36, 'j'),
    (37, 'k'),
    (38, 'l'),
    (39, ';'),
    (40, '\''),
    (41, '`'),
    (43, '\\'),
    (44, 'z'),
    (45, 'x'),
    (46, 'c'),
    (47, 'v'),
    (48, 'b'),
    (49, 'n'),
    (50, 'm'),
    (51, ','),
    (52, '.'),
    (53, '/'),
];

/// The ANSI keyboard's characters, by AppKit's virtual key code
/// (`kVK_ANSI_*`; `\0`: not a character key). Code 0 is the `A` key.
const APPKIT_KEYS: &[u8; 51] = b"asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm.\0\0`";

/// AppKit's virtual key codes of F1 to F20.
const APPKIT_FUNCTION_KEYS: [u16; 20] = [
    122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90,
];

/// The US layout's punctuation and digit-row keys by the DOM's
/// `KeyboardEvent.code`.
const DOM_CODES: &[(&str, char)] = &[
    ("Semicolon", ';'),
    ("Equal", '='),
    ("Comma", ','),
    ("Minus", '-'),
    ("Period", '.'),
    ("Slash", '/'),
    ("Backquote", '`'),
    ("BracketLeft", '['),
    ("Backslash", '\\'),
    ("BracketRight", ']'),
    ("Quote", '\''),
];

/// The US layout's punctuation keys' USB HID usages.
const HID_KEYS: &[(u16, char)] = &[
    (45, '-'),
    (46, '='),
    (47, '['),
    (48, ']'),
    (49, '\\'),
    (51, ';'),
    (52, '\''),
    (53, '`'),
    (54, ','),
    (55, '.'),
    (56, '/'),
];

/// The US layout's punctuation keys' Android key codes.
const ANDROID_KEYS: &[(i32, char)] = &[
    (55, ','),
    (56, '.'),
    (68, '`'),
    (69, '-'),
    (70, '='),
    (71, '['),
    (72, ']'),
    (73, '\\'),
    (74, ';'),
    (75, '\''),
    (76, '/'),
];

/// One key with its modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Keystroke {
    /// Modifiers held.
    pub modifiers: Modifiers,
    /// The key.
    pub key: Key,
}

/// Why a keystroke's text is not one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeystrokeError(pub String);

impl fmt::Display for KeystrokeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KeystrokeError {}

/// Characters typed with Shift on the US layout, and the key typing them.
const SHIFTED: &[(char, char)] = &[
    ('~', '`'),
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
];

/// Named keys, by their names (lower case).
const NAMED: &[(&str, Key)] = &[
    ("enter", Key::Enter),
    ("return", Key::Enter),
    ("escape", Key::Escape),
    ("esc", Key::Escape),
    ("tab", Key::Tab),
    ("space", Key::Char(' ')),
    ("backspace", Key::Backspace),
    ("delete", Key::Delete),
    ("del", Key::Delete),
    ("insert", Key::Insert),
    ("up", Key::Up),
    ("arrowup", Key::Up),
    ("down", Key::Down),
    ("arrowdown", Key::Down),
    ("left", Key::Left),
    ("arrowleft", Key::Left),
    ("right", Key::Right),
    ("arrowright", Key::Right),
    ("home", Key::Home),
    ("end", Key::End),
    ("pageup", Key::PageUp),
    ("pagedown", Key::PageDown),
    // The plus key is the one typing `=`, as on the US layout.
    ("plus", Key::Char('=')),
    ("minus", Key::Char('-')),
];

fn error(text: &str, reason: impl fmt::Display) -> KeystrokeError {
    KeystrokeError(format!("invalid keystroke {text:?}: {reason}"))
}

impl Keystroke {
    /// Parses `text`: modifiers and a key joined by `+` (`"mod+shift+k"`,
    /// `"mod+-"` for Command and minus), in any case (lowercase is the
    /// style; a capital letter is the same key, not Shift). Modifiers:
    /// `ctrl`, `alt` (`option`), `shift`, `cmd` (`super`, `win`) and `mod`,
    /// which is Command on Apple platforms (`os`, [`apple_keys`]) and
    /// Control elsewhere. Keys: a character of the US layout typed without Shift (`k`, `1`,
    /// `/`, `[`, `=`: the plus key is `=`, `shift+=` with Shift), `space`,
    /// `enter`, `escape`, `tab`, `backspace`, `delete`, the arrows `up` /
    /// `down` / `left` / `right`, `home`, `end`, `pageup`, `pagedown`, `f1`
    /// to `f24`.
    pub fn parse(text: &str, os: Os) -> Result<Keystroke, KeystrokeError> {
        Keystroke::parse_with(text, apple_keys(os, false))
    }

    /// Parses `text` as [`Keystroke::parse`] does, with Apple's conventions
    /// (`mod` is Command) or not: a browser decides by the platform it runs
    /// on ([`apple_keys`]).
    pub fn parse_with(text: &str, apple: bool) -> Result<Keystroke, KeystrokeError> {
        // Case does not matter (lowercase is the style); messages quote
        // the text as written and suggest lowercase.
        let lower = text.to_ascii_lowercase();
        let (mods, key) = split(text, &lower)?;
        let mut modifiers = Modifiers::default();
        for name in mods {
            let flag = modifier(&mut modifiers, name, apple).map_err(|why| error(text, why))?;
            if *flag {
                return Err(error(text, format_args!("{name:?} twice")));
            }
            *flag = true;
        }
        Ok(Keystroke {
            modifiers,
            key: parse_key(text, key)?,
        })
    }

    /// Why the platform keeps this keystroke for itself on `os` (quitting,
    /// hiding, minimizing the app or making it full screen), if it does:
    /// such keystrokes cannot be bound.
    pub fn reserved(&self, os: Os) -> Option<&'static str> {
        let m = self.modifiers;
        let only = |ctrl, alt, cmd| m.ctrl == ctrl && m.alt == alt && m.cmd == cmd && !m.shift;
        match os {
            Os::MacOs | Os::Ios => match self.key {
                Key::Char('q') if only(false, false, true) => Some("it quits the app"),
                Key::Char('h') if only(false, false, true) => Some("it hides the app"),
                Key::Char('h') if only(false, true, true) => Some("it hides the other apps"),
                Key::Char('m') if only(false, false, true) => Some("it minimizes the window"),
                Key::Char('f') if only(true, false, true) => Some("it toggles full screen"),
                _ => None,
            },
            Os::Linux | Os::Windows | Os::Android => match self.key {
                Key::Char('q') if only(true, false, false) => Some("it quits the app"),
                Key::F(4) if only(false, true, false) => Some("it closes the window"),
                Key::F(11) if m == Modifiers::default() => Some("it toggles full screen"),
                _ => None,
            },
            // The browser keeps its own keys: the page never sees them.
            Os::Web => None,
        }
    }

    /// Whether this is the development hosts' reload key on `os` (`mod+r`:
    /// Cmd+R, Ctrl+R): built with their `dev` feature, platforms leave it
    /// to the dev server client, whatever the app binds.
    pub fn reloads(&self, os: Os) -> bool {
        let m = self.modifiers;
        let apple = apple_keys(os, false);
        self.key == Key::Char('r') && !m.shift && !m.alt && m.cmd == apple && m.ctrl != apple
    }

    /// Whether a text input that has the keyboard focus uses this
    /// keystroke itself: keys that type, move the caret or delete, and the
    /// editing shortcuts (select all, cut, copy, paste, undo, redo).
    /// Bindings of these keystrokes do not fire while text is edited,
    /// unless they ask to ([`When::Always`]).
    pub fn edits_text(&self) -> bool {
        let m = self.modifiers;
        match self.key {
            Key::F(_) => false,
            Key::Char(c) => !(m.cmd || m.ctrl) || matches!(c, 'a' | 'c' | 'v' | 'x' | 'y' | 'z'),
            _ => true,
        }
    }
}

/// The modifiers and the key of `text` (`"mod+shift+k"`).
fn split<'a>(text: &str, lower: &'a str) -> Result<(Vec<&'a str>, &'a str), KeystrokeError> {
    if text.is_empty() {
        return Err(error(text, "empty"));
    }
    if text.contains(char::is_whitespace) {
        return Err(error(
            text,
            "one keystroke per binding (key sequences such as \"mod+k mod+s\" are not supported)",
        ));
    }
    if lower == "+" || lower.ends_with("++") {
        let mods = lower.strip_suffix("++").unwrap_or("");
        let plus = if mods.is_empty() {
            "plus".to_owned()
        } else {
            format!("{mods}+plus")
        };
        return Err(error(
            text,
            format_args!("write the plus key as \"plus\": {plus:?}"),
        ));
    }
    let (mods, key) = match lower.rsplit_once('+') {
        Some((mods, key)) => (mods.split('+').collect(), key),
        None => (Vec::new(), lower),
    };
    if key.is_empty() {
        return Err(error(text, "no key after the modifiers"));
    }
    Ok((mods, key))
}

/// The flag of modifier `name` in `modifiers`, or why it is none.
fn modifier<'a>(
    modifiers: &'a mut Modifiers,
    name: &str,
    apple: bool,
) -> Result<&'a mut bool, String> {
    Ok(match name {
        "ctrl" | "control" => &mut modifiers.ctrl,
        "alt" | "option" | "opt" => &mut modifiers.alt,
        "shift" => &mut modifiers.shift,
        "cmd" | "command" | "meta" | "super" | "win" => &mut modifiers.cmd,
        "mod" | "cmdorctrl" | "commandorcontrol" if apple => &mut modifiers.cmd,
        "mod" | "cmdorctrl" | "commandorcontrol" => &mut modifiers.ctrl,
        "" => return Err("an empty modifier".into()),
        other => {
            return Err(format!(
                "unknown modifier {other:?} (ctrl, alt, shift, cmd or mod)"
            ));
        }
    })
}

fn parse_key(text: &str, key: &str) -> Result<Key, KeystrokeError> {
    if let Some(&(_, named)) = NAMED.iter().find(|(name, _)| *name == key) {
        return Ok(named);
    }
    if let Some(n) = key
        .strip_prefix('f')
        .filter(|n| !n.starts_with('0'))
        .and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&n)
    {
        return Ok(Key::F(n));
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        let hint = if let Some((first, _)) = key.split_once('-')
            && modifier(&mut Modifiers::default(), first, false).is_ok()
        {
            // `mod--` is `mod+-`.
            let joined = match key.strip_suffix("--") {
                Some(mods) => format!("{}+-", mods.replace('-', "+")),
                None => key.replace('-', "+"),
            };
            format!(" (join modifiers and the key with \"+\": {joined:?})")
        } else {
            String::new()
        };
        return Err(error(text, format_args!("unknown key {key:?}{hint}")));
    };
    if let Some(&(_, base)) = SHIFTED.iter().find(|(shifted, _)| *shifted == c) {
        return Err(error(
            text,
            format_args!("{c:?} is typed with Shift: write \"shift+{base}\""),
        ));
    }
    if !c.is_ascii_graphic() {
        return Err(error(
            text,
            "keys are named by the US layout's characters (they match on any layout)",
        ));
    }
    Ok(Key::Char(c))
}

/// The character the key typing `c` types with Shift on the US layout
/// (`8` a `*`, `a` an `A`): what `shift+8` types.
pub fn shifted(c: char) -> char {
    SHIFTED
        .iter()
        .find(|(_, base)| *base == c)
        .map_or(c.to_ascii_uppercase(), |(shifted, _)| *shifted)
}

/// The key typing `c` without Shift on the US layout, when `c` is one of
/// its shifted characters (`?` is `/`, `*` is `8`): what a source that
/// reports only the character it typed means for a shortcut (`shift+/`).
/// `None` for a base character and for another layout's.
pub fn unshifted(c: char) -> Option<char> {
    SHIFTED
        .iter()
        .find(|(shifted, _)| *shifted == c)
        .map(|&(_, base)| base)
}

/// Whether keyboard shortcuts on `os` follow Apple's conventions: `mod` is
/// Command and labels use ⌘ ([`Keystroke::label`]). macOS and iOS do; a
/// browser does when it runs on an Apple platform (`apple_keyboard`,
/// [`crate::platform::Environment::apple_keyboard`]).
pub fn apple_keys(os: Os, apple_keyboard: bool) -> bool {
    match os {
        Os::MacOs | Os::Ios => true,
        Os::Web => apple_keyboard,
        Os::Android | Os::Windows | Os::Linux => false,
    }
}

impl Keystroke {
    /// The keys pressed for this keystroke: where Ctrl stands in for
    /// Command (`ctrl_for_command`, a terminal on macOS that cannot report
    /// Command: [`crate::platform::Environment::ctrl_for_command`]), its
    /// Command is Ctrl, so a label shows the key to press (`⌃O` for
    /// `mod+o`). A keystroke with both keeps them.
    pub fn pressed(mut self, ctrl_for_command: bool) -> Keystroke {
        if ctrl_for_command && self.modifiers.cmd && !self.modifiers.ctrl {
            self.modifiers.cmd = false;
            self.modifiers.ctrl = true;
        }
        self
    }

    /// The keystroke as menus and hints show it: `⌃⌥⇧⌘K` with Apple's
    /// conventions ([`apple_keys`]), else `Ctrl+Alt+Shift+K` (the Command
    /// modifier is `Win` on Windows, `Meta` in a browser, `Super`
    /// elsewhere).
    pub fn label(&self, os: Os, apple: bool) -> String {
        let m = self.modifiers;
        let mut s = String::new();
        if apple {
            for (on, glyph) in [(m.ctrl, '⌃'), (m.alt, '⌥'), (m.shift, '⇧'), (m.cmd, '⌘')] {
                if on {
                    s.push(glyph);
                }
            }
        } else {
            let cmd = match os {
                Os::Windows => "Win+",
                Os::Web => "Meta+",
                _ => "Super+",
            };
            for (on, name) in [
                (m.ctrl, "Ctrl+"),
                (m.alt, "Alt+"),
                (m.shift, "Shift+"),
                (m.cmd, cmd),
            ] {
                if on {
                    s.push_str(name);
                }
            }
        }
        let named = |mac: &'static str, other: &'static str| if apple { mac } else { other };
        match self.key {
            Key::Char(' ') => s.push_str("Space"),
            Key::Char(c) => s.push(c.to_ascii_uppercase()),
            Key::F(n) => {
                let _ = fmt::Write::write_fmt(&mut s, format_args!("F{n}"));
            }
            Key::Enter => s.push_str(named("↩", "Enter")),
            Key::Escape => s.push_str(named("⎋", "Esc")),
            Key::Tab => s.push_str(named("⇥", "Tab")),
            Key::Backspace => s.push_str(named("⌫", "Backspace")),
            Key::Delete => s.push_str(named("⌦", "Del")),
            Key::Insert => s.push_str(named("Help", "Ins")),
            Key::Up => s.push('↑'),
            Key::Down => s.push('↓'),
            Key::Left => s.push('←'),
            Key::Right => s.push('→'),
            Key::Home => s.push_str(named("↖", "Home")),
            Key::End => s.push_str(named("↘", "End")),
            Key::PageUp => s.push_str(named("⇞", "PgUp")),
            Key::PageDown => s.push_str(named("⇟", "PgDn")),
        }
        s
    }
}

impl fmt::Display for Keystroke {
    /// The canonical text: `ctrl+alt+shift+cmd+<key>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        for (on, name) in [
            (m.ctrl, "ctrl+"),
            (m.alt, "alt+"),
            (m.shift, "shift+"),
            (m.cmd, "cmd+"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        match self.key {
            Key::Char(' ') => f.write_str("space"),
            Key::Char(c) => write!(f, "{c}"),
            Key::F(n) => write!(f, "f{n}"),
            key => {
                let name = NAMED.iter().find(|(_, k)| *k == key).map(|(n, _)| *n);
                f.write_str(name.unwrap_or("?"))
            }
        }
    }
}

/// When a binding fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum When {
    /// Unless a text input (or a hosted native view, such as a web view)
    /// has the keyboard focus and the keystroke is one it uses
    /// ([`Keystroke::edits_text`]): the default.
    #[default]
    Auto,
    /// Wherever the keyboard focus is, in text inputs too.
    Always,
    /// Only while no text input or hosted native view has the keyboard
    /// focus.
    NotEditing,
}

/// One binding of a [`Keymap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    /// What the platform reports when it fires
    /// ([`crate::EventKind::KeyCommand`]'s payload).
    pub id: u32,
    /// The keystroke.
    pub keystroke: Keystroke,
    /// It fires while a text input or hosted native view has the keyboard
    /// focus ([`When`], resolved).
    pub in_text: bool,
    /// It fires again on the auto-repeats of a held key; else once until
    /// the key is released.
    pub repeat: bool,
    /// It fires and lets the key go on as if there were no binding
    /// ([`KeyMatch::Notify`]): Tab still moves the focus, a text input
    /// still types. Else the platform consumes the key.
    pub pass_through: bool,
    /// The window it belongs to, as its root node ([`crate::ROOT_ID`] for
    /// the main window, a dialog root for a window of the app's own): it
    /// fires only for keys pressed there. `None`: in any window.
    pub window: Option<crate::NodeId>,
    /// It is the keys of the view with the keyboard focus (a component's,
    /// bound while its part has the focus or its popup is open): Return
    /// and Space reach it while the focus is on a control they press
    /// ([`Keymap::find_at`]). Other bindings of those keys give way to the
    /// focused control.
    pub for_focus: bool,
}

impl KeyBinding {
    /// A binding of `keystroke` firing as `when` says, on auto-repeats
    /// too when `repeat`.
    pub fn new(id: u32, keystroke: Keystroke, when: When, repeat: bool) -> KeyBinding {
        let in_text = match when {
            When::Auto => !keystroke.edits_text(),
            When::Always => true,
            When::NotEditing => false,
        };
        KeyBinding {
            id,
            keystroke,
            in_text,
            repeat,
            pass_through: false,
            window: None,
            for_focus: false,
        }
    }

    /// The binding firing only in the window of root node `window`
    /// ([`Self::window`]).
    pub fn in_window(mut self, window: crate::NodeId) -> KeyBinding {
        self.window = Some(window);
        self
    }

    /// The binding firing without consuming the key when `pass_through`
    /// ([`Self::pass_through`]).
    pub fn passing_through(mut self, pass_through: bool) -> KeyBinding {
        self.pass_through = pass_through;
        self
    }

    /// The binding being the keys of the view with the keyboard focus when
    /// `for_focus` ([`Self::for_focus`]).
    pub fn focus_keys(mut self, for_focus: bool) -> KeyBinding {
        self.for_focus = for_focus;
        self
    }
}

/// What a key does with a [`Keymap`] ([`Keymap::find`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMatch {
    /// No binding: the key goes on as if there were no keymap.
    None,
    /// It fires binding `id`: the platform consumes the key and reports it
    /// ([`crate::EventKind::KeyCommand`]).
    Fire(u32),
    /// An auto-repeat of a held key whose binding fires once: the platform
    /// consumes it and reports nothing.
    Held,
    /// It fires binding `id`, which passes the key through
    /// ([`KeyBinding::pass_through`]): the platform reports it
    /// ([`crate::EventKind::KeyCommand`]) and lets the key go on as if there
    /// were no keymap (Tab moves the focus after the report).
    Notify(u32),
}

impl KeyMatch {
    /// The platform consumes the key.
    pub fn consumed(self) -> bool {
        matches!(self, KeyMatch::Fire(_) | KeyMatch::Held)
    }

    /// The binding the key fires, which the platform reports
    /// ([`crate::EventKind::KeyCommand`]): a consumed one's or a passing
    /// one's.
    pub fn fired(self) -> Option<u32> {
        match self {
            KeyMatch::Fire(id) | KeyMatch::Notify(id) => Some(id),
            KeyMatch::None | KeyMatch::Held => None,
        }
    }
}

/// The bindings of a surface, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
}

impl Keymap {
    /// A table of `bindings`, oldest first.
    pub fn new(bindings: Vec<KeyBinding>) -> Keymap {
        Keymap { bindings }
    }

    /// No bindings: the platform need not look at keys.
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// The bindings, oldest first.
    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    /// What a key does: the newest binding of `keystroke` that fires while
    /// `editing` (a text input or hosted native view has the keyboard
    /// focus) takes it; `repeat` is an auto-repeat of a held key, which only
    /// fires bindings that ask for it. A binding that passes the key
    /// through gives [`KeyMatch::Notify`], and its held key's auto-repeats
    /// go on ([`KeyMatch::None`]) unless it repeats.
    /// The key is pressed in the main window ([`Self::find_in`]).
    pub fn find(&self, keystroke: Keystroke, editing: bool, repeat: bool) -> KeyMatch {
        self.find_in(crate::ROOT_ID, keystroke, editing, repeat)
    }

    /// [`Self::find`] for a key pressed in the window of root node
    /// `window`: only that window's bindings and those of no window
    /// ([`KeyBinding::window`]) take it.
    pub fn find_in(
        &self,
        window: crate::NodeId,
        keystroke: Keystroke,
        editing: bool,
        repeat: bool,
    ) -> KeyMatch {
        self.find_at(window, keystroke, editing, false, repeat)
    }

    /// [`Self::find_in`] where the keyboard focus may be on a control
    /// (`control`: a view that takes presses, not a text input). The keys
    /// that press it ([`crate::input::presses_focused_control`]: Return
    /// and Space) are its own: only the bindings of the focus
    /// ([`KeyBinding::for_focus`], a menu button's Return opening its
    /// menu) take them first; with none, the key goes on and the platform
    /// presses the control. A binding of Return elsewhere (a dialog's main
    /// action) does not act instead of the control the user moved the
    /// focus to.
    pub fn find_at(
        &self,
        window: crate::NodeId,
        keystroke: Keystroke,
        editing: bool,
        control: bool,
        repeat: bool,
    ) -> KeyMatch {
        let pressing = control
            && !editing
            && crate::input::presses_focused_control(keystroke.key, keystroke.modifiers);
        let found = self.bindings.iter().rev().find(|b| {
            b.keystroke == keystroke
                && (b.in_text || !editing)
                && b.window.is_none_or(|w| w == window)
                && (b.for_focus || !pressing)
        });
        match found {
            None => KeyMatch::None,
            Some(b) if repeat && !b.repeat && b.pass_through => KeyMatch::None,
            Some(b) if repeat && !b.repeat => KeyMatch::Held,
            Some(b) if b.pass_through => KeyMatch::Notify(b.id),
            Some(b) => KeyMatch::Fire(b.id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_binding_takes_only_its_windows_keys() {
        let escape = Keystroke::parse_with("escape", true).unwrap();
        let k = |id| KeyBinding::new(id, escape, When::Always, false);
        // JS's (any window), the main window's, then window 7's.
        let table = Keymap::new(vec![
            k(1),
            k(2).in_window(crate::ROOT_ID),
            k(3).in_window(7),
        ]);
        assert_eq!(table.find_in(7, escape, false, false), KeyMatch::Fire(3));
        assert_eq!(
            table.find_in(crate::ROOT_ID, escape, false, false),
            KeyMatch::Fire(2)
        );
        assert_eq!(table.find(escape, false, false), KeyMatch::Fire(2));
        // A window with none of its own: the one of no window.
        assert_eq!(table.find_in(9, escape, false, false), KeyMatch::Fire(1));
        let only = Keymap::new(vec![k(3).in_window(7)]);
        assert_eq!(only.find_in(9, escape, false, false), KeyMatch::None);
    }

    #[test]
    fn return_and_space_on_a_control_fire_only_the_focus_bindings() {
        let key = |s| Keystroke::parse_with(s, true).unwrap();
        let (enter, space, cmd_enter) = (key("enter"), key("space"), key("cmd+enter"));
        // A dialog's Return, Space and Command+Return, then a menu button's
        // Return while it has the focus.
        let mut bindings = vec![
            KeyBinding::new(1, enter, When::NotEditing, false),
            KeyBinding::new(2, space, When::NotEditing, false),
            KeyBinding::new(3, cmd_enter, When::Always, false),
        ];
        let table = Keymap::new(bindings.clone());
        let at = |t: &Keymap, k, control| t.find_at(crate::ROOT_ID, k, false, control, false);
        // The focus on no control: they fire.
        assert_eq!(at(&table, enter, false), KeyMatch::Fire(1));
        assert_eq!(at(&table, space, false), KeyMatch::Fire(2));
        // On a control: they are its, but with Command held.
        assert_eq!(at(&table, enter, true), KeyMatch::None);
        assert_eq!(at(&table, space, true), KeyMatch::None);
        assert_eq!(at(&table, key("shift+enter"), true), KeyMatch::None);
        assert_eq!(at(&table, cmd_enter, true), KeyMatch::Fire(3));
        // The focus' own binding takes Return first.
        bindings.push(KeyBinding::new(4, enter, When::NotEditing, false).focus_keys(true));
        let table = Keymap::new(bindings);
        assert_eq!(at(&table, enter, true), KeyMatch::Fire(4));
        assert_eq!(at(&table, space, true), KeyMatch::None);
        // In a text field the control is not focused: as before.
        assert_eq!(
            table.find_at(crate::ROOT_ID, cmd_enter, true, true, false),
            KeyMatch::Fire(3)
        );
    }

    #[test]
    fn shift_types_the_us_layouts_characters() {
        assert_eq!(shifted('8'), '*');
        assert_eq!(shifted('2'), '@');
        assert_eq!(shifted('/'), '?');
        assert_eq!(shifted('a'), 'A');
        assert_eq!(shifted('*'), '*');
    }

    #[test]
    fn keys_have_windows_virtual_key_codes_both_ways() {
        assert_eq!(Key::from_virtual_key(0x4B), Some(Key::Char('k')));
        assert_eq!(Key::from_virtual_key(0x31), Some(Key::Char('1')));
        assert_eq!(Key::from_virtual_key(0x74), Some(Key::F(5)));
        assert_eq!(Key::from_virtual_key(0xBF), Some(Key::Char('/')));
        assert_eq!(Key::from_virtual_key(0x62), Some(Key::Char('2')));
        assert_eq!(Key::from_virtual_key(0x10), None);
        for vk in (0..=0xFF).filter(|vk| !(0x60..=0x69).contains(vk)) {
            if let Some(key) = Key::from_virtual_key(vk) {
                assert_eq!(key.virtual_key(), Some(vk), "{key:?}");
            }
        }
        assert_eq!(Key::Char('?').virtual_key(), None);
        assert_eq!(Key::F(25).virtual_key(), None);
    }

    #[test]
    fn keys_have_hid_usages_and_android_key_codes_both_ways() {
        assert_eq!(Key::from_hid_usage(4), Some(Key::Char('a')));
        assert_eq!(Key::from_hid_usage(39), Some(Key::Char('0')));
        assert_eq!(Key::from_hid_usage(81), Some(Key::Down));
        assert_eq!(Key::from_hid_usage(56), Some(Key::Char('/')));
        assert_eq!(Key::from_hid_usage(88), Some(Key::Enter));
        assert_eq!(Key::from_hid_usage(104), Some(Key::F(13)));
        assert_eq!(Key::from_hid_usage(225), None);
        // The keypad's keys have the main keyboard's usages back.
        for usage in (0..=0xFF).filter(|u| !(88..=98).contains(u)) {
            if let Some(key) = Key::from_hid_usage(usage) {
                assert_eq!(key.hid_usage(), Some(usage), "{key:?}");
            }
        }
        assert_eq!(Key::from_android_keycode(29), Some(Key::Char('a')));
        assert_eq!(Key::from_android_keycode(7), Some(Key::Char('0')));
        assert_eq!(Key::from_android_keycode(20), Some(Key::Down));
        assert_eq!(Key::from_android_keycode(76), Some(Key::Char('/')));
        assert_eq!(Key::from_android_keycode(160), Some(Key::Enter));
        assert_eq!(Key::from_android_keycode(59), None);
        for code in (0..=300).filter(|c| *c != 160 && !(144..=153).contains(c)) {
            if let Some(key) = Key::from_android_keycode(code) {
                assert_eq!(key.android_keycode(), Some(code), "{key:?}");
            }
        }
    }

    fn parse(text: &str) -> Result<Keystroke, KeystrokeError> {
        Keystroke::parse(text, Os::MacOs)
    }

    fn stroke(ctrl: bool, alt: bool, shift: bool, cmd: bool, key: Key) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                ctrl,
                alt,
                shift,
                cmd,
            },
            key,
        }
    }

    #[test]
    fn parses_keystrokes() {
        let c = |c| Key::Char(c);
        assert_eq!(parse("k"), Ok(stroke(false, false, false, false, c('k'))));
        assert_eq!(
            parse("cmd+shift+k"),
            Ok(stroke(false, false, true, true, c('k')))
        );
        assert_eq!(
            parse("ctrl+alt+enter"),
            Ok(stroke(true, true, false, false, Key::Enter))
        );
        assert_eq!(
            parse("cmd+-"),
            Ok(stroke(false, false, false, true, c('-')))
        );
        assert_eq!(parse("-"), Ok(stroke(false, false, false, false, c('-'))));
        assert_eq!(
            parse("cmd+="),
            Ok(stroke(false, false, false, true, c('=')))
        );
        assert_eq!(
            parse("space"),
            Ok(stroke(false, false, false, false, c(' ')))
        );
        assert_eq!(
            parse("f5"),
            Ok(stroke(false, false, false, false, Key::F(5)))
        );
        assert_eq!(
            parse("shift+/"),
            Ok(stroke(false, false, true, false, c('/')))
        );
        assert_eq!(
            parse("option+escape"),
            Ok(stroke(false, true, false, false, Key::Escape))
        );
        // `mod` is Command on Apple platforms, Control elsewhere.
        assert_eq!(parse("mod+k"), parse("cmd+k"));
        // Case does not matter, and a capital letter is not Shift.
        assert_eq!(parse("Mod+Shift+N"), parse("mod+shift+n"));
        assert_eq!(parse("CMD+K"), parse("cmd+k"));
        assert_eq!(parse("mod+K"), parse("mod+k"));
        assert_eq!(parse("Escape"), parse("escape"));
        assert_eq!(parse("F5"), parse("f5"));
        assert_eq!(parse("Ctrl+Plus"), parse("ctrl+="));
        assert_eq!(parse("cmdorctrl+k"), parse("meta+k"));
        assert_eq!(parse("mod+plus"), parse("mod+="));
        assert_eq!(parse("ctrl+minus"), parse("ctrl+-"));
        assert_eq!(parse("arrowup"), parse("up"));
        assert_eq!(parse("insert").unwrap().key, Key::Insert);
        assert_eq!(
            Keystroke::parse("mod+k", Os::Linux),
            Keystroke::parse("ctrl+k", Os::Linux)
        );
        assert_eq!(parse("mod+k").unwrap().to_string(), "cmd+k");
        assert_eq!(
            parse("shift+cmd+alt+ctrl+pagedown").unwrap().to_string(),
            "ctrl+alt+shift+cmd+pagedown"
        );
    }

    #[test]
    fn follows_the_browsers_platform() {
        // A browser resolves `mod` by the platform it runs on.
        assert!(apple_keys(Os::Web, true));
        assert!(!apple_keys(Os::Web, false));
        assert!(apple_keys(Os::MacOs, false));
        assert_eq!(
            Keystroke::parse_with("mod+k", true),
            Keystroke::parse("cmd+k", Os::MacOs)
        );
        assert_eq!(
            Keystroke::parse("mod+k", Os::Web),
            Keystroke::parse("ctrl+k", Os::Web)
        );
        // Literal modifiers stay what they say.
        assert_eq!(
            Keystroke::parse_with("ctrl+k", true),
            Keystroke::parse("ctrl+k", Os::Linux)
        );
    }

    #[test]
    fn labels_keystrokes() {
        let label = |t, os, apple| Keystroke::parse_with(t, apple).unwrap().label(os, apple);
        assert_eq!(label("mod+shift+l", Os::Web, true), "⇧⌘L");
        assert_eq!(label("mod+shift+l", Os::Web, false), "Ctrl+Shift+L");
        assert_eq!(label("mod+backspace", Os::MacOs, true), "⌘⌫");
        assert_eq!(label("ctrl+alt+enter", Os::Linux, false), "Ctrl+Alt+Enter");
        assert_eq!(label("cmd+space", Os::Windows, false), "Win+Space");
        assert_eq!(label("f5", Os::Web, true), "F5");
        assert_eq!(label("mod+/", Os::Web, false), "Ctrl+/");
        // Where Ctrl stands in for Command, the label shows Ctrl.
        let pressed = |t| {
            Keystroke::parse_with(t, true)
                .unwrap()
                .pressed(true)
                .label(Os::MacOs, true)
        };
        assert_eq!(pressed("mod+o"), "⌃O");
        assert_eq!(pressed("mod+shift+t"), "⌃⇧T");
        assert_eq!(pressed("ctrl+cmd+f"), "⌃⌘F");
    }

    #[test]
    fn explains_invalid_keystrokes() {
        let err = |t| parse(t).unwrap_err().0;
        assert_eq!(err(""), "invalid keystroke \"\": empty");
        assert!(err("mod+k mod+s").contains("one keystroke per binding"));
        assert!(err("cmd+").contains("no key after the modifiers"));
        assert!(err("hyper+k").contains("unknown modifier \"hyper\""));
        assert!(err("cmd+cmd+k").contains("\"cmd\" twice"));
        assert!(err("cmd+kk").contains("unknown key \"kk\""));
        assert!(err("mod-k").contains("join modifiers and the key with \"+\": \"mod+k\""));
        assert!(err("ctrl-shift-p").contains("\"ctrl+shift+p\""));
        assert!(err("mod--").contains("\"mod+-\""));
        // Hints suggest lowercase, whatever case the text was written in.
        assert!(err("Mod-K").contains("join modifiers and the key with \"+\": \"mod+k\""));
        assert!(err("CTRL++").contains("\"ctrl+plus\""));
        assert!(err("cmd+?").contains("write \"shift+/\""));
        assert!(err("mod++").contains("write the plus key as \"plus\": \"mod+plus\""));
        assert!(err("+").contains("\"plus\""));
        assert!(err("cmd+é").contains("US layout"));
        assert!(err("f25").contains("unknown key"));
        assert!(err("f05").contains("unknown key"));
        assert!(err("cmd++k").contains("an empty modifier"));
    }

    #[test]
    fn reserves_what_the_platform_keeps() {
        let reserved = |t, os| Keystroke::parse(t, os).unwrap().reserved(os).is_some();
        assert!(reserved("mod+q", Os::MacOs));
        assert!(reserved("cmd+alt+h", Os::MacOs));
        assert!(reserved("ctrl+cmd+f", Os::MacOs));
        assert!(!reserved("cmd+shift+q", Os::MacOs));
        assert!(!reserved("ctrl+q", Os::MacOs));
        assert!(!reserved("cmd+w", Os::MacOs));
        assert!(reserved("mod+q", Os::Linux));
        assert!(reserved("f11", Os::Linux));
        assert!(reserved("alt+f4", Os::Windows));
        assert!(!reserved("cmd+q", Os::Linux));
    }

    #[test]
    fn the_dev_reload_key() {
        let reloads = |t, os| Keystroke::parse(t, os).unwrap().reloads(os);
        assert!(reloads("mod+r", Os::MacOs));
        assert!(reloads("ctrl+r", Os::Windows));
        assert!(!reloads("ctrl+r", Os::MacOs));
        assert!(!reloads("mod+shift+r", Os::Linux));
    }

    #[test]
    fn text_inputs_keep_their_keys() {
        let edits = |t| parse(t).unwrap().edits_text();
        assert!(edits("k"));
        assert!(edits("shift+k"));
        assert!(edits("alt+k"));
        assert!(edits("escape"));
        assert!(edits("cmd+left"));
        assert!(edits("cmd+c"));
        assert!(edits("cmd+shift+z"));
        assert!(edits("ctrl+y"));
        assert!(edits("cmd+y"));
        assert!(!edits("cmd+k"));
        assert!(!edits("ctrl+shift+p"));
        assert!(!edits("f5"));
    }

    #[test]
    fn the_newest_binding_that_fires_wins() {
        let k = parse("cmd+k").unwrap();
        let e = parse("escape").unwrap();
        let map = Keymap::new(vec![
            KeyBinding::new(1, k, When::Auto, false),
            KeyBinding::new(2, e, When::Auto, true),
            KeyBinding::new(3, k, When::NotEditing, false),
        ]);
        assert_eq!(map.find(k, false, false), KeyMatch::Fire(3));
        assert_eq!(map.find(k, true, false), KeyMatch::Fire(1));
        assert_eq!(map.find(e, false, false), KeyMatch::Fire(2));
        assert_eq!(map.find(e, true, false), KeyMatch::None);
        assert_eq!(
            map.find(parse("cmd+j").unwrap(), false, false),
            KeyMatch::None
        );
        // A held key fires once, unless its binding repeats.
        assert_eq!(map.find(k, false, true), KeyMatch::Held);
        assert_eq!(map.find(e, false, true), KeyMatch::Fire(2));
        let always = Keymap::new(vec![KeyBinding::new(4, e, When::Always, false)]);
        assert_eq!(always.find(e, true, false), KeyMatch::Fire(4));
    }

    #[test]
    fn a_passing_binding_fires_and_leaves_the_key() {
        let tab = parse("tab").unwrap();
        let map = Keymap::new(vec![
            KeyBinding::new(1, tab, When::Auto, false).passing_through(true),
        ]);
        let found = map.find(tab, false, false);
        assert_eq!(found, KeyMatch::Notify(1));
        assert_eq!(found.fired(), Some(1));
        assert!(!found.consumed());
        // Its held key's repeats go on, unreported.
        assert_eq!(map.find(tab, false, true), KeyMatch::None);
        let repeating = Keymap::new(vec![
            KeyBinding::new(2, tab, When::Auto, true).passing_through(true),
        ]);
        assert_eq!(repeating.find(tab, false, true), KeyMatch::Notify(2));
        assert_eq!(KeyMatch::Fire(3).fired(), Some(3));
        assert!(KeyMatch::Fire(3).consumed() && KeyMatch::Held.consumed());
        assert_eq!(KeyMatch::Held.fired(), None);
    }
}
