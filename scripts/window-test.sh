#!/bin/sh
# Starts the real client on a real compositor that has no screen, and fails
# unless a window opens: the one test that sees a client built with no display
# backend, which starts, attaches to its daemon, and shows nothing.
#
# Everything runs in a work folder of its own: a D-Bus session that activates
# no service (no portal, no accessibility bus), a headless compositor whose
# socket is in that folder, a throwaway daemon, and the client, with HOME and
# the XDG folders inside it. No window reaches the screen of a desktop the
# script runs on. The client draws on the CPU (Mesa's lavapipe) unless --gpu
# is given. The test passes when, 5 seconds after its start, the client is
# alive, attached to the daemon, and shows its window:
#   - Wayland: the client's protocol log (WAYLAND_DEBUG) holds a toplevel with
#     the application ID, its configure acknowledged, and a buffer attached to
#     its surface;
#   - X11: the X server holds a viewable window of the application's class.
# Whatever the outcome, the client, the daemon, the compositor, and the bus
# are stopped. The work folder is removed after a pass, and kept after a
# failure (its path is printed) or with --keep. The X server alone writes
# outside it, its socket and lock under /tmp, and removes them when it stops.
#
#   sh scripts/window-test.sh                  build, then test the client of target/debug on Wayland
#   sh scripts/window-test.sh --x11            the same on an X server with no screen (Xvfb)
#   sh scripts/window-test.sh --client <path> --daemon <path>   test programs already built
#
# The Wayland compositor is Mutter, headless with a virtual monitor. Weston's
# headless backend offers no seat (wl_seat), which GPUI requires.
#
# Needs dbus-run-session, ss (iproute2), Mesa's lavapipe, and Mutter (Fedora:
# dbus-daemon iproute mesa-vulkan-drivers mutter), and for --x11 Xvfb and
# xdotool (Fedora: xorg-x11-server-Xvfb xdotool).

set -eu

usage() {
  echo "usage: ${0##*/} [--x11] [--gpu] [--keep]" >&2
  echo "       [--release] [--client <path> --daemon <path>]" >&2
  exit 2
}

repo=$(cd "$(dirname "$0")/.." && pwd)

