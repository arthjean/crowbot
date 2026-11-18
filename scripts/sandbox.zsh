#!/usr/bin/env zsh
# Builds Crow Bot and starts the client on a throwaway daemon, apart from your
# own data.
#
# The sandbox has its own socket, database, log, bot folders, runtimes, and
# provider homes, under ${XDG_STATE_HOME:-~/.local/state}/crowbot-sandbox/<name>,
# and never touches the systemd unit of your own daemon. Sign-ins
# made there stay there until you clean the sandbox. The client starts the
# sandbox daemon, which keeps running after the window closes: stop it with
# `stop`.
#
#   scripts/sandbox.zsh                 build, then open the client on the sandbox
#   scripts/sandbox.zsh stop            stop the sandbox daemon and client
#   scripts/sandbox.zsh reset           wipe the sandbox, then start it fresh
#   scripts/sandbox.zsh clean           stop and delete the sandbox
#   scripts/sandbox.zsh start --name x  several sandboxes live side by side

emulate -L zsh
setopt err_exit no_unset pipe_fail

usage() {
  print -u2 "usage: ${0:t} [start|stop|reset|clean] [--name <name>]"
  exit 2
}

action=start
name=default
while (( $# )); do
  case $1 in
    start|stop|reset|clean) action=$1 ;;
    --name)
      (( $# >= 2 )) || usage
      name=$2
      shift
      ;;
    -h|--help) usage ;;
    *) print -u2 "unexpected argument: $1"; usage ;;
  esac
  shift
done
[[ $name =~ '^[A-Za-z0-9_-]+$' ]] || { print -u2 "a name holds letters, digits, - and _ only"; exit 2 }

repo=${0:A:h:h}
sandbox=${XDG_STATE_HOME:-$HOME/.local/state}/crowbot-sandbox/$name
socket=$sandbox/crowbotd.sock

# Both workspaces build into one folder, so the client and the daemon sit side by side.
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$repo/target}
daemon=$CARGO_TARGET_DIR/debug/crowbotd
client=$CARGO_TARGET_DIR/debug/crowbot

# The processes of this sandbox: the daemon and the client both carry its socket
# in their command line.
sandbox_pids() {
  pgrep -f -- "--endpoint $socket( |\$)" || true
}

# The agents the daemon started from the runtimes it installed in the sandbox.
# They end with the daemon; this finds one that left its process group.
agent_pids() {
  pgrep -f -- "$sandbox/data/" || true
}

# Stops what a function lists, then forces what is left after ten seconds. The
# daemon ends its turns on SIGTERM and exits within five seconds.
stop_listed() {
  local list=$1
  local -a pids
  pids=(${(f)"$($list)"})
  (( ${#pids} )) || return 1
  kill $pids 2>/dev/null || true
  local tries=0
  while (( tries < 50 )) && [[ -n "$($list)" ]]; do
    sleep 0.2
    (( tries += 1 ))
  done
  pids=(${(f)"$($list)"})
  (( ${#pids} )) && kill -9 $pids 2>/dev/null || true
  return 0
}

stop_sandbox() {
  local stopped=0
  stop_listed sandbox_pids && stopped=1
  stop_listed agent_pids && stopped=1
  (( stopped )) && print "Stopped the sandbox '$name'."
  return 0
}

case $action in
  stop)
    stop_sandbox
    exit 0
    ;;
  clean)
    stop_sandbox
    rm -rf -- $sandbox
    print "Removed $sandbox."
    exit 0
    ;;
  reset)
    stop_sandbox
    rm -rf -- $sandbox
    ;;
esac

stop_sandbox

cd $repo
cargo build -p crowbotd
cargo build --manifest-path crates/crowbot-desktop/Cargo.toml

# The database and the log can hold your conversations: only you read them.
mkdir -p -m 700 $sandbox
chmod 700 $sandbox
print "Sandbox '$name'
  endpoint  $socket
  data      $sandbox
The daemon keeps running after the window closes: scripts/sandbox.zsh stop --name $name"

exec $client --endpoint $socket --daemon $daemon -- \
  --db $sandbox/crowbot.db \
  --bots-dir $sandbox/bots \
  --data-dir $sandbox/data
