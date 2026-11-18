#!/bin/sh
# Installs the system packages that building and testing Crow Bot need on
# Fedora or Ubuntu, read from /etc/os-release. Continuous integration runs it
# too, so this list is the one that is known to work.
#
#   sh scripts/linux-packages.sh               what the build and the tests need
#   sh scripts/linux-packages.sh --window-test  also what scripts/window-test.sh needs
#
# It runs the package manager through sudo unless it runs as root.

set -eu

window_test=false
case ${1:-} in
  "") ;;
  --window-test) window_test=true ;;
  *)
    echo "usage: ${0##*/} [--window-test]" >&2
    exit 2
    ;;
esac

as_root=
[ "$(id -u)" -eq 0 ] || as_root=sudo

# shellcheck disable=SC1091 # the file of the computer, not of the repository
. /etc/os-release
case " ${ID:-} ${ID_LIKE:-} " in
  *" fedora "*)
    # The C toolchain, the libraries GPUI links (xcb, xkbcommon, fontconfig,
    # freetype), and what the tests start: ps, kill, and ss.
    packages="gcc gcc-c++ git-core findutils procps-ng util-linux-core iproute
      libxcb-devel libxkbcommon-devel libxkbcommon-x11-devel fontconfig-devel
      freetype-devel"
    # The compositor, Mesa's lavapipe, the X server, and the private bus.
    $window_test && packages="$packages mutter mesa-vulkan-drivers vulkan-loader
      mesa-dri-drivers xorg-x11-server-Xvfb xdotool dbus-daemon"
    # shellcheck disable=SC2086 # one word per package
    $as_root dnf install -y --setopt=install_weak_deps=False $packages
    ;;
  *" ubuntu "* | *" debian "*)
    packages="build-essential ca-certificates git procps iproute2 pkg-config
      libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libfontconfig-dev
      libfreetype-dev"
    $window_test && packages="$packages mutter mesa-vulkan-drivers libvulkan1
      libgl1-mesa-dri xvfb xdotool dbus-daemon"
    $as_root apt-get update
    # shellcheck disable=SC2086
    $as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends $packages
    ;;
  *)
    echo "no package list for ${PRETTY_NAME:-this system}" >&2
    exit 1
    ;;
esac
