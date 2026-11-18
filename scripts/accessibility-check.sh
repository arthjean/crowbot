#!/bin/sh
# Starts the client the way a screen reader finds it, and checks what it shows
# on the accessibility bus (AT-SPI).
#
# GPUI publishes a window to AT-SPI only once org.a11y.Status.IsEnabled is
# true, which Orca sets when it starts. This script runs everything in a
# D-Bus session of its own, so the desktop it runs on is not touched: its own
# accessibility bus with IsEnabled on, a headless GNOME compositor (Mutter)
# drawing on the CPU, and the client on a throwaway daemon. It reads the tree
# as a screen reader does, through libatspi, prints it, and fails unless the
# window and its buttons read with their roles and labels.
#
#   sh scripts/accessibility-check.sh             check the client of target/debug
#   sh scripts/accessibility-check.sh --release   check the client of target/release
#
# Needs mutter, at-spi2-core, python3-gobject, and Mesa's lavapipe (Fedora:
# mutter at-spi2-core python3-gobject mesa-vulkan-drivers).

set -eu

usage() {
  echo "usage: ${0##*/} [--release]" >&2
  exit 2
}

repo=$(cd "$(dirname "$0")/.." && pwd)
profile=debug
inside=false
while [ $# -gt 0 ]; do
  case $1 in
    --release) profile=release ;;
    --inside) inside=true ;;
    -h | --help) usage ;;
    *) usage ;;
  esac
  shift
done
target=${CARGO_TARGET_DIR:-$repo/target}
client=$target/$profile/crowbot

if [ "$inside" = false ]; then
  [ -x "$client" ] && [ -x "$target/$profile/crowbotd" ] || {
    echo "build the client and the daemon first: $client" >&2
    exit 1
  }
  for program in dbus-run-session mutter python3 /usr/libexec/at-spi-bus-launcher /usr/libexec/at-spi2-registryd; do
    command -v "$program" >/dev/null || { echo "missing: $program" >&2; exit 1; }
  done
  exec dbus-run-session -- sh "$0" --inside $([ "$profile" = release ] && echo --release)
fi

# Inside the private session. Socket paths must stay short, so the session's
# files live under the runtime folder, not the scratch folder of a test.
runtime=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/crowbot-a11y.XXXXXX")
export XDG_RUNTIME_DIR="$runtime"
pids=
cleanup() {
  pkill -f -- "--endpoint $runtime/crowbotd.sock" 2>/dev/null || true
  for pid in $pids; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
  rm -rf "$runtime"
}
trap cleanup EXIT

/usr/libexec/at-spi-bus-launcher --launch-immediately >/dev/null 2>&1 &
pids="$pids $!"
for _ in $(seq 50); do
  gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
    --method org.a11y.Bus.GetAddress >/dev/null 2>&1 && break
  sleep 0.1
done
/usr/libexec/at-spi2-registryd >/dev/null 2>&1 &
pids="$pids $!"
# What Orca does when it starts.
gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
  --method org.freedesktop.DBus.Properties.Set org.a11y.Status IsEnabled '<true>' >/dev/null

mutter --headless --wayland --no-x11 --virtual-monitor 1280x800 \
  --wayland-display crowbot-a11y >"$runtime/mutter.log" 2>&1 &
pids="$pids $!"
for _ in $(seq 100); do
  [ -S "$runtime/crowbot-a11y" ] && break
  sleep 0.1
done
[ -S "$runtime/crowbot-a11y" ] || { cat "$runtime/mutter.log" >&2; echo "mutter did not start" >&2; exit 1; }

for icd in /usr/share/vulkan/icd.d/lvp_icd.*.json; do
  [ -e "$icd" ] && export VK_DRIVER_FILES="$icd"
done
WAYLAND_DISPLAY=crowbot-a11y "$client" --endpoint "$runtime/crowbotd.sock" -- \
  --db "$runtime/crowbot.db" --bots-dir "$runtime/bots" >"$runtime/client.log" 2>&1 &
pids="$pids $!"

# libatspi warns about each node AccessKit announces, whose cache signature it
# does not know; it reads the nodes all the same. Its log is shown on a failure.
status=0
python3 -I - 2>"$runtime/atspi.log" <<'EOF' || status=$?
import sys, time
import gi
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

def tree(node, depth=0):
    lines = ["  " * depth + f"[{node.get_role_name()}] {node.get_name()!r}"]
    for index in range(node.get_child_count()):
        child = node.get_child_at_index(index)
        if child is not None:
            lines += tree(child, depth + 1)
    return lines

def nodes(node):
    yield node
    for index in range(node.get_child_count()):
        child = node.get_child_at_index(index)
        if child is not None:
            yield from nodes(child)

def client():
    desktop = Atspi.get_desktop(0)
    for index in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(index)
        if app is not None and app.get_name() == "crowbot":
            return app
    return None

failures = []
deadline = time.monotonic() + 20
while True:
    app = client()
    buttons = {}
    if app is not None:
        buttons = {
            node.get_name(): node.get_role_name()
            for node in nodes(app)
            if node.get_role() == Atspi.Role.BUTTON
        }
    if "Close" in buttons or time.monotonic() > deadline:
        break
    time.sleep(0.5)

if app is None:
    print("fail: the client is not on the accessibility bus")
    sys.exit(1)
print("\n".join(tree(app)))
frames = [node for node in nodes(app) if node.get_role() == Atspi.Role.FRAME]
if not any(frame.get_name() == "Crow Bot" for frame in frames):
    failures.append("no window named Crow Bot")
for label in ("Close", "Minimize"):
    if label not in buttons:
        failures.append(f"no button named {label}")
if "Maximize" not in buttons and "Restore" not in buttons:
    failures.append("no button named Maximize or Restore")
if failures:
    print("fail: " + "; ".join(failures))
    sys.exit(1)
print("ok: the window and its buttons read with their roles and labels")
EOF
if [ "$status" -ne 0 ]; then
  grep -v 'AddAccessible with unknown signature' "$runtime/atspi.log" >&2 || true
  tail -n 20 "$runtime/client.log" >&2
fi
exit "$status"
