# Web UI, Phase 0 spike: the iced UI on wasm32

Task 0DC4FM, part of the "Web UI" epic. This crate builds the desktop GUI's
own source (`crates/taskboard/src/{app,markdown,source_link}.rs` and `ui/`,
included with `#[path]`) for `wasm32-unknown-unknown`. The board is seeded
from `fixture/tasks.json` into an in-memory store. Nothing moves between
crates; that happens in Phase 1.

## Answers

**Does `iced_drop` compile for wasm?** Yes, as-is (`=0.2.54`), with no
workaround. It turns on iced's `x11` feature (and, because it doesn't disable
iced's default features, `wayland`, `thread-pool` and `linux-theme-detection`
too). winit and softbuffer only pull in their X11 and Wayland dependencies
for unix targets, so on wasm32 those features do nothing. Its `tiny-skia`
feature compiles too. The drag itself also works in the browser (see below).

**Do `iced::time::every` and `window::frames` work on wasm?** Yes. The spike
wraps `Gui::update` and logs to the console when either fires. In headless
Chrome, `time::every` (the 5 s `CheckProjects` timer) fired repeatedly, and
`window::frames` fired during a card drag's gap animation (10+ frames with
increasing `Instant`s). One code change was needed: `ui/mod.rs` and
`ui/drag.rs` used `std::time::Instant`. `Instant::now()` panics on
wasm32-unknown-unknown, and `Message::Frame` has to carry the `Instant` type
that `window::frames` produces. Both now use `iced::time::Instant`, which is
a re-export of `web_time::Instant` and the same type as `std::time::Instant`
on native, so desktop behaviour doesn't change.

**Do the embedded fonts load?** Yes. `.font(theme::*_BYTES)` and
`.default_font(theme::REGULAR)` work unchanged, because `include_bytes!` puts
the fonts in the wasm binary. The screenshots show DejaVu Sans, Bold
(headings) and Mono rendering.

**Renderer.** Headless Chrome reports "No available adapters" for WebGPU, so
wgpu falls back to WebGL2 through iced's `webgl` feature, which this crate
turns on. tiny-skia is also compiled in as the last fallback.

**Requirement: plain HTTP on the LAN.** The web UI has to work when served
over plain `http://` from a machine on the local network. That isn't a secure
context, so browsers expose no WebGPU (`navigator.gpu` is undefined) and the
WebGL2 path is the one that matters, on phones too. Firefox for Android has
no WebGPU at all, so it always takes this path.

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

## What was gated or stubbed

Every gate uses `cfg(target_arch = "wasm32")`, so native builds keep the
desktop code paths. The one exception is the `Instant` swap, which applies on
every target but changes nothing on native, where `iced::time::Instant` is
`std::time::Instant`.

| Where | What | On wasm |
|---|---|---|
| `taskboard-core/src/store.rs` | `Store::open`, `load`, `locked_update`, `write` | New `store::memory` module: `tasks.json` is a thread-local string seeded with `memory::seed()`. There's no lock file and no `create_dir_all`. Board edits (create, edit, move, delete) work in memory. Attachments still call `std::fs` and fail with an error banner. |
| `taskboard/src/app.rs` `check_projects` | `Path::is_dir` | No project counts as missing. Otherwise every project shows "Project folder is missing". |
| `taskboard/src/ui/mod.rs` `pick_file`/`pick_folder` | `rfd` | Returns `None`. rfd isn't a dependency of the spike. Its wasm build has no `FileHandle::path()`, so Phase 1 needs bytes-based upload. |
| `taskboard/src/ui/{mod,drag}.rs` | `std::time::Instant` | Changed to `iced::time::Instant` on every target (see above). |
| `taskboard-wasm-spike/src/watch.rs` | `notify` watcher | A stub module with the same API whose `subscription()` is `Subscription::none()`. Phase 2 replaces it with SSE. |
| Spike `Cargo.toml` | dependencies | iced without `tokio`, plus `webgl`. Also chrono with `wasmbind` (otherwise `Utc::now()` panics). |
| `taskboard-core/Cargo.toml` | wasm32-only `getrandom` dependency with `wasm_js` | ulid/rand use getrandom 0.4, which needs its JS backend in the browser. With this, `cargo check -p taskboard-core --target wasm32-unknown-unknown` works on its own. |

Not gated, because they compile and only fail at runtime:

- `xdg-open` in `app.rs` (`std::process::Command`) fails, so opening links
  does nothing.
- Hooks (`hooks::run`) fail to spawn and are reported as hook failures. The
  fixture has no hooks.
- The viewer's `std::fs::read` shows a read error.
- `project_path`/`create_project_dir` use `fs::canonicalize`/`create_dir_all`.

All the dependencies in this crate are under
`[target.'cfg(target_arch = "wasm32")'.dependencies]`. A native
`cargo build/test --workspace` only builds an empty stub `main`.

## Build and serve

```sh
cd crates/taskboard-wasm-spike
trunk build                                   # -> dist/ (~37 MB debug wasm; use --release for size)
trunk serve                                   # http://127.0.0.1:8080/
trunk serve --address 0.0.0.0 --port 8080     # reachable from a phone on the LAN / Tailscale
```

`index.html` mirrors console output, errors and some environment details
(secure context, WebGPU/WebGL2 availability, viewport, canvas sizes) into a
log panel at the bottom of the page, because a phone's browser console isn't
visible. Tap the panel to hide it.

To test the WebGL2 path from a desktop browser, open the page through the
machine's LAN or Tailscale address (not `127.0.0.1`/`localhost`, which count
as secure contexts and may get WebGPU).

The phone opens `http://<this-machine's-LAN-or-Tailscale-IP>:8080/`. A
release build is much smaller and faster: `trunk serve --release --address
0.0.0.0`. Reloading the page resets the board to the fixture.

## What was verified in a browser

Headless Google Chrome was driven over CDP:

- At 1400x900, the board renders all six columns with the fixture's 10 tasks,
  including the epic badge, parent links and the source-link chip.
- A mouse drag of "Apple Pay button" from Open to In Progress shows the
  custom drag ghost and the animated gap. After the drop the card is in In
  Progress and the counts update.
- Clicking the search box and typing "nvim" filters the board.
- No panics or exceptions appeared in the console.
- In phone emulation (390x844, DPR 3, touch enabled), the page renders and an
  emulated tap selects a card, which opens the detail panel. The desktop
  layout is unusable at that width (Phase 3). The canvas also didn't cover
  the right ~20% of the viewport at DPR 3, which is a sizing issue to check
  on a real device. An emulated horizontal swipe didn't scroll anything
  visibly, so that result is inconclusive.

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

## Left for the real-phone test

- **Soft keyboard.** Does tapping a `text_input` (search) or `text_editor`
  (description) bring it up? iced draws to a canvas and winit's web backend
  only focuses the canvas, so it probably won't. The fallback is a hidden DOM
  `<input>` that mirrors focus.
- **Touch scroll.** Swipe the column lists vertically and the board
  horizontally.
- **Taps.** Buttons, cards, the status menu and the toolbar.
- **Custom drag (`ui/drag.rs` with `iced_drop`).** Long-press or press and
  drag a card between columns. Check whether the browser's own touch scrolling
  or zoom steals the gesture (it may need `touch-action: none` on the canvas).
- **DPR/canvas sizing.** Check the clipped right edge seen in emulation.
- **Release-build size and load time over the tunnel.**
