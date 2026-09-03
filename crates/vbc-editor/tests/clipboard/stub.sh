#!/bin/sh
# A stand-in for the clipboard helper, speaking the same framed protocol over the same pipes.
#
# The real helper is PowerShell talking to Windows, which the machines this workspace's tests run
# on do not have. What the lifecycle tests need of a helper is not a clipboard at all: they need a
# real process that can be killed, that records every time it was started and every request it was
# handed, and that can be told to fail and told to stop failing. That is what this is.
#
# It is steered by the environment and by files, so a test changes its behaviour without restarting
# it:
#
#   VBC_STUB_SPAWN_LOG        a line holding the pid is appended to this on every start
#   VBC_STUB_REQUEST_LOG      a line of nanoseconds since the epoch, per request served
#   VBC_STUB_FAIL_FLAG        while this file exists, every request is answered "failed"
#   VBC_STUB_DEATH_FLAG       while this file exists, the stub exits instead of answering
#   VBC_STUB_STARTUP_DELAY_MS milliseconds spent starting up before the first answer
#   VBC_STUB_TEXT             the text a read is answered with

set -u

TEXT_STATUS=1
FAILED_STATUS=4
STORED_STATUS=5

WRITE_TAG=2

if [ -n "${VBC_STUB_SPAWN_LOG:-}" ]; then
    echo "$$" >>"$VBC_STUB_SPAWN_LOG"
fi

if [ -n "${VBC_STUB_STARTUP_DELAY_MS:-}" ]; then
    sleep "$(awk "BEGIN { print $VBC_STUB_STARTUP_DELAY_MS / 1000 }")"
fi

put_frame() {
    status="$1"
    body="$2"
    length=$((${#body} + 1))
    printf "$(printf '\\%03o\\%03o\\%03o\\%03o\\%03o' \
        $((length / 16777216 % 256)) \
        $((length / 65536 % 256)) \
        $((length / 256 % 256)) \
        $((length % 256)) \
        "$status")"
    printf '%s' "$body"
}

payload=$(mktemp)
trap 'rm -f "$payload"' EXIT

while :; do
    length=$(dd bs=1 count=4 2>/dev/null | od -An -tu4 -N4 --endian=big | tr -d ' \n')
    if [ -z "$length" ] || [ "$length" -eq 0 ]; then
        exit 0
    fi

    dd bs=1 count="$length" of="$payload" 2>/dev/null
    tag=$(od -An -tu1 -N1 "$payload" | tr -d ' \n')

    if [ -n "${VBC_STUB_REQUEST_LOG:-}" ]; then
        date +%s%N >>"$VBC_STUB_REQUEST_LOG"
    fi

    if [ -n "${VBC_STUB_DEATH_FLAG:-}" ] && [ -e "$VBC_STUB_DEATH_FLAG" ]; then
        exit 3
    fi

    if [ -n "${VBC_STUB_FAIL_FLAG:-}" ] && [ -e "$VBC_STUB_FAIL_FLAG" ]; then
        put_frame "$FAILED_STATUS" "the clipboard is held by another process"
    elif [ "$tag" = "$WRITE_TAG" ]; then
        put_frame "$STORED_STATUS" ""
    else
        put_frame "$TEXT_STATUS" "${VBC_STUB_TEXT:-}"
    fi
done
