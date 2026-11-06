# PR #1008 review follow-up evidence

Framework head: `481f4a6ed5513968d19a71fde372ce058190eddb`, rebased onto `dc00005600c75590d5def96d248a39d6ba617f16`.
Paired checker: `6128dfdd6b5b017817d024229c4bcefdbe7afa71`, based on `2785160136b95bc10c2c099a6f46847724adbf65`.
This branch contains evidence only, not additional production changes. Neither PR adds merge commits. Browser keyboard handling is now separate in [#1098](https://github.com/longbridge/nsh/pull/1098).

## Latest Apple tint verification

- The same Rust scene exercised optional tint through the real AppKit and UIKit backends at the head above. `Signal<Option<AdaptiveColor>>` was exercised first; the final scene also compiles direct `.glass_tint_color(None)` and drives an optional closure.
- Both appearances: blue, transparent, native default, and red reached the native object while retaining the same glass view. The JSON observations include the source SHA, PID, window ID, tint values and native view identity.
- AppKit: mouse events reached the controls. The final LaunchServices bundle was active and key, and captures were taken after rendering settled. Earlier inactive-window captures muted the tint and are not used as proof of active tint rendering.
- UIKit: accessibility activation drove a real public `AppView` embedding host on Mac Catalyst. `UIGlassEffect.tintColor` received RGBA or nil, and the same `UIVisualEffectView` remained. Blur ignored tint changes without replacement; restoring glass applied the current tint.
- `pr1008-{appkit,uikit}-{light,dark}-{blue,clear,default,red}.png` are WindowServer captures. They are not `renderInContext:` screenshots.
- `apple-glass-smoke/native-proof.json` and `mac-native-proof.json` record the observations. Native test applications were closed afterward.
- Final-head UIKit lifecycle tests: 2 passed. Bundled-guide checks: 5 passed. Changed Rust modules passed the repository's nightly formatting check.

### Boundaries

Only macOS 27.2 / its Catalyst UIKit runtime was available; no older Apple OS or iPhone simulator run is claimed. The standard Catalyst `run_app` hit a pre-existing nil `URLContexts` return at `nsh-platform-ios/src/app.rs:150`; this unrelated startup path was not patched. The successful UIKit run uses the public embedding API instead. AppKit tint activation is now verified, but the older material-only follows-window activation experiment below was not repeated. Strict macOS Clippy remains blocked by the base's `drop(active)` reference warning in `selectable_view.rs:358`.

### Reproducing the current UIKit scene

On Apple Silicon, with the `aarch64-apple-ios-macabi` Rust target installed, run from `verification/pr1008/apple-glass-smoke` in this evidence checkout:

```sh
sdk="$(xcrun --sdk macosx --show-sdk-path)"
export CARGO_TARGET_DIR="$PWD/target"
SDKROOT="$sdk" cargo build --locked --target aarch64-apple-ios-macabi
clang -target arm64-apple-ios16.0-macabi -isysroot "$sdk" -c probe.c -o probe.o
swiftc -emit-library -target arm64-apple-ios16.0-macabi -sdk "$sdk" \
  -F "$sdk/System/iOSSupport/System/Library/Frameworks" \
  -L "$sdk/System/iOSSupport/usr/lib" probe.swift probe.o -o probe.dylib
mkdir -p AppleGlassTint.app/Contents/MacOS
cp Info.plist AppleGlassTint.app/Contents/Info.plist
cp target/aarch64-apple-ios-macabi/debug/apple-glass-smoke AppleGlassTint.app/Contents/MacOS/
codesign --force --sign - AppleGlassTint.app
open -n AppleGlassTint.app --env DYLD_INSERT_LIBRARIES="$PWD/probe.dylib" \
  --env APPLE_GLASS_COMMAND="$PWD/command.json"
printf '{"action":"inspect","path":"%s/frame.json"}' "$PWD" > command.json
```

The driver consumes `inspect`, `press` with a `label`, and `appearance` with `dark: true/false`. Give each command an absolute output `path`; wait for its command file to be consumed before writing the next one. The visible buttons can also be used directly.

For AppKit, build the same crate without `--target`, build the existing sibling `../probe` driver, and put the executable in a bundle as `Contents/MacOS/nsh-material-smoke` using `AppKit-Info.plist`. That executable name matches the AppKit driver's initializer. Launch with `DYLD_INSERT_LIBRARIES` and `REXNSH_COMPOSITION_COMMAND`. Use the driver's `windows`, `activate`, `inspect`, `appearance` and `click` operations; verify `active` and `key` before interpreting tint screenshots. Capture the reported window with `screencapture -x -o -l <window>`.

## Historical material-only verification

The sections below describe the earlier `de735c18` framework / `ab9d1c62` checker runs. Their counts and limits are historical, not final-head reruns. The previous duplicate OrderWatch implementation was dropped in favor of upstream's pending-focus fix.

### Exercised paths

- Fixed-base WindowServer comparison: base `2bfe8d2e6decd5f333d690eb1afff489e972c97c` versus PR material implementation `99aa22d8cb5aa9a78079f9959f7f41c80925f3eb` with the reviewed non-exhaustive constructor migration. The material-rendering implementation itself is unchanged by the follow-up commits.
- `screencapture -x -o -l <window>` captured actual compositor output, not `renderInContext` or an in-process view screenshot.
- Normal/multiply comparison for plain, text, SVG and clipped views; return to normal removed all four layer filters.
- HUD/behind-window active and inactive, follows-window state assignment, Sidebar/within-window inactive; removing the explicit material restored glass.
- Live red tint, blue tint and clear retained the same glass object; regular/clear/none/restored effect transitions exercised glass and blur.
- AppKit mouse events reached the button through both glass and material: click count advanced from 0 to 1 to 2.
- Light/dark appearance overrides were applied only to the test window.
- Standalone transparent host: `isOpaque=false`, background alpha 0, unpainted screenshot pixel alpha 0; baseline opaque window had alpha 255.
- Existing opaque blue embedding host retained opacity/background on first `AppView::new`/`load` with `transparent=true`, and after a behind-window material update. Reparenting a live view also retained the host settings and click state.
- After the live-main rebase, the fixture was rebuilt and standalone transparency, HUD switching, live tint/clear with glass identity retained, click delivery, and first-load opaque embedding were exercised again. The `pr1008-rebased-standalone.png` and `pr1008-rebased-embedded.png` captures cover this final code (the subsequent CI-selection edit changes no runtime code).

### Boundaries

- macOS 27.2 only; no macOS-before-26 execution is claimed. The macOS unit suite exercises the glass-unavailable selection branch, but this is not an older-OS run.
- The session could not make the fixture app active/key, even from a bundle. Explicit active/inactive material states and the follows-window property were exercised, but real WindowServer active/inactive focus transitions remain unverified.
- The standard CLI/generated TSX host intentionally remains opaque; the public docs now state this. No general packaged-app transparency feature was added.
- Input was directed through AppKit test events. No system authorization was approved and no user settings were changed.

### Reproduction sources

`native-fixture/src/main.rs` holds the isolated Rust scene. Build with `cargo build --manifest-path native-fixture/Cargo.toml --features materials`; omit that feature against the fixed base to compare existing blend behavior. Set `NSH_VERIFY_EMBEDDED=1` to exercise first-load embedding.

`probe/probe.swift` and `probe/probe.c` are the injected, test-only AppKit driver. Build the C initializer and link it with `swiftc -emit-library`. Set `DYLD_INSERT_LIBRARIES` to the resulting dylib and `REXNSH_COMPOSITION_COMMAND` to a private command JSON path. The driver consumes one command at a time; examples are `{"action":"windows","path":"/absolute/windows.json"}`, `{"action":"menu","windowNumber":123,"title":"Tint red"}`, and `{"action":"inspect","windowNumber":123,"path":"/absolute/frame.json"}`. Capture the returned window number using the system screenshot command.

Only the disposable fixture binary is instrumented; none of this driver is shipped by NSH.

### Validation results

Completed after rebasing onto current upstream main:

- Bundled skills: 5 passed.
- Core: 1143 passed, 9 existing ignored.
- Menu suite: 16 passed, using upstream's pending-focus implementation.
- macOS backend: 87 passed.
- Original headless context-menu file: all 3 passed, including the held-finger case.
- Real Chromium: keyboard context menu and ordinary tap each passed. The upstream `nsh.js` with the same WASM failed the keyboard case, confirming the retained browser dispatch correction is still needed.
- The real-browser test host drives a desktop mouse. Its long-press operation is not a touch input; the held-finger case therefore is not part of the newly added desktop-Chrome selection. The original full headless file remains unchanged and runs in CI.
- Clippy core/base/engine/macOS all-targets with `-D warnings`: passed.
- Paired compatibility checker: 32 passed with no ignored tests, including `checkout_apps_run` and both material-value tests. No historical releases are recorded in this checker repository.
- Browser modules: 88 passed, 2 skipped, 1 bidi-link fixture failure. The same failure was reproduced at the earlier fixed baseline; it is not changed or suppressed here.

Earlier-stage evidence, not presented as a rerun of the final rebased tree:

- Full Gallery headless suite: 294 passed before the live-main rebase.

Remote workflow state is reported separately in the PR comments; local evidence does not override required upstream gates.