if [ "${1:-}" != --inside ]; then
  backend=wayland
  gpu=false
  keep=false
  profile=debug
  client=
  daemon=
  while [ $# -gt 0 ]; do
    case $1 in
      --x11) backend=x11 ;;
      --gpu) gpu=true ;;
      --keep) keep=true ;;
      --release) profile=release ;;
      --client)
        [ $# -ge 2 ] || usage
        client=$2
        shift
        ;;
      --daemon)
        [ $# -ge 2 ] || usage
        daemon=$2
        shift
        ;;
      -h | --help) usage ;;
      *)
        echo "unexpected argument: $1" >&2
        usage
        ;;
    esac
    shift
  done
  if [ -z "$client$daemon" ]; then
    release=
    [ "$profile" = release ] && release=--release
    (cd "$repo" && cargo build --locked $release -p crowbotd)
    (cd "$repo" && cargo build --locked $release --manifest-path crates/crowbot-desktop/Cargo.toml)
    client=$repo/crates/crowbot-desktop/target/$profile/crowbot
    daemon=$repo/target/$profile/crowbotd
  elif [ -z "$client" ] || [ -z "$daemon" ]; then
    echo "--client and --daemon go together" >&2
    usage
  fi
  needed="dbus-run-session ss"
  if [ "$backend" = x11 ]; then
    needed="$needed Xvfb xdotool"
  else
    needed="$needed mutter"
  fi
  for program in $needed "$client" "$daemon"; do
    command -v "$program" >/dev/null || { echo "missing: $program" >&2; exit 1; }
  done

  # Socket paths hold at most 107 bytes: a long TMPDIR gives way to /tmp.
  base=${TMPDIR:-/tmp}
  [ ${#base} -le 40 ] || base=/tmp
  work=$(mktemp -d "$base/crowbot-window.XXXXXX")
  case $work in *[[:space:]]*)
    echo "the work folder $work holds a space" >&2
    rm -rf "$work"
    exit 1
    ;;
  esac
  # A bus with no service folder activates nothing on request.
  mkdir "$work/bus"
  cat >"$work/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$work/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
  # The application ID, from its one constant.
  app_id=$(sed -n 's/^pub const APP_ID: &str = "\(.*\)";$/\1/p' \
    "$repo/crates/crowbot-desktop/src/identity.rs")
  export window_test_work="$work" window_test_backend="$backend" \
    window_test_gpu="$gpu" window_test_app_id="$app_id" \
    window_test_client="$client" window_test_daemon="$daemon"
  status=0
  dbus-run-session --config-file="$work/bus.conf" -- sh "$0" --inside || status=$?
  if [ "$status" -eq 0 ] && [ "$keep" = false ]; then
    rm -rf "$work"
  else
    echo "the work folder is kept: $work" >&2
  fi
  exit "$status"
fi

# Inside the private D-Bus session.
work=$window_test_work
app_id=$window_test_app_id
export HOME="$work/home"
export XDG_CONFIG_HOME="$HOME/.config" XDG_CACHE_HOME="$HOME/.cache"
export XDG_DATA_HOME="$HOME/.local/share" XDG_STATE_HOME="$HOME/.local/state"
export XDG_RUNTIME_DIR="$work/run"
unset DISPLAY WAYLAND_DISPLAY WAYLAND_SOCKET
# No system bus either: Mutter would ask it for the keyboard layout, and
# systemd would start a service of the computer to answer.
export DBUS_SYSTEM_BUS_ADDRESS="unix:path=$work/no-system-bus"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

compositor_pid=
daemon_pid=
client_pid=
stop() {
  # The client first, then the daemon it is attached to, then the display.
  for pid in $client_pid $daemon_pid $compositor_pid; do
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 50); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
}
trap stop EXIT
trap 'exit 130' INT TERM HUP

fail() {
  echo "fail: $*" >&2
  for log in compositor daemon; do
    [ -s "$work/$log.log" ] && { echo "--- $log" >&2; tail -n 15 "$work/$log.log" >&2; }
  done
  # The client's own words, without the protocol log.
  if [ -s "$work/client.log" ]; then
    echo "--- client" >&2
    grep -v '^\[' "$work/client.log" | tail -n 15 >&2 || true
  fi
  exit 1
}

# Waits up to 20 seconds until the command given holds, or the process
# `$watched` has exited.
until_ready() {
  for _ in $(seq 200); do
    "$@" && return 0
    kill -0 "$watched" 2>/dev/null || return 1
    sleep 0.1
  done
  return 1
}
compositor_ready() {
  if [ "$window_test_backend" = wayland ]; then
    [ -S "$XDG_RUNTIME_DIR/$display" ]
  else
    [ -s "$work/x-display" ]
  fi
}
daemon_ready() {
  grep -q "listening on" "$work/daemon.log"
}

if [ "$window_test_gpu" = false ]; then
  icd=
  # Fedora names the file after the architecture, Ubuntu does not.
  for file in "/usr/share/vulkan/icd.d/lvp_icd.$(uname -m).json" /usr/share/vulkan/icd.d/lvp_icd.json; do
    if [ -e "$file" ]; then icd=$file; break; fi
  done
  [ -n "$icd" ] || fail "Mesa's lavapipe is not installed (no lvp_icd file in /usr/share/vulkan/icd.d)"
  export VK_DRIVER_FILES="$icd"
  # The compositor draws with Mesa's software OpenGL.
  export LIBGL_ALWAYS_SOFTWARE=1
fi

if [ "$window_test_backend" = wayland ]; then
  display=crowbot-window
  mutter --headless --wayland --no-x11 --virtual-monitor 1280x800 \
    --wayland-display "$display" >"$work/compositor.log" 2>&1 &
  compositor_pid=$!
  watched=$compositor_pid
  until_ready compositor_ready || fail "mutter did not start"
  client_display="WAYLAND_DISPLAY=$display WAYLAND_DEBUG=1"
