import { mount, Overlay, Scroll, signal, Text, View } from "nsh";

/** The fields of the core key event this test reads. */
interface KeyLike {
  key: string;
  text: string | null;
  physical: number | string | null;
  repeat: boolean;
  captured: boolean;
}

/** The props of a focusable view in this test: the app's key event props
 * and the ones every view has. Spreading them keeps this app type-checking
 * whether or not the generated view props already declare the key ones. */
interface KeyViewProps {
  testID: string;
  focusable: true;
  captureKeys?: "all" | readonly string[];
  onPress?: () => void;
  onFocus: () => void;
  onBlur: () => void;
  onKeyDown?: (e: KeyLike) => void;
  onKeyUp?: (e: KeyLike) => void;
}

/** A scroll view's key props (also extra to its generated props). */
interface ScrollKeyProps {
  captureKeys?: "all" | readonly string[];
  onKeyDown: (e: KeyLike) => void;
}

/** A logical ancestor's key props (also extra to View's generated props). */
interface HostKeyProps {
  captureKeys?: "all" | readonly string[];
  onKeyDown: (e: KeyLike) => void;
}

/** What the focused view's `onKeyDown` / `onKeyUp` reported last: the
 * source's key name, whether a mounted capture rule took it, the physical
 * HID usage, whether it repeated, and its text preview. */
function App() {
  const focus = signal("none");
  const key = signal("");
  const captured = signal("");
  const physical = signal("");
  const repeat = signal("");
  const text = signal("");
  const up = signal("");
  const scrollKey = signal("");
  const scrollCaptured = signal("");
  const portals = signal<"none" | "a" | "b">("none");
  const hostAKey = signal("");
  const hostACaptured = signal("");
  const hostACount = signal(0);
  const childBKey = signal("");
  const childBCaptured = signal("");

  const focusable = (
    name: string,
    captureKeys?: "all" | readonly string[],
    press = true,
  ): KeyViewProps => {
    const props: KeyViewProps = {
      testID: name,
      focusable: true,
      onFocus: () => focus.set(name),
      onBlur: () => focus.set("none"),
      onKeyDown: (e) => {
        key.set(e.key);
        captured.set(String(e.captured));
        physical.set(String(e.physical));
        repeat.set(e.repeat ? "repeat" : "once");
        text.set(e.text ?? "");
      },
      onKeyUp: (e) => up.set(e.key),
    };
    if (press) {
      // A pressable view: Return and Space press it, and its own
      // PressRecognizer focuses it.
      props.onPress = () => {};
    }
    if (captureKeys) {
      props.captureKeys = captureKeys;
    }
    return props;
  };

  /** A focusable view with no press or key listeners of its own. */
  const focusOnly = (name: string): KeyViewProps => ({
    testID: name,
    focusable: true,
    onFocus: () => focus.set(name),
    onBlur: () => focus.set("none"),
  });

  const scrollProps: ScrollKeyProps = {
    captureKeys: ["a"],
    onKeyDown: (e) => {
      key.set(e.key);
      captured.set(String(e.captured));
      scrollKey.set(e.key);
      scrollCaptured.set(String(e.captured));
    },
  };

  const hostAProps: HostKeyProps = {
    captureKeys: ["a"],
    onKeyDown: (e) => {
      hostAKey.set(e.key);
      hostACaptured.set(String(e.captured));
      hostACount.update((n) => n + 1);
    },
  };

  const childAProps: KeyViewProps = {
    testID: "childA",
    focusable: true,
    onPress: () => {},
    onFocus: () => focus.set("childA"),
    onBlur: () => focus.set("none"),
  };

  const childBProps: KeyViewProps = {
    ...childAProps,
    testID: "childB",
    onFocus: () => focus.set("childB"),
    onKeyDown: (e) => {
      childBKey.set(e.key);
      childBCaptured.set(String(e.captured));
    },
  };

  const row = { height: 64, backgroundColor: "#eeeeee", margin: 8 };

  return (
    <View style={{ flex: 1, paddingTop: 64, backgroundColor: "#ffffff" }}>
      <Text>{`focus:${focus()}`}</Text>
      <Text>{`key:${key()}`}</Text>
      <Text>{`captured:${captured()}`}</Text>
      <Text>{`physical:${physical()}`}</Text>
      <Text>{`repeat:${repeat()}`}</Text>
      <Text>{`text:${text()}`}</Text>
      <Text>{`up:${up()}`}</Text>
      <Text>{`scrollKey:${scrollKey()}`}</Text>
      <Text>{`scrollCaptured:${scrollCaptured()}`}</Text>
      <Text>{`hostAKey:${hostAKey()}`}</Text>
      <Text>{`hostACaptured:${hostACaptured()}`}</Text>
      <Text>{`hostACount:${hostACount()}`}</Text>
      <Text>{`childBKey:${childBKey()}`}</Text>
      <Text>{`childBCaptured:${childBCaptured()}`}</Text>
      <View testID="openPortalsA" focusable={false} onPress={() => portals.set("a")} style={row}>
        <Text>open portal A</Text>
      </View>
      <View testID="openPortalsB" focusable={false} onPress={() => portals.set("b")} style={row}>
        <Text>open portal B</Text>
      </View>
      <View {...focusable("first")} style={row}>
        <Text>first</Text>
      </View>
      <View {...focusable("second", ["a"])} style={row}>
        <Text>second</Text>
      </View>
      <View {...focusable("third")} style={row}>
        <Text>third</Text>
      </View>
      <View {...focusable("capturedTab", ["tab"])} style={row}>
        <Text>captured tab</Text>
      </View>
      <View {...focusable("noPress", undefined, false)} style={row}>
        <Text>no press</Text>
      </View>
      <Scroll testID="scrollAncestor" style={{ height: 180, margin: 8 }} {...scrollProps}>
        <View {...focusOnly("scrollChild")} style={{ height: 48, backgroundColor: "#dddddd" }}>
          <Text>scroll child</Text>
        </View>
      </Scroll>
      <View testID="hostA" style={{ height: 1 }} {...hostAProps}>
        {portals() === "a" ? (
          <Overlay style={{ top: 140, left: 8, width: 150, height: 70 }}>
            <View {...childAProps} style={{ height: 40 }}>
              <Text>child A</Text>
            </View>
          </Overlay>
        ) : null}
      </View>
      <View testID="hostB" style={{ height: 1 }}>
        {portals() === "b" ? (
          <Overlay style={{ top: 140, left: 200, width: 150, height: 70 }}>
            <View {...childBProps} style={{ height: 40 }}>
              <Text>child B</Text>
            </View>
          </Overlay>
        ) : null}
      </View>
    </View>
  );
}

mount(App);
