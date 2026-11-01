# taskboard-wasm: the web client

Taskboard's iced UI (`crates/taskboard-ui`) built for `wasm32-unknown-unknown`
with trunk, and served by the `taskboard-web` daemon. It runs with the web
`Platform` (`src/platform.rs`):

- Commands, the board, attached files, path checks and global settings go to
  the daemon's API (`taskboard_core::api`) with the browser's `fetch`
  (`src/api.rs`). Requests are same-origin, so the daemon's sign-in cookie
  goes with them. A 401 shows an error saying to open the URL printed in the
  daemon's log.
- Files picked to attach to a task are uploaded raw to `POST /api/uploads`;
  a new task's files go in its create command, as base64. Files over 40 MiB
  are refused when picked, since base64 makes them a third bigger and the
  daemon takes requests up to 64 MiB.
- `board_changes()` is an `EventSource` on `/api/events`. The browser
  reconnects by itself after a dropped connection, and the daemon says the
  board changed on each connection, so changes made while disconnected show
  up. If the daemon refuses the stream, it's reopened after 2 s, doubling up
  to 30 s; after 4 refusals in a row live updates are reported as off,
  though it keeps trying. A refusal for want of signing in stops it.
- Links open with `window.open`, only for `http:`, `https:` and `mailto:`,
  so a `javascript:` link in a task can't run in the daemon's origin.
- The page says "Loading…" until the UI starts, and why if it can't. The
  first requests for the board and settings give up after 15 s.
- There's no folder picker, so the folder Browse buttons are hidden and
  folders are typed. Files can't be attached by typing a path either (it
  would be read on the daemon's machine), only uploaded with Upload….
  Both come from the `Platform` (`picks_folders`, `attaches_paths`).
- Font size and theme are this device's: they're kept in the browser's
  `localStorage` (`taskboard.device`). Hooks, columns and the rest stay in
  the daemon's global settings.

## Build and serve

```sh
cd crates/taskboard-wasm
trunk build                         # -> dist/ (~38 MB debug wasm; use --release for size)
taskboard-web --static dist         # prints http://127.0.0.1:7878/?token=…
```

Open the `?token=…` link the daemon prints: it sets the sign-in cookie that
the client's requests (and its event stream) then carry.

`scripts/build-web.sh` makes the release build instead: trunk's release
`dist/` (trunk runs wasm-opt on it), gzip copies of its files, and the daemon
with all of it built in (`--features embed`), which needs no `--static`.

`trunk serve` can serve the client while working on it: it proxies `/api/`
to a daemon on the default `127.0.0.1:7878` (see `Trunk.toml`). The pages
then come from trunk's port, so the daemon needs `--allow-host
127.0.0.1:8080` to take their writes, and the cookie from opening the
daemon's link (cookies don't depend on the port).

Add `?debug` to the page's URL to mirror console output, errors and some
environment details (secure context, WebGPU/WebGL2 availability, viewport,
canvas sizes) into a log panel at the bottom of the page, since a phone's
browser console isn't visible. Tap the panel to hide it.

## Running iced on wasm

Found in the Phase 0 spike (task 0DC4FM), which this crate replaces:

- **`iced_drop` compiles for wasm as it is** (`=0.2.54`). It turns on iced's
  `x11` feature (and, because it doesn't disable iced's default features,
  `wayland`, `thread-pool` and `linux-theme-detection` too), but winit and
  softbuffer only pull in their X11 and Wayland dependencies on unix, so on
  wasm32 those do nothing. The custom card drag works in the browser.
- **`iced::time::every` and `window::frames` work.** The UI uses
  `iced::time::Instant` rather than `std::time::Instant`, whose `now()`
  panics on wasm32; natively they're the same type.
- **The embedded fonts load**, since `include_bytes!` puts them in the wasm.
- **iced without `tokio`.** iced uses wasm-bindgen-futures on the web;
  `taskboard-ui` only enables `tokio` off wasm32.
- **`chrono` needs `wasmbind`**, or `Utc::now()` panics, and ulid's
  `getrandom` needs its `wasm_js` backend (in `taskboard-core`).
- **Renderer.** `webgl` lets wgpu fall back to WebGL2 where there's no
  WebGPU; tiny-skia is compiled in as the last fallback.
- **Plain HTTP on the LAN.** The web UI has to work over plain `http://`
  from another machine. That isn't a secure context, so browsers expose no
  WebGPU (`navigator.gpu` is undefined) and the WebGL2 path is the one that
  matters, on phones too. Firefox for Android has no WebGPU at all. To test
  the WebGL2 path from a desktop browser, open the page through the
  machine's LAN or Tailscale address, not `127.0.0.1`/`localhost`, which
  count as secure contexts and may get WebGPU.
- The canvas has `touch-action: none` (`index.html`): the app scrolls and
  zooms nothing itself on touch, so the browser mustn't take the gesture.

## Mali GPUs and WebGL2 (task 8DS45NWN)

