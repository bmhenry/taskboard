#!/usr/bin/env bash
# Drive the taskboard GUI inside a headless Xvfb display.
# Usage: gui.sh <command> [args]   (run `gui.sh help` for the list)
set -euo pipefail

STATE=${TB_GUI_STATE:-${TMPDIR:-/tmp}/taskboard-gui-$USER}
REPO=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
BIN=$REPO/target/debug/taskboard

die() { echo "gui.sh: $*" >&2; exit 1; }
running() { [[ -f $STATE/display ]]; }
need_running() { running || die "not running; use 'gui.sh start' first"; }
x() { DISPLAY=$(cat "$STATE/display") "$@"; }

wait_for_window() {
  for _ in $(seq 150); do
    # The window maps before the first frame is drawn; wait until it isn't black.
    if x xdotool search --name '^Taskboard$' >/dev/null 2>&1 &&
       [[ $(x import -window root -format '%[fx:mean>0]' info:) == 1 ]]; then return 0; fi
    kill -0 "$(cat "$STATE/app.pid")" 2>/dev/null || { cat "$STATE/app.log" >&2; die "app exited during startup"; }
    sleep 0.1
  done
  die "window never appeared; see $STATE/app.log"
}

launch_app() {
  # Tasks and settings both live in the sandbox, so test runs never touch the
  # real ~/.local/share/tb or ~/.config/tb. Without WAYLAND_DISPLAY, winit
  # can't open the window on the user's desktop instead of Xvfb; Iced's
  # software renderer needs no GPU.
  DISPLAY=$(cat "$STATE/display") XDG_CONFIG_HOME=$STATE/config XDG_DATA_HOME=$STATE/data ICED_BACKEND=tiny-skia \
    nohup env -u WAYLAND_DISPLAY "$BIN" "$STATE/project" >"$STATE/app.log" 2>&1 &
  echo $! >"$STATE/app.pid"
  wait_for_window
  # With no window manager nothing focuses the window, and winit ignores
  # keys until it has focus.
  x xdotool search --name '^Taskboard$' windowfocus --sync
}

cmd=${1:-help}; shift || true
case $cmd in
  start) # start [WxH]: fresh sandbox project, Xvfb, and the app
    running && die "already running (display $(cat "$STATE/display")); use 'gui.sh stop' first"
    size=${1:-1600x950}
    (cd "$REPO" && cargo build -q -p taskboard -p tb)
    rm -rf "$STATE"; mkdir -p "$STATE/project/.git" "$STATE/config" "$STATE/data"
    for n in $(seq 99 140); do [[ -e /tmp/.X11-unix/X$n || -e /tmp/.X$n-lock ]] || break; done
    nohup Xvfb ":$n" -screen 0 "${size}x24" -nolisten tcp >"$STATE/xvfb.log" 2>&1 &
    echo $! >"$STATE/xvfb.pid"
    for _ in $(seq 50); do [[ -e /tmp/.X11-unix/X$n ]] && break; sleep 0.1; done
    echo ":$n" >"$STATE/display"
    launch_app
    echo "display :$n, project $STATE/project"
    ;;
  restart) # restart: relaunch the app (after a rebuild), keeping the sandbox data
    need_running
    kill "$(cat "$STATE/app.pid")" 2>/dev/null || true
    (cd "$REPO" && cargo build -q -p taskboard)
    launch_app
    ;;
  stop)
    for p in app xvfb; do [[ -f $STATE/$p.pid ]] && kill "$(cat "$STATE/$p.pid")" 2>/dev/null || true; done
    rm -f "$STATE/display"
    ;;
  shot) # shot [FILE] [WxH+X+Y]: screenshot, optionally cropped
    need_running
    out=${1:-$STATE/shot.png}
    if [[ -n ${2:-} ]]; then x import -window root -crop "$2" +repage "$out"; else x import -window root "$out"; fi
    echo "$out"
    ;;
  click) # click X Y [BUTTON]
    need_running; x xdotool mousemove "$1" "$2" sleep 0.05 click "${3:-1}" ;;
  dblclick) # dblclick X Y
    need_running; x xdotool mousemove "$1" "$2" sleep 0.05 click --repeat 2 --delay 80 1 ;;
  move) # move X Y: hover
    need_running; x xdotool mousemove "$1" "$2" ;;
  drag) # drag X1 Y1 X2 Y2: press, move in steps, release
    need_running
    x xdotool mousemove "$1" "$2" sleep 0.05 mousedown 1
    for i in 1 2 3 4 5 6 7 8 9 10; do
      x xdotool mousemove $(( $1 + ($3 - $1) * i / 10 )) $(( $2 + ($4 - $2) * i / 10 )) sleep 0.03
    done
    x xdotool sleep 0.05 mouseup 1
    ;;
  scroll) # scroll X Y up|down [N]
    need_running
    btn=$([[ $3 == up ]] && echo 4 || echo 5)
    x xdotool mousemove "$1" "$2" click --repeat "${4:-3}" --delay 30 "$btn"
    ;;
  type) # type TEXT
    need_running; x xdotool type --delay "${TB_GUI_TYPE_DELAY:-120}" -- "$1" ;;
  key) # key KEY...: xdotool key names, e.g. Return Escape ctrl+a
    need_running; x xdotool key --delay 50 "$@" ;;
  tb) # tb ARGS...: run the tb CLI against the sandbox store; --project DIR picks another project
    need_running
    project=(--project "$STATE/project"); [[ " $* " == *" --project "* ]] && project=()
    XDG_CONFIG_HOME=$STATE/config XDG_DATA_HOME=$STATE/data "$REPO/target/debug/tb" "${project[@]}" "$@" ;;
  env) # env: print paths
    echo "state $STATE"; echo "project $STATE/project"; echo "data $STATE/data/tb"; echo "log $STATE/app.log"
    running && echo "display $(cat "$STATE/display")" || echo "not running"
    ;;
  *)
    grep -E '^  [a-z]+\) #' "$0" | sed 's/^  [a-z]*) # /  /' ;;
esac
