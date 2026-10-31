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

## What was gated or stubbed

All gates use `cfg(target_arch = "wasm32")`, so native builds compile exactly
the same code as before.

| Where | What | On wasm |
|---|---|---|
| `taskboard-core/src/store.rs` | `Store::open`, `load`, `locked_update`, `write` | New `store::memory` module: `tasks.json` is a thread-local string seeded with `memory::seed()`. There's no lock file and no `create_dir_all`. Board edits (create, edit, move, delete) work in memory. Attachments still call `std::fs` and fail with an error banner. |
| `taskboard/src/app.rs` `check_projects` | `Path::is_dir` | No project counts as missing. Otherwise every project shows "Project folder is missing". |
| `taskboard/src/ui/mod.rs` `pick_file`/`pick_folder` | `rfd` | Returns `None`. rfd isn't a dependency of the spike. Its wasm build has no `FileHandle::path()`, so Phase 1 needs bytes-based upload. |
| `taskboard/src/ui/{mod,drag}.rs` | `std::time::Instant` | Changed to `iced::time::Instant` on every target (see above). |
| `taskboard-wasm-spike/src/watch.rs` | `notify` watcher | A stub module with the same API whose `subscription()` is `Subscription::none()`. Phase 2 replaces it with SSE. |
| Spike `Cargo.toml` | dependencies | iced without `tokio`, plus `webgl`. Also `getrandom` with `wasm_js` (needed by ulid/rand) and chrono with `wasmbind` (otherwise `Utc::now()` panics). |

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
