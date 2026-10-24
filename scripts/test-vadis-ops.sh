#!/bin/sh
# Integration test for the ops scripts. Uses no vadis binary, config, or port.
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
deploy=$repo_dir/scripts/vadis-deploy.sh
wrapper=$repo_dir/scripts/vadis-serve-wrapper.sh
test_root=$(mktemp -d "${TMPDIR:-/tmp}/vadis-ops-test.XXXXXX")
child_pid=
wrapper_pid=

cleanup() {
    trap - EXIT HUP INT TERM
    if [ -n "$wrapper_pid" ]; then
        kill -KILL "$wrapper_pid" 2>/dev/null || true
        wait "$wrapper_pid" 2>/dev/null || true
    fi
    if [ -n "$child_pid" ]; then
        kill -KILL "$child_pid" 2>/dev/null || true
        wait "$child_pid" 2>/dev/null || true
    fi
    rm -rf "$test_root"
}
trap cleanup EXIT HUP INT TERM

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

assert_contains() {
    haystack=$1
    needle=$2
    case $haystack in
        *"$needle"*) ;;
        *) fail "missing '$needle' in output: $haystack" ;;
    esac
}

inode() {
    if stat -f '%i' "$1" >/dev/null 2>&1; then
        stat -f '%i' "$1"
    else
        stat -c '%i' "$1"
    fi
}

cat >"$test_root/fake-v1" <<'EOF'
#!/bin/sh
trap 'exit 0' HUP INT TERM
while :; do
    sleep 1
done
EOF
cat >"$test_root/fake-v2" <<'EOF'
#!/bin/sh
# A different inode and different bytes stand in for the new build.
trap 'exit 0' HUP INT TERM
while :; do
    sleep 1
done
EOF
chmod +x "$test_root/fake-v1" "$test_root/fake-v2"
cp "$test_root/fake-v1" "$test_root/installed"
installed="$test_root/installed"
old_inode=$(inode "$installed")

printf 'TEST deploy refuses while the fake binary is running\n'
"$installed" &
child_pid=$!
sleep 0.2
set +e
refusal_output=$("$deploy" "$test_root/fake-v2" "$installed" 2>&1)
refusal_status=$?
set -e
[ "$refusal_status" -ne 0 ] || fail 'deploy unexpectedly replaced a running binary'
assert_contains "$refusal_output" 'refusing to replace'
assert_contains "$refusal_output" "inode=$old_inode"
[ "$(inode "$installed")" = "$old_inode" ] || fail 'refusal changed the inode'
printf '%s\n' "$refusal_output"
printf 'PASS refusal status=%s inode=%s\n' "$refusal_status" "$old_inode"

printf 'TEST deploy stops, replaces, and starts without retaining the inode\n'
operations_log=$test_root/deploy-operations.log
success_output=$(VADIS_DEPLOY_STOP="printf 'stop\\n' >>'$operations_log'; kill -TERM '$child_pid'; sleep 1.2" \
    VADIS_DEPLOY_START="printf 'start\\n' >>'$operations_log'" \
    "$deploy" "$test_root/fake-v2" "$installed" 2>&1)
wait "$child_pid"
child_pid=
new_inode=$(inode "$installed")
[ "$new_inode" != "$old_inode" ] || fail 'successful deploy retained the old inode'
assert_contains "$success_output" "old: path=$installed"
assert_contains "$success_output" "new: path=$installed"
assert_contains "$success_output" "inode=$old_inode"
assert_contains "$success_output" "inode=$new_inode"
cmp -s "$test_root/fake-v2" "$installed" || fail 'installed bytes differ from the new build'
operations=$(cat "$operations_log")
[ "$operations" = 'stop
start' ] || fail "deploy hooks ran out of order: $operations"
printf '%s\n' "$success_output"
printf 'PASS replacement old_inode=%s new_inode=%s\n' "$old_inode" "$new_inode"

printf 'TEST wrapper records one caught signal and does not restart\n'
death_log=$test_root/death.log
VADIS_DEATH_LOG=$death_log "$wrapper" "$installed" &
wrapper_pid=$!
attempt=0
while [ ! -f "$death_log" ] && [ "$attempt" -lt 50 ]; do
    sleep 0.1
    attempt=$((attempt + 1))
done
[ -f "$death_log" ] || fail 'wrapper did not become ready'
kill -TERM "$wrapper_pid"
set +e
wait "$wrapper_pid"
wrapper_status=$?
set -e
wrapper_pid=
[ "$wrapper_status" -eq 143 ] || fail "wrapper status was $wrapper_status, expected 143"
[ -f "$death_log" ] || fail 'death log was not created'
line_count=$(wc -l <"$death_log" | tr -d ' ')
[ "$line_count" = 1 ] || fail "expected one death line, found $line_count"
death_line=$(cat "$death_log")
assert_contains "$death_line" 'timestamp='
assert_contains "$death_line" 'pid='
assert_contains "$death_line" 'signal=TERM'
assert_contains "$death_line" 'uptime_seconds='
printf '%s\n' "$death_line"
printf 'PASS signal recorded once; supervised exit status=%s\n' "$wrapper_status"
printf 'ALL TESTS PASSED\n'
