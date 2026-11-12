import { mount, signal, Text, View } from "nsh";

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
  onPress: () => void;
  onFocus: () => void;
  onBlur: () => void;
  onKeyDown: (e: KeyLike) => void;
  onKeyUp: (e: KeyLike) => void;
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

  const focusable = (name: string, captureKeys?: "all" | readonly string[]): KeyViewProps => {
    const props: KeyViewProps = {
      testID: name,
      focusable: true,
      // A pressable view: Return and Space press it, and a tap focuses it.
      onPress: () => {},
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
    if (captureKeys) {
      props.captureKeys = captureKeys;
    }
    return props;
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
    </View>
  );
}

mount(App);