On a real Android phone over plain HTTP (Chrome 156, "ANGLE (ARM, Mali-G715,
OpenGL ES 3.2)", WebGL 2.0) the app panicked at startup:

```
Shader compilation failed: 0:165: S0032: no default precision defined for variable 'vec4[8]'
wgpu error: Validation Error
  In Device::create_render_pipeline, label = 'iced_wgpu.triangle.gradient.pipeline'
```

Over HTTPS (WebGPU) the same phone rendered fine, and desktop Chrome accepts
the shader, so this only shows up on Mali's GLES driver.

**Root cause.** iced's mesh gradient shader (`iced_wgpu`
`shader/triangle/gradient.wgsl`) builds `array<vec4<f32>, 8>` and
`array<f32, 8>` locals in its fragment stage and passes them to a function.
naga turns these into GLSL ES `vec4 colors_1[8] = vec4[8](...)` and so on, under
a default `precision highp float;`. That GLSL is fine. But Chrome hands it to
ANGLE, which re-emits it for the driver with an explicit precision on every
declaration and no default precision statement, and writes the arrays as
`vec4[8](...)` constructors (it also zero-fills every local array that way, so
even an uninitialised `vec4 a[8];` produces one). A constructor has no
precision qualifier of its own. Mali's compiler wants a default float
precision for that `vec4[8]` value, finds none in ANGLE's output, and rejects
the shader. Line 165 is in ANGLE's translated source, not naga's. ANGLE's
translation was captured locally by running Chrome with
`--use-angle=gles-egl` on Mesa (the same ANGLE-over-GLES path the phone uses)
and reading `WEBGL_debug_shaders`: it has no `precision` statement, and it has
`vec4[8](...)` and `float[8](...)` constructors in both `gradient()` and
`main()`. Mesa accepts it; Mali doesn't.

All 12 shaders iced compiles on WebGL2 were captured (by wrapping
`WebGL2RenderingContext.prototype.shaderSource`) and scanned. That fragment
shader is the only one with arrays. `quad/gradient.wgsl` has the same code,
but iced already skips that pipeline on wasm32. The other arrays (`blit.wgsl`,
`image.wgsl`) are in vertex shaders, where GLSL ES predeclares `highp float`,
and the blit pipeline is created before the failing one on the phone, so it
compiled there.

**Fix.** `vendor/iced_wgpu` is iced_wgpu 0.14.0 with a wasm32-only,
array-free copy of the gradient shader (`gradient_webgl.wgsl`), applied with
`[patch.crates-io]`. It reads stop `i` straight from the packed vertex outputs
(a `switch` for the colour, a vector index for the offset) and keeps the
gradient maths the same. Native builds still compile the upstream shader, so
the desktop app doesn't change. `vendor/README.md` describes the patch and
when to drop it. The generated GLSL and ANGLE's translation of it now contain
no arrays at all.

Alternatives considered:

- *Upgrade.* iced_wgpu 0.14.0 is the latest release and pins wgpu/naga 27
  (27.0.3 is the newest naga 27). iced master still has the arrays, and there
  was no upstream issue for this.
- *A naga/wgpu option.* naga's GLSL backend has no option that changes how
  arrays are written, and an explicit precision in naga's output wouldn't
  help, because ANGLE rewrites it anyway.
- *A JS shim rewriting the GLSL in `shaderSource`.* Adding precision
  statements does nothing for the same reason. Unrolling the arrays in the
  GLSL with regexes would be fragile, and swapping in hand-written GLSL would
  have to track naga's output names. Fixing the WGSL lets naga and wgpu
  validate it.

## Touch input (task NQKE8X84)

winit's web backend delivers `pointerType: "touch"` pointers as touch events
(`touch::Event::Finger*`), not mouse events. Iced's scrollable only scrolls
on touch when nothing inside it captures the `FingerPressed` first, but
buttons (list rows), mouse areas (headers), `iced_drop` droppables (cards) and
the nested column scrollable all do. So drags on them never scrolled, and a
column's own scrollable kept horizontal drags from reaching the board.

`ui/touch.rs` wraps the whole view and turns touch into mouse input: a tap is
a left click, a drag is pixel wheel scrolling locked to its first axis (with
a fling after lifting), and a 500 ms long press holds the left button, so
dragging on from there moves a card. The canvas also has `touch-action:
none`. In phone emulation (390 px wide, CDP touch events), drags on the
column background, on cards and on list rows scroll, and taps select.

## Soft keyboard (task WRJVWT28)

A phone only opens its keyboard for a focused editable DOM element, and iced
draws to a canvas that winit focuses. `src/keyboard.rs` is the fallback: a
hidden `<textarea>` that takes the focus while an iced text widget has it,
after a touch. It lives in this crate (`src/keyboard.rs`) and patches nothing in iced.

