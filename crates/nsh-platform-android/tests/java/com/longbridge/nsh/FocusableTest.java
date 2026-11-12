package com.longbridge.nsh;

/**
 * Coverage for the `focusable` stop/pointer rule. Plain JDK, no Android
 * APIs:
 *
 * <pre>
 * javac -d target/java-tests \
 *   crates/nsh-platform-android/java/com/longbridge/nsh/Focusable.java \
 *   crates/nsh-platform-android/tests/java/com/longbridge/nsh/FocusableTest.java
 * java -cp target/java-tests com.longbridge.nsh.FocusableTest
 * </pre>
 */
public final class FocusableTest {
    public static void main(String[] args) {
        check("unset leaves a pressable a stop", Focusable.stop(-1, true, true));
        check("unset leaves a view taking no presses out", !Focusable.stop(-1, false, true));
        check("true makes any enabled view a stop", Focusable.stop(1, false, true));
        check("false keeps a pressable out", !Focusable.stop(0, true, true));
        check("a disabled view is never a stop", !Focusable.stop(1, true, false));
        check("a disabled pressable is not a stop", !Focusable.stop(-1, true, false));
        check("only an explicit focusable takes a pointer press from a native child", Focusable.pointer(1, true));
        check("an explicit focusable takes a pointer press", Focusable.pointer(1, false));
        check("a default press takes it when no native child has the focus",
                Focusable.pointer(-1, false));
        check("a default press leaves it to a native child", !Focusable.pointer(-1, true));
        check("false never takes it", !Focusable.pointer(0, false));
        System.out.println("FocusableTest: ok");
    }

    private static void check(String what, boolean ok) {
        if (!ok) throw new AssertionError(what);
    }
}