else
  Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$work/x-display" \
    >"$work/compositor.log" 2>&1 &
  compositor_pid=$!
  watched=$compositor_pid
  until_ready compositor_ready || fail "Xvfb did not start"
  client_display="DISPLAY=:$(tr -d '[:space:]' <"$work/x-display")"
fi

socket=$work/crowbotd.sock
"$window_test_daemon" --endpoint "$socket" --db "$work/data/crowbot.db" \
  --bots-dir "$work/bots" </dev/null >/dev/null 2>"$work/daemon.log" &
daemon_pid=$!
watched=$daemon_pid
until_ready daemon_ready || fail "the daemon did not start"

# shellcheck disable=SC2086 # the display variables are words on purpose
env $client_display "$window_test_client" --endpoint "$socket" \
  </dev/null >"$work/client.log" 2>&1 &
client_pid=$!
for _ in $(seq 50); do
  kill -0 "$client_pid" 2>/dev/null || break
  sleep 0.1
done

problems=
problem() {
  problems="${problems:+$problems; }$*"
}
kill -0 "$client_pid" 2>/dev/null || {
  status=0
  wait "$client_pid" || status=$?
  client_pid=
  problem "the client exited with status $status"
}

# The daemon's end of each connection names the socket, and its peer is the
# client's end: `ss` gives both with the process that holds each.
ss -xpH >"$work/sockets.log"
attached=$(awk -v socket="$socket" -v daemon="pid=$daemon_pid," -v client="pid=$client_pid," '
  { local_inode[NR] = $6; holder[NR] = $0 }
  $5 == socket && index($0, daemon) { peer[$8] = 1 }
  END {
    for (line in local_inode)
      if ((local_inode[line] in peer) && client != "pid=," && index(holder[line], client)) {
        print "yes"; exit
      }
  }' "$work/sockets.log")
[ "$attached" = yes ] || problem "the client is not attached to the daemon"

if [ "$window_test_backend" = wayland ]; then
  # The protocol log names objects `interface#id` (libwayland 1.23 on) or
  # `interface@id` (before).
  shown=$(awk -v app_id="$app_id" '
    function id(text, interface,    rest) {
      if (!match(text, interface "[#@][0-9]+")) return ""
      rest = substr(text, RSTART, RLENGTH)
      sub(interface "[#@]", "", rest)
      return rest
    }
    / -> xdg_wm_base[#@][0-9]+\.get_xdg_surface\(/ {
      surface_of[id($0, "new id xdg_surface")] = id($0, ", wl_surface")
    }
    / -> xdg_surface[#@][0-9]+\.get_toplevel\(/ {
      xdg_of[id($0, "new id xdg_toplevel")] = id($0, "xdg_surface")
    }
    / -> xdg_toplevel[#@][0-9]+\.set_app_id\(/ && index($0, "\"" app_id "\"") {
      window = xdg_of[id($0, "xdg_toplevel")]
    }
    window != "" && / -> xdg_surface[#@][0-9]+\.ack_configure\(/ && id($0, "xdg_surface") == window {
      acknowledged = 1
    }
    acknowledged && / -> wl_surface[#@][0-9]+\.attach\(wl_buffer/ && id($0, "wl_surface") == surface_of[window] {
      print "yes"; exit
    }' "$work/client.log")
  [ "$shown" = yes ] || problem "no window opened: no toplevel of $app_id was configured and drawn"
else
  # shellcheck disable=SC2086
  windows=$(env $client_display xdotool search --onlyvisible --classname "^$(echo "$app_id" | sed 's/\./\\./g')\$" 2>/dev/null || true)
  [ -n "$windows" ] || problem "no window opened: the X server holds no viewable window of $app_id"
fi

[ -z "$problems" ] || fail "$problems"
server=mutter
[ "$window_test_backend" = x11 ] && server=Xvfb
echo "ok: the client is alive, attached to its daemon, and shows its window ($window_test_backend on $server)"
