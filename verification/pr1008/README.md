# PR #1008 review follow-up evidence

Framework fix head: `de735c18c829340d4afe706ec1bfa282407a59a8`, rebased onto upstream `0d0338ac55e94a004acdb59a333356410c3530c4`.
Paired checker fix: `ab9d1c629e7bab4de2312fd54028364e1ec3cb02`, rebased onto `660a8fe28fd7164285fdf923bde1e61e32e6e666`.
This branch contains evidence only and is not the PR source branch. Neither PR adds a merge commit.
The current upstream pending-focus implementation superseded the local OrderWatch patch, which was dropped during rebase.

## Exercised paths

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

## Boundaries

- macOS 27.2 only; no macOS-before-26 execution is claimed. The macOS unit suite exercises the glass-unavailable selection branch, but this is not an older-OS run.
- The session could not make the fixture app active/key, even from a bundle. Explicit active/inactive material states and the follows-window property were exercised, but real WindowServer active/inactive focus transitions remain unverified.
- The standard CLI/generated TSX host intentionally remains opaque; the public docs now state this. No general packaged-app transparency feature was added.
- Input was directed through AppKit test events. No system authorization was approved and no user settings were changed.

## Reproduction sources

`native-fixture/src/main.rs` holds the isolated Rust scene. Build with `cargo build --manifest-path native-fixture/Cargo.toml --features materials`; omit that feature against the fixed base to compare existing blend behavior. Set `NSH_VERIFY_EMBEDDED=1` to exercise first-load embedding.

`probe/probe.swift` and `probe/probe.c` are the injected, test-only AppKit driver. Build the C initializer and link it with `swiftc -emit-library`. Set `DYLD_INSERT_LIBRARIES` to the resulting dylib and `REXNSH_COMPOSITION_COMMAND` to a private command JSON path. The driver consumes one command at a time; examples are `{"action":"windows","path":"/absolute/windows.json"}`, `{"action":"menu","windowNumber":123,"title":"Tint red"}`, and `{"action":"inspect","windowNumber":123,"path":"/absolute/frame.json"}`. Capture the returned window number using the system screenshot command.

Only the disposable fixture binary is instrumented; none of this driver is shipped by NSH.

## Validation results

Completed after rebasing onto current upstream main:

- Bundled skills: 5 passed.
- Core: 1143 passed, 9 existing ignored.
- Menu suite: 16 passed, using upstream's pending-focus implementation.
- macOS backend: 87 passed.
- Original headless context-menu file: all 3 passed, including the held-finger case.
- Real Chromium: keyboard context menu and ordinary tap each passed. The upstream `nsh.js` with the same WASM failed the keyboard case, confirming the retained browser dispatch correction is still needed.
- The real-browser test host drives a desktop mouse. Its long-press operation is not a touch input; the held-finger case therefore is not part of the newly added desktop-Chrome selection. The original full headless file remains unchanged and runs in CI.
- Clippy core/base/engine/macOS all-targets with `-D warnings`: passed.
- Browser modules: 88 passed, 2 skipped, 1 bidi-link fixture failure. The same failure was reproduced at the earlier fixed baseline; it is not changed or suppressed here.

Earlier-stage evidence, not presented as a rerun of the final rebased tree:

- Full Gallery headless suite: 294 passed before the live-main rebase.
- Paired checker: 32 passed, including `checkout_apps_run` and both material-value tests, before the live-main rebase. The fresh rebased result is reported in PR comments. No historical releases are recorded in this checker repository.

Remote workflow state is reported separately in the PR comments; local evidence does not override required upstream gates.

