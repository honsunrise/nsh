# PR #1008 review follow-up evidence

Framework fix head: `e448d451e7b586d0348309ffa7f3c4e8d4e61f6b`. Paired checker fix: `cb812f800a753b41ab8285dcda91b2f62e390c3a`.
This branch contains evidence only and is not the PR source branch.
The framework history was rebased onto the target branch, with no merge commits. Its tree remains
`6f01d72960bd740754d5c789a2923b33db4b29cf`, identical before and after rebase.

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

- Bundled skills: 5 passed.
- Core after order notification correction: 1134 passed, 9 existing ignored.
- Menu suite: 17 passed, including the new single-item Web/Linux control.
- macOS backend: 82 passed; engine macOS prop mount test: 1 passed.
- Original headless gallery context-menu tests: 3 passed; one/two-item discriminator: 2 passed after correction (single failed before, two passed).
- Full Gallery headless suite: 294 passed.
- Real Chromium context-menu tests: 3 passed after the explicit keyboard event correction; tap and long press are included.
- Clippy core/base/engine/macOS all-targets with `-D warnings`: passed.
- Paired compatibility checker: 32 passed, including `checkout_apps_run` and both material-value tests; no ignored tests. No historical releases are recorded in this checker repository.
- Extra browser module suite: 80 passed, 2 skipped, 1 bidi-link test failed. The exact same bidi failure was reproduced at the fixed base; it is not changed or suppressed here.

Remote workflow state is reported separately in the PR comments; local evidence does not override required upstream gates.
