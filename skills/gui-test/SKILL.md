---
name: gui-test
description: Drive the taskboard GUI headlessly to check a UI change works — launch it in an invisible Xvfb display, click, drag, scroll and type with xdotool, and read screenshots. Use when asked to test, try, click through, screenshot, or visually verify the taskboard app (crates/taskboard), or after changing its UI code.
---

# Headless GUI testing

The app is macroquad/miniquad over X11, so it runs unchanged in an Xvfb display.
That keeps testing off the user's desktop: their cursor and focus are untouched and
the window is the same size every run. Rendering is Mesa's software OpenGL, which
is fine for this 2D UI.

Everything goes through `skills/gui-test/scripts/gui.sh` (run `gui.sh help`):

| Command | Does |
|---|---|
| `start [WxH]` | builds, wipes the sandbox, starts Xvfb (default 1600x950, the app's window size) and the app, waits for the first frame |
| `restart` | rebuilds and relaunches the app, keeping the sandbox's tasks and settings |
| `stop` | kills the app and Xvfb |
| `shot [FILE] [WxH+X+Y]` | screenshot (default `$STATE/shot.png`), optionally cropped; prints the path |
| `click X Y [BUTTON]`, `dblclick X Y`, `move X Y` | mouse; `move` is for hover states |
| `drag X1 Y1 X2 Y2` | press, move in steps (passes the drag threshold), release |
| `scroll X Y up\|down [N]` | wheel at a point |
| `type TEXT`, `key KEY...` | keyboard; key names are xdotool's (`Return`, `Escape`, `ctrl+s`, `ctrl+a BackSpace`) |
| `tb ARGS...` | the `tb` CLI against the sandbox project |
| `env` | prints sandbox paths and the display |

## Sandbox

`start` points the app at a fresh project in `$STATE/project` (default
`$STATE` = `${TMPDIR:-/tmp}/taskboard-gui-$USER`) and sets `XDG_CONFIG_HOME` to
`$STATE/config`, so tests never touch the real `.tb/` board or the user's global
settings. Never launch the app against the repo itself for testing.

Seed data with `gui.sh tb add ...` / `tb move ...`; the app's file watcher picks it
up live. Check results the same way — `gui.sh tb list --json` / `tb show ID --json`
is more reliable than reading state off a screenshot. Saved settings are in
`$STATE/project/.tb/settings.json` and `$STATE/config/tb/settings.json`. App
stdout/stderr goes to `$STATE/app.log`.

## Loop

1. `gui.sh start` (or `restart` after editing code; `start` again for clean data).
2. `gui.sh shot`, then Read the PNG to find coordinates. Screenshot pixels are
   screen coordinates — the window sits at 0,0 with no window manager.
3. Act, `sleep 0.3` so a frame renders, then shot again to see the result.
4. `gui.sh stop` when done.

## Gotchas

- **Re-screenshot before every coordinate.** Layout shifts constantly: opening or
  closing the side panel resizes the columns, collapsed columns are narrow strips,
  and font size changes everything. Coordinates from an earlier shot go stale.
- **Typing speed.** `type` sends one key per 120ms (override with
  `TB_GUI_TYPE_DELAY`, in ms). Faster than that, keys that arrive in the same
  frame get applied out of order ("Typed via" → "Typed vai"). If you're testing
  text input itself, that's worth reporting as a possible app bug, not hiding.
- Crop with `shot FILE WxH+X+Y` to read small text (IDs, counts, hints) up close.
- Don't use `key` for text; use `type`. Don't use `type` for shortcuts; use `key`.
- If `start` says "already running", a previous session was left up: `gui.sh stop`.
