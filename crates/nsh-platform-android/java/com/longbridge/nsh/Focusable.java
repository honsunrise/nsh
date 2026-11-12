package com.longbridge.nsh;

/**
 * The `focusable` decision, kept free of Android types so it is testable
 * on a plain JDK: `ViewProps::focus_stop`'s rule. Unset leaves the view a
 * stop when it takes presses; `true` makes any view one; `false` keeps it
 * out; a disabled view is never a stop. A pointer press focuses any stop,
 * but a default pressable leaves the focus to a text input or hosted
 * native view that has it; only an explicit `true` takes it from them.
 */
final class Focusable {
    private Focusable() {}

    /** Whether a view with `prop` (-1 unset, 0 false, 1 true) is a Tab stop. */
    static boolean stop(int prop, boolean takesPresses, boolean enabled) {
        if (!enabled) return false;
        if (prop == 0) return false;
        if (prop == 1) return true;
        return takesPresses;
    }

    /** Whether a view with `prop` is a Tab stop; see {@link #stop}. */
    static boolean pointer(int prop, boolean nativeFocus) {
        // An explicit focusable always takes a pointer press. A default
        // pressable stop does too, unless a text input or hosted native
        // view has the focus: a tap on a button must not steal an input's
        // editing (and hide its keyboard).
        return prop == 1 || (prop == -1 && !nativeFocus);
    }
}
