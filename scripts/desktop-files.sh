#!/bin/sh
# Puts Crow Bot in the application grid of a Linux desktop, on the client built
# from this repository.
#
# The desktop entry and the icons of crates/crowbot-desktop/resources/linux go
# under ${XDG_DATA_HOME:-~/.local/share}, and the entry starts the built
# client, which starts crowbotd from its own folder. The list of the files
# copied is kept in ${XDG_STATE_HOME:-~/.local/state}/crowbot/desktop-files,
# and `remove` deletes exactly those.
#
#   sh scripts/desktop-files.sh                    build, then install on target/debug
#   sh scripts/desktop-files.sh install --release  build, then install on target/release
#   sh scripts/desktop-files.sh install --client <path>  install on a client already built
#   sh scripts/desktop-files.sh remove             delete every file installed

set -eu

usage() {
  echo "usage: ${0##*/} [install|remove] [--release] [--client <path>]" >&2
  exit 2
}

action=install
profile=debug
client=
while [ $# -gt 0 ]; do
  case $1 in
    install | remove) action=$1 ;;
    --release) profile=release ;;
    --client)
      [ $# -ge 2 ] || usage
      client=$2
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

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
resources=$repo/crates/crowbot-desktop/resources/linux

absolute_or() {
  case ${1:-} in
    /*) printf '%s' "$1" ;;
    *) printf '%s' "$2" ;;
  esac
}
data=$(absolute_or "${XDG_DATA_HOME:-}" "$HOME/.local/share")
state=$(absolute_or "${XDG_STATE_HOME:-}" "$HOME/.local/state")/crowbot
manifest=$state/desktop-files

set -- "$resources"/applications/*.desktop
if [ $# -ne 1 ] || [ ! -f "$1" ]; then
  echo "expected one desktop entry in $resources/applications" >&2
  exit 1
fi
entry=$1
app_id=$(basename "$entry" .desktop)

refresh() {
  if [ -f "$data/icons/hicolor/icon-theme.cache" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$data/icons/hicolor" || true
  fi
  if [ -d "$data/icons/hicolor" ]; then
    touch "$data/icons/hicolor"
  fi
}

remove_files() {
  if [ -f "$manifest" ]; then
    while IFS= read -r path; do
      case ${path##*/} in
        "$app_id".*) rm -f -- "$path" ;;
      esac
    done <"$manifest"
    rm -f -- "$manifest"
    rmdir -- "$state" 2>/dev/null || true
  else
    (cd "$resources" && find . -type f) | while IFS= read -r relative; do
      rm -f -- "$data/${relative#./}"
    done
  fi
}

desktop_string() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g'
}

exec_argument() {
  printf '"%s"' "$(printf '%s' "$1" | sed -e 's/[\\"`$]/\\&/g' -e 's/\\/\\\\/g' -e 's/%/%%/g')"
}

write_entry() {
  while IFS= read -r line; do
    case $line in
      Exec=*)
        printf 'Exec=%s\n' "$(exec_argument "$client")"
        printf 'TryExec=%s\n' "$(desktop_string "$client")"
        ;;
      TryExec=*) ;;
      *) printf '%s\n' "$line" ;;
    esac
  done <"$entry"
}

if [ "$action" = remove ]; then
  remove_files
  refresh
  echo "Removed the desktop files of $app_id from $data."
  exit 0
fi

if [ -z "$client" ]; then
  target=${CARGO_TARGET_DIR:-$repo/target}
  release=
  [ "$profile" = release ] && release=--release
  (
    cd "$repo"
    export CARGO_TARGET_DIR="$target"
    cargo build $release -p crowbotd
    cargo build $release --manifest-path crates/crowbot-desktop/Cargo.toml
  )
  client=$target/$profile/crowbot
fi
case $client in
  *'
'*)
    echo "the client path holds a line break" >&2
    exit 1
    ;;
esac
if [ ! -f "$client" ] || [ ! -x "$client" ]; then
  echo "no client to start at $client: build it, or pass --client <path>" >&2
  exit 1
fi
client=$(CDPATH='' cd -- "$(dirname -- "$client")" && pwd)/$(basename -- "$client")
if [ ! -e "$(dirname -- "$client")/crowbotd" ]; then
  echo "warning: no crowbotd next to $client; the client starts the daemon from its own folder" >&2
fi

remove_files
mkdir -p -- "$state"
: >"$manifest"
(cd "$resources" && find . -type f | sort) | while IFS= read -r relative; do
  relative=${relative#./}
  destination=$data/$relative
  mkdir -p -- "$(dirname -- "$destination")"
  staged=$destination.partial
  if [ "$resources/$relative" = "$entry" ]; then
    write_entry >"$staged"
  else
    cp -- "$resources/$relative" "$staged"
  fi
  chmod 644 "$staged"
  mv -f -- "$staged" "$destination"
  printf '%s\n' "$destination" >>"$manifest"
done

installed=$data/applications/$app_id.desktop
if command -v desktop-file-validate >/dev/null 2>&1; then
  if ! report=$(desktop-file-validate "$installed" 2>&1) || [ -n "$report" ]; then
    echo "$report" >&2
    remove_files
    echo "the installed desktop entry does not validate; nothing is left installed" >&2
    exit 1
  fi
fi
refresh
echo "Installed $app_id under $data, starting $client."
echo "Remove it with: sh scripts/desktop-files.sh remove"
