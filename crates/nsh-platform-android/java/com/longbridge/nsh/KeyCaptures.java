package com.longbridge.nsh;

/**
 * The mounted {@code captureKeys} rules of one view, as
 * {@code encode.rs} {@code key_capture} writes them, and the canonical
 * shortcut key of an Android {@code KeyEvent} to match them with.
 *
 * <p>A rule is the same canonical shortcut keystroke the app's shortcuts
 * use: the unshifted key of the device's layout for a printable key (with
 * the actual Unicode character, never a fabricated US one), a named key
 * otherwise, and the exact modifiers. {@code none}
 * captures nothing, {@code all} every key, and {@code keys} the listed
 * ones. State is a flat {@code int[]}: mode, and for {@code keys} the
 * count and each key's id and modifiers. {@code keymap.rs}
 * {@code capture_key} and {@code capture_modifiers} write it; this class
 * is the reader, kept in step by {@code KeyCaptureTest}.
 */
final class KeyCaptures {
    static final int NONE = 0;
    static final int ALL = 1;
    static final int KEYS = 2;

    /** Modifier bits, in `keymap.rs` `capture_modifiers` order. */
    static final int CTRL = 1, ALT = 2, SHIFT = 4, CMD = 8;

    /** `keymap.rs` `capture_key`: the negative ids of named keys. */
    static final int ENTER = -1, ESCAPE = -2, TAB = -3, BACKSPACE = -4, DELETE = -5,
            INSERT = -6, UP = -7, DOWN = -8, LEFT = -9, RIGHT = -10, HOME = -11, END = -12,
            PAGE_UP = -13, PAGE_DOWN = -14;
    /** {@code -100 - n}. */
    static final int FUNCTION_BASE = -100;

    private KeyCaptures() {}

    /** Whether {@code policy} captures any key at all. */
    static boolean capturesAnything(int[] policy) {
        if (policy == null || policy.length == 0) return false;
        return policy[0] == ALL || (policy[0] == KEYS && policy.length > 1 && policy[1] > 0);
    }

    /** Whether {@code policy} captures the key {@code key} with {@code modifiers}. */
    static boolean matches(int[] policy, int key, int modifiers) {
        if (policy == null || policy.length == 0) return false;
        if (policy[0] == ALL) return true;
        if (policy[0] != KEYS || policy.length < 2) return false;
        int count = policy[1];
        for (int i = 0; i < count; i++) {
            int at = 2 + i * 2;
            if (at + 1 >= policy.length) return false;
            if (policy[at] == key && policy[at + 1] == modifiers) return true;
        }
        return false;
    }

    /** `KeyEvent.META_*_ON`. */
    static final int META_SHIFT_ON = 0x1, META_ALT_ON = 0x2, META_CTRL_ON = 0x1000,
            META_META_ON = 0x10000;

    /** The core's modifiers (`keymap::Modifiers`) for Android's meta state. */
    static int modifiers(int meta) {
        int modifiers = 0;
        if ((meta & META_CTRL_ON) != 0) modifiers |= CTRL;
        if ((meta & META_ALT_ON) != 0) modifiers |= ALT;
        if ((meta & META_SHIFT_ON) != 0) modifiers |= SHIFT;
        if ((meta & META_META_ON) != 0) modifiers |= CMD;
        return modifiers;
    }

    /**
     * The canonical key of key code {@code code} and its unshifted
     * character {@code unicode} ({@code KeyEvent.getUnicodeChar(0)}): a
     * printable layout character (letters lower case), else a named key.
     * {@code keymap.rs} {@code keystroke} makes the same choice for the
     * app's shortcuts; Android's key codes are the US layout's, but a
     * layout typing another character on one keeps that character.
     */
    static int eventKey(int code, int unicode) {
        if (unicode > 0 && !Character.isISOControl(unicode)
                && Character.isValidCodePoint(unicode)) {
            // As `Keystroke::parse` canonicalizes: ASCII letters lower
            // case, every other character as the layout reports it.
            return unicode >= 'A' && unicode <= 'Z' ? unicode + ('a' - 'A') : unicode;
        }
        if (code >= 7 && code <= 16) return (int) '0' + (code - 7);
        if (code >= 29 && code <= 54) return (int) 'a' + (code - 29);
        if (code >= 144 && code <= 153) return (int) '0' + (code - 144);
        switch (code) {
            case 55: return (int) ',';
            case 56: return (int) '.';
            case 61: return TAB;
            case 62: return (int) ' ';
            case 66:
            case 160: return ENTER;
            case 67: return BACKSPACE;
            case 68: return (int) '`';
            case 69: return (int) '-';
            case 70: return (int) '=';
            case 71: return (int) '[';
            case 72: return (int) ']';
            case 73: return (int) '\\';
            case 74: return (int) ';';
            case 75: return (int) '\'';
            case 76: return (int) '/';
            case 92: return PAGE_UP;
            case 93: return PAGE_DOWN;
            case 111: return ESCAPE;
            case 112: return DELETE;
            case 122: return HOME;
            case 123: return END;
            case 124: return INSERT;
            case 19: return UP;
            case 20: return DOWN;
            case 21: return LEFT;
            case 22: return RIGHT;
            case 23: return ENTER;
            case 131: return FUNCTION_BASE - 1;
            case 132: return FUNCTION_BASE - 2;
            case 133: return FUNCTION_BASE - 3;
            case 134: return FUNCTION_BASE - 4;
            case 135: return FUNCTION_BASE - 5;
            case 136: return FUNCTION_BASE - 6;
            case 137: return FUNCTION_BASE - 7;
            case 138: return FUNCTION_BASE - 8;
            case 139: return FUNCTION_BASE - 9;
            case 140: return FUNCTION_BASE - 10;
            case 141: return FUNCTION_BASE - 11;
            case 142: return FUNCTION_BASE - 12;
            default: return Integer.MIN_VALUE;
        }
    }
}
