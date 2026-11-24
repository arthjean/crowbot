#!/bin/sh
# Runs the daemon's tests many times at once, to find the ones that fail only
# under load: by default 24 runs in parallel, 20 times in a row. Each run is
# the whole test binary of crates/crowbotd/tests, built once.
#
#   sh scripts/stress-tests.sh                 24 runs at once, 20 rounds
#   sh scripts/stress-tests.sh 8 5             8 runs at once, 5 rounds
#   sh scripts/stress-tests.sh 24 20 <filter>  only the tests whose name holds <filter>
#
# The output of each failed run is kept in a folder of its own, printed at
# the end; nothing is kept when every run passes.

set -eu

parallel=${1:-24}
rounds=${2:-20}
filter=${3:-}
repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

# The test binary, and not the daemon that Cargo builds for it.
binary=$(cargo test --locked --test daemon --no-run --message-format json 2>/dev/null |
  grep '"kind":\["test"\]' | grep '"name":"daemon"' |
  sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -n 1)
[ -x "$binary" ] || { echo "cannot build the daemon tests" >&2; exit 1; }
# The scripted agent the tests start, built once for every run: a run that
# built it again could copy it over itself while another run starts it.
cargo build --locked -p crowbotd --example fake_acp_agent 2>/dev/null
export CROWBOT_FAKE_AGENT_BUILT=1

logs=$(mktemp -d "${TMPDIR:-/tmp}/crowbot-stress.XXXXXX")
failed=0
round=1
while [ "$round" -le "$rounds" ]; do
  pids=
  run=1
  while [ "$run" -le "$parallel" ]; do
    # shellcheck disable=SC2086 # no filter is no argument
    (cd "$repo/crates/crowbotd" && "$binary" $filter >"$logs/$round-$run.log" 2>&1) &
    pids="$pids $!:$round-$run"
    run=$((run + 1))
  done
  round_failed=0
  for entry in $pids; do
    if wait "${entry%%:*}"; then
      rm -f "$logs/${entry#*:}.log"
    else
      round_failed=$((round_failed + 1))
    fi
  done
  failed=$((failed + round_failed))
  echo "round $round of $rounds: $round_failed of $parallel runs failed"
  round=$((round + 1))
done

if [ "$failed" -eq 0 ]; then
  rmdir "$logs"
  echo "ok: $((parallel * rounds)) runs, none failed"
  exit 0
fi
echo "$failed runs failed; their output is in $logs" >&2
grep -h '^    [a-z_:]*$' "$logs"/*.log | sort | uniq -c | sort -rn >&2 || true
exit 1
