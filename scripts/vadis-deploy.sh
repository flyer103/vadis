#!/bin/sh
# Replace a vadis executable without ever writing into its current inode.
set -eu

usage() {
    cat >&2 <<'EOF'
Usage: vadis-deploy.sh NEW_BINARY INSTALLED_BINARY

Environment:
  VADIS_DEPLOY_STOP   Shell command that stops the service and waits for exit.
  VADIS_DEPLOY_START  Shell command that starts the service (optional).

The installed binary's directory must be writable. The stop command is run when
one is supplied; execution is checked again before an atomic same-filesystem mv.
EOF
    exit 64
}

[ "$#" -eq 2 ] || usage
new_binary=$1
installed_binary=$2

[ -f "$new_binary" ] && [ -x "$new_binary" ] || {
    printf 'vadis-deploy: new binary is not an executable regular file: %s\n' "$new_binary" >&2
    exit 66
}

case $installed_binary in
    */*) ;;
    *) installed_binary=./$installed_binary ;;
esac
installed_dir=$(dirname "$installed_binary")
[ -d "$installed_dir" ] && [ -w "$installed_dir" ] || {
    printf 'vadis-deploy: installed binary directory is not writable: %s\n' "$installed_dir" >&2
    exit 77
}

identity() {
    path=$1
    if [ ! -e "$path" ]; then
        printf 'path=%s identity=absent\n' "$path"
        return
    fi

    if stat -f 'device=%d inode=%i' "$path" >/dev/null 2>&1; then
        location=$(stat -f 'device=%d inode=%i' "$path")
    else
        location=$(stat -c 'device=%d inode=%i' "$path")
    fi

    if command -v shasum >/dev/null 2>&1; then
        digest=$(shasum -a 256 "$path" | awk '{print $1}')
    else
        digest=$(sha256sum "$path" | awk '{print $1}')
    fi
    printf 'path=%s %s sha256=%s\n' "$path" "$location" "$digest"
}

# lsof also sees an interpreted executable's open script, unlike a txt-only
# check. Refusing any open handle is conservative: a reader can stop a deploy,
# but an unnoticed executor can never have its inode overwritten.
open_pids() {
    [ -e "$installed_binary" ] || return 1
    command -v lsof >/dev/null 2>&1 || {
        printf 'vadis-deploy: lsof is required to check whether %s is executing\n' "$installed_binary" >&2
        exit 69
    }

    lsof -nP -t -- "$installed_binary" 2>/dev/null | sort -u
}

refuse_if_open() {
    pids=$(open_pids) || {
        status=$?
        [ "$status" -eq 1 ] && return 0
        return "$status"
    }
    if [ -n "$pids" ]; then
        printf 'vadis-deploy: refusing to replace an executing or open binary; pids=%s\n' \
            "$(printf '%s' "$pids" | tr '\n' ',' | sed 's/,$//')" >&2
        return 1
    fi
}

printf 'source: '
identity "$new_binary"
printf 'old: '
identity "$installed_binary"

if pids=$(open_pids) && [ -n "$pids" ] && [ -z "${VADIS_DEPLOY_STOP:-}" ]; then
    printf 'vadis-deploy: refusing to replace an executing or open binary; pids=%s (set VADIS_DEPLOY_STOP to stop it first)\n' \
        "$(printf '%s' "$pids" | tr '\n' ',' | sed 's/,$//')" >&2
    exit 75
fi

if [ -n "${VADIS_DEPLOY_STOP:-}" ]; then
    printf 'stop: %s\n' "$VADIS_DEPLOY_STOP"
    VADIS_DEPLOY_TARGET=$installed_binary /bin/sh -c "$VADIS_DEPLOY_STOP"
fi

# Detect after stopping too. The atomic mv below remains necessary even after
# this check: it closes the check/replace race without modifying a live inode.
if ! refuse_if_open; then
    exit 75
fi

staged=$(mktemp "$installed_dir/.vadis-deploy.XXXXXX")
cleanup() {
    [ -z "${staged:-}" ] || rm -f "$staged"
}
trap cleanup EXIT HUP INT TERM

cp -p "$new_binary" "$staged"
mv -f "$staged" "$installed_binary"
staged=

# Shell scripts and other non-Mach-O test executables need no signature. Real
# macOS Mach-O binaries are ad-hoc signed only after their new inode is in place.
if [ "$(uname -s)" = Darwin ]; then
    magic=$(od -An -tx1 -N4 "$installed_binary" | tr -d ' \n')
    case $magic in
        feedface|feedfacf|cefaedfe|cffaedfe|cafebabe|cafebabf)
            codesign --force --sign - "$installed_binary"
            ;;
    esac
fi

printf 'new: '
identity "$installed_binary"

if [ -n "${VADIS_DEPLOY_START:-}" ]; then
    printf 'start: %s\n' "$VADIS_DEPLOY_START"
    VADIS_DEPLOY_TARGET=$installed_binary /bin/sh -c "$VADIS_DEPLOY_START"
fi
