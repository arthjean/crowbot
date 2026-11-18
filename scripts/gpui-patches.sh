#!/bin/sh
# Keeps the GPUI crates this repository changes in step with Zed.
#
# crates/crowbot-desktop/patches/<crate> is Zed's crate at the rev the client
# pins, with the changes recorded in patches/<crate>.diff. The client takes it
# through [patch] in its Cargo.toml. See crates/crowbot-desktop/patches/README.md.
#
#   sh scripts/gpui-patches.sh check     the copy is upstream plus the diff, nothing else
#   sh scripts/gpui-patches.sh diff      write the diff again after a change to the copy
#   sh scripts/gpui-patches.sh refresh   after a rev bump: take the new upstream, apply the diff

set -eu

usage() {
  echo "usage: ${0##*/} check|diff|refresh" >&2
  exit 2
}

[ $# -eq 1 ] || usage
action=$1
case $action in
  check | diff | refresh) ;;
  *) usage ;;
esac

repo=$(cd "$(dirname "$0")/.." && pwd)
client=$repo/crates/crowbot-desktop
patches=$client/patches
crates="gpui_wgpu"

# The rev of Zed the client pins.
rev=$(sed -n 's/^gpui = { git = "https:\/\/github.com\/zed-industries\/zed", rev = "\([0-9a-f]*\)".*/\1/p' "$client/Cargo.toml" | head -n 1)
[ -n "$rev" ] || { echo "no Zed rev in $client/Cargo.toml" >&2; exit 1; }

# Zed's source at that rev, where Cargo keeps its git checkouts.
upstream_crates() {
  host=$(rustc -vV | sed -n 's/^host: //p')
  cargo metadata --manifest-path "$client/Cargo.toml" --format-version 1 --filter-platform "$host" |
    python3 -I -c '
import json, os, sys
rev = sys.argv[1]
for package in json.load(sys.stdin)["packages"]:
    if package["name"] == "gpui" and rev in (package.get("source") or ""):
        print(os.path.dirname(os.path.dirname(package["manifest_path"])))
        break
' "$rev"
}
upstream=$(upstream_crates)
[ -d "$upstream" ] || { echo "Zed's source at $rev is not in Cargo's cache: run cargo fetch" >&2; exit 1; }

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# The diff of a crate's sources, upstream to the copy, with stable paths.
crate_diff() {
  mkdir -p "$scratch/$1/a" "$scratch/$1/b"
  cp -R "$upstream/$1/src" "$scratch/$1/a/src"
  cp -R "$patches/$1/src" "$scratch/$1/b/src"
  (cd "$scratch/$1" && git diff --no-index --no-color --no-prefix a b || true)
}

status=0
for crate in $crates; do
  case $action in
    check)
      crate_diff "$crate" >"$scratch/$crate.diff"
      if ! cmp -s "$scratch/$crate.diff" "$patches/$crate.diff"; then
        echo "$crate: the copy is not Zed's $rev plus patches/$crate.diff" >&2
        diff -u "$patches/$crate.diff" "$scratch/$crate.diff" >&2 || true
        status=1
      fi
      grep -q "rev = \"$rev\"" "$patches/$crate/Cargo.toml" ||
        { echo "$crate: its Cargo.toml does not name rev $rev" >&2; status=1; }
      ;;
    diff)
      crate_diff "$crate" >"$patches/$crate.diff"
      echo "wrote $patches/$crate.diff"
      ;;
    refresh)
      work=$scratch/$crate
      mkdir -p "$work"
      cp -R "$upstream/$crate/src" "$work/src"
      (cd "$work" && git apply -p1 "$patches/$crate.diff") || {
        echo "$crate: the diff does not apply to Zed's $rev; resolve it in a copy of $upstream/$crate" >&2
        exit 1
      }
      rm -rf "$patches/$crate/src"
      cp -R "$work/src" "$patches/$crate/src"
      sed -i "s/\(zed-industries\/zed\", rev = \"\)[0-9a-f]\{40\}/\1$rev/" "$patches/$crate/Cargo.toml"
      echo "$crate: now Zed's $rev plus patches/$crate.diff; compare its Cargo.toml with $upstream/$crate/Cargo.toml"
      ;;
  esac
done
exit $status