- `keyboard::bridge` wraps the root view (outside `ui/touch.rs`). After each
  event it runs an `Operation` over the tree, and over the overlay (the
  new-file popup's name field is in one), to find a focused `Focusable`.
- The textarea is focused from a `pointerup` listener on the document. That
  runs after winit's own listener on the canvas, which has already fed the
  tap to iced synchronously (winit's web runner handles the event and
  `AboutToWait` inside the DOM handler). `touch.rs` turns the lift into a
  click there, so the widget is focused by then, and the focus call happens
  inside the user gesture. Focus set by a task (the "+" button focusing the
  new task's title) is also seen in time. If a widget gains focus some other
  way after a touch, the textarea is focused at the next redraw instead.
- The textarea sits, invisible, over the focused widget, so the browser knows
  what to keep visible above the keyboard.
- Text goes back into iced as synthetic `keydown`/`keyup` events on the
  canvas, which winit handles like a physical keyboard. Each `input` event is
  diffed against the textarea's previous value (`src/text_diff.rs`), so
  composition updates (Gboard composes each word, with keyCode 229),
  autocorrect, `insertText`, `insertLineBreak` and `deleteContentBackward` all
  become backspaces and characters. The value rests at one space, so a
  backspace in an empty field still produces an `input` event and deletes
  iced's text before the caret. Keys that aren't text (Enter, Backspace with
  a real key code, arrows, Tab, Escape, Ctrl/Meta shortcuts) are forwarded
  as they are.
- winit focuses the canvas on every `pointerdown`, which would close the
  keyboard on each tap. The canvas's `focus` is overridden to do nothing
  while the textarea has focus. The canvas losing focus to the textarea is
  also kept from iced, which would otherwise hide the caret.
- When iced's text widget loses focus, the textarea is blurred (closing the
  keyboard) and the canvas gets the focus back.
- Mouse pointers never focus the textarea, so desktop browsers work as
  before, and the native app doesn't compile any of this.

Known limits:

- The diff counts grapheme clusters, since a backspace or arrow press moves
  over one in both the browser and iced. Insertions are replayed one `char`
  at a time, because iced inserts only the first `char` of a key's text.
- Edits are applied at iced's caret, which is assumed to sit where the
  textarea's caret does (at the end of its value). If the textarea's caret
  moves on its own (iOS's space-bar trackpad, caret handles, Gboard's cursor
  swipe), the next edit is still replayed relative to iced's caret and moving
  the caret itself isn't mirrored. A selection made inside the textarea isn't
  mirrored either: typing over it is replayed as deletes and inserts at
  iced's caret, which can remove the wrong text.
- Focus that comes from a task rather than the tap itself (the "+" button
  focusing the new title) is seen inside the `pointerup` today because iced
  runs the task synchronously. Any focus that lands later is applied at the
  next redraw, outside the gesture. Chrome on Android still opens the
  keyboard then, because the tap's user activation hasn't expired, but iOS
  Safari only opens it from inside the gesture, so there the field would need
  a second tap.
- Whether the keyboard is open is guessed from the visual viewport being
  shorter than 85% of the window (it decides whether tapping an already
  focused field should blur and refocus to reopen the keyboard). A floating
  or split keyboard, or a resizes-content viewport, doesn't shrink it, so
  there every tap in a focused field blurs and refocuses, which may flicker
  the keyboard.

Verified in headless Chrome over CDP in phone emulation (390x844, DPR 3,
touch events from `Input.dispatchTouchEvent`). Tapping the search field, the
new task's title and description, and the new-file popup's name field
focused the textarea during the `pointerup`, with
`navigator.userActivation.isActive` true. `Input.insertText`,
`Input.imeSetComposition` (a word composed and then corrected on commit),
`insertLineBreak`, `deleteContentBackward` and Backspace/Enter keys
(keyCode 8/13) all showed up in the iced fields. A task created that way was
saved. Tapping outside a field blurred the textarea. At desktop size with a
mouse, the textarea never took focus and typing still reached the canvas.
Headless Chrome has no on-screen keyboard, so whether one opens, and how a
real IME behaves, needs a phone.

## Left for a real-phone test

- **Soft keyboard.** Tap the search box, a task's Edit (title and
  description) and Files → New…. Does the keyboard open and stay open while
  typing? Does Gboard's composition (word suggestions, autocorrect, swipe
  typing), backspace, held backspace and Enter edit the field correctly? Does
  tapping another spot in the same field keep the keyboard up, and tapping
  outside close it? After closing the keyboard with the back button, does
  tapping the field reopen it? Is a field low on the screen kept visible
  above the keyboard?
- **Touch scroll.** Swipe the column lists vertically and the board
  horizontally.
- **Taps.** Buttons, cards, the status menu and the toolbar.
- **Custom drag (`ui/drag.rs` with `iced_drop`).** Long-press or press and
  drag a card between columns. Check whether the browser's own touch scrolling
  or zoom steals the gesture (it may need `touch-action: none` on the canvas).
- **DPR/canvas sizing.** Check the clipped right edge seen in emulation.
- **Release-build size and load time over the tunnel.**
- **The file picker.** Upload… should open the phone's picker (camera, files).
