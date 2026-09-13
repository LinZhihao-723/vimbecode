#!/bin/sh
# A stand-in for the `claude` binary that answers every turn one block at a time.
#
# It speaks what `stub.sh` speaks over the same pipes and differs in one thing: a turn is answered
# as `drip` assistant frames rather than one, `pause` seconds apart, so a test can watch the panel
# take in one arriving block after another while the turn is still running. Both are read from
# files in the directory it was started in, and default to 50 frames 0.03 s apart. It writes every
# line it is sent to `heard`, as `stub.sh` does.

session=""
while [ $# -gt 0 ]; do
    case "$1" in
        --session-id) session="$2"; shift 2 ;;
        --resume) session="$2"; shift 2 ;;
        *) shift ;;
    esac
done

count=50
if [ -f drip ]; then
    count=$(cat drip)
fi
pause=0.03
if [ -f pause ]; then
    pause=$(cat pause)
fi

here=$(pwd)
tools='"Bash","Read","Edit","AskUserQuestion","EnterPlanMode","ExitPlanMode"'
turn=0
while IFS= read -r line; do
    if [ -z "$line" ]; then
        continue
    fi
    printf '%s\n' "$line" >> heard
    turn=$((turn + 1))

    printf '{"type":"system","subtype":"init","session_id":"%s","claude_code_version":"stub",' "$session"
    printf '"model":"stub","cwd":"%s","permissionMode":"default","tools":[%s],' "$here" "$tools"
    printf '"capabilities":["interrupt_receipt_v1"],"slash_commands":["clear"]}\n'

    drop=1
    while [ "$drop" -le "$count" ]; do
        printf '{"type":"assistant","session_id":"%s","parent_tool_use_id":null,' "$session"
        printf '"message":{"role":"assistant","content":[{"type":"text",'
        printf '"text":"drop %s of turn %s"}]}}\n' "$drop" "$turn"
        sleep "$pause"
        drop=$((drop + 1))
    done

    printf '{"type":"result","subtype":"success","session_id":"%s","is_error":false,' "$session"
    printf '"num_turns":%s,"result":"turn %s","permission_denials":[]}\n' "$turn" "$turn"
done
