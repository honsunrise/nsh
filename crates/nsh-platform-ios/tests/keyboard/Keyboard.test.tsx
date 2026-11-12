// The hardware keyboard on a focusable view: one typed KeyDown/KeyUp per
// UIPress transition (the HID usage, the reported key with its case, the
// text preview, no invented repeat), a mounted capture rule taking the key
// before the default, UIKit's initial Tab traversal and our focus moves.
// The simulator injects real IOHID key events.
import { expect, open, test } from "nsh/test";

test("tab from the window's initial focus reaches the first stop", async () => {
  // Nothing has the keyboard focus yet: UIKit's focus engine moves through
  // the views that answer canBecomeFocused, and the surface makes the
  // focused one the first responder.
  const app = await open("./App.tsx");
  await app.keyboard.press("tab");
  await app.waitFor("text:focus:first");
});

test("a focused view reports a hardware press and release with the source's data", async () => {
  const app = await open("./App.tsx");
  await app.press("id:first");
  await app.waitFor("text:focus:first");
  await app.keyboard.press("a");
  await app.waitFor("text:key:a");
  expect(app.shows("text:captured:false")).toBe(true);
  expect(app.shows("text:physical:4")).toBe(true);
  expect(app.shows("text:repeat:once")).toBe(true);
  expect(app.shows("text:text:a")).toBe(true);
  expect(app.shows("text:up:a")).toBe(true);
});

test("a capture rule takes the key before the default", async () => {
  const app = await open("./App.tsx");
  await app.press("id:second");
  await app.waitFor("text:focus:second");
  await app.keyboard.press("a");
  await app.waitFor("text:key:a");
  expect(app.shows("text:captured:true")).toBe(true);
});

test("tab moves between focusable views", async () => {
  const app = await open("./App.tsx");
  await app.press("id:first");
  await app.waitFor("text:focus:first");
  await app.keyboard.press("tab");
  await app.waitFor("text:focus:second");
  await app.keyboard.press("tab");
  await app.waitFor("text:focus:third");
});

test("a captured tab stays in the focused view", async () => {
  const app = await open("./App.tsx");
  await app.press("id:capturedTab");
  await app.waitFor("text:focus:capturedTab");
  // The view's mounted rule captures Tab: a prioritized key command takes
  // it from UIKit's focus traversal and reports it captured.
  await app.keyboard.press("tab");
  await app.waitFor("text:captured:true");
  expect(app.shows("text:key:Tab")).toBe(true);
  expect(app.shows("text:focus:capturedTab")).toBe(true);
});

test("a focusable view without onPress takes focus on a tap", async () => {
  const app = await open("./App.tsx");
  await app.press("id:noPress");
  await app.waitFor("text:focus:noPress");
  await app.keyboard.press("a");
  await app.waitFor("text:key:a");
  expect(app.shows("text:captured:false")).toBe(true);
});

test("a scroll view ancestor hears and captures a focused child's key", async () => {
  const app = await open("./App.tsx");
  await app.press("id:scrollChild");
  await app.waitFor("text:focus:scrollChild");
  await app.keyboard.press("a");
  await app.waitFor("text:scrollKey:a");
  await app.waitFor("text:scrollCaptured:true");
  expect(app.shows("text:captured:true")).toBe(true);
  expect(app.shows("text:focus:scrollChild")).toBe(true);
});

test("a portal notifies and captures its logical ancestor", async () => {
  const app = await open("./App.tsx");
  await app.press("id:openPortalsA");
  await app.waitFor("id:childA");
  await app.press("id:childA");
  await app.waitFor("text:focus:childA");
  await app.keyboard.press("a");
  await app.waitFor("text:hostAKey:a");
  await app.waitFor("text:hostACaptured:true");
  await app.waitFor("text:hostACount:1");
});

test("a sibling portal does not capture another portal's ancestor", async () => {
  const app = await open("./App.tsx");
  await app.press("id:openPortalsB");
  await app.waitFor("id:childB");
  await app.press("id:childB");
  await app.waitFor("text:focus:childB");
  await app.keyboard.press("a");
  await app.waitFor("text:childBKey:a");
  await app.waitFor("text:childBCaptured:false");
  expect(app.shows("text:hostACount:0")).toBe(true);
});

test("shift+tab moves the focus back", async () => {
  const app = await open("./App.tsx");
  await app.press("id:second");
  await app.waitFor("text:focus:second");
  await app.keyboard.press("shift+tab");
  await app.waitFor("text:focus:first");
});
