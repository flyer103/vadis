#!/bin/sh
# Supervise one command, record catchable termination signals, never restart it.
set -eu

if [ "$#" -eq 0 ]; then
    cat >&2 <<'EOF'
Usage: vadis-serve-wrapper.sh BINARY [ARG ...]

Environment:
  VADIS_DEATH_LOG  Append-only signal log. Default:
                   ${XDG_STATE_HOME:-$HOME/.local/state}/vadis/death.log
EOF
    exit 64
fi

if [ -n "${VADIS_DEATH_LOG:-}" ]; then
    death_log=$VADIS_DEATH_LOG
else
    death_log=${XDG_STATE_HOME:-$HOME/.local/state}/vadis/death.log
fi

umask 077
mkdir -p "$(dirname "$death_log")"
touch "$death_log"
started_at=$(date +%s)
recorded=0
received_signal=
child_pid=

record_death() {
    signal=$1
    if [ "$recorded" -eq 0 ]; then
        recorded=1
        received_signal=$signal
        now=$(date +%s)
        timestamp=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
        printf 'timestamp=%s pid=%s child_pid=%s signal=%s uptime_seconds=%s\n' \
            "$timestamp" "$$" "${child_pid:-unstarted}" "$signal" "$((now - started_at))" >>"$death_log"
    fi

    if [ -n "$child_pid" ]; then
        kill -s "$signal" "$child_pid" 2>/dev/null || true
    fi
}

trap 'record_death TERM' TERM
trap 'record_death INT' INT
trap 'record_death HUP' HUP

# The wrapper must remain present to catch and forward a signal; exec here would
# replace the very process that owns the traps and make the death line impossible.
"$@" &
child_pid=$!

while :; do
    set +e
    wait "$child_pid"
    status=$?
    set -e

    # A trapped signal interrupts wait while the child can still be alive. Wait
    # again rather than mistaking that interruption for the child's exit.
    if kill -0 "$child_pid" 2>/dev/null; then
        continue
    fi

    if [ -n "$received_signal" ]; then
        case $received_signal in
            HUP) exit 129 ;;
            INT) exit 130 ;;
            TERM) exit 143 ;;
        esac
    fi
    exit "$status"
done
