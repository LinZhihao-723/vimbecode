#!/bin/sh
# A stand-in for the `claude` binary, speaking the same stream protocol over the same pipes.
#
# It exists so that the client can be driven end to end by a test that needs no network, no login
# and no model: what it answers is fixed, and what it is worth is that everything around the answer
# is real -- a process of its own, pipes of its own, NDJSON in and NDJSON out, and an environment
# it writes down so a test can read what the child was actually handed rather than what the spawn
# believes it handed over.
#
# Everything it is steered by and everything it records lives in the directory it was started in,
# which is the one thing about a spawn a test can choose through the client's own interface. It
# writes `args` and `env` there, and it reads three files from there:
#
#   reject  refuse the permission flag and exit, as a release that dropped it would
#   drop    take the permission flag and do nothing about it
#   hooked  speak before announcing itself, as a session with a SessionStart hook does
#   said.N  answer the Nth turn with the frames this file holds instead of the canned ones
#
# The last of those is what a test of the translation layer drives: the frames a real session wrote
# are recorded into a file, the stand-in writes them back out over the same pipes, and everything
# between the recording and the blocks -- the spawn, the framing, the decode -- is the client's own
# rather than a fixture's.
#
# All three mirror what the real binary does rather than inventing a failure. Told to reject, it
# writes commander's own "unknown option" line and exits, which is what claude does with a flag it
# has never heard of. Told to drop, it starts normally and leaves the tools that need somewhere to
# ask a question out of its catalog -- which is exactly how claude 2.1.263's own catalog differs
# with the flag and without it. Told it is hooked, it writes the frame a SessionStart hook puts
# ahead of the init frame, because on a machine that configures one the init frame is not the first
# thing a real session says.

printf '%s\n' "$@" > args
env > env

session=""
resume=""
fork="no"
permission=""
while [ $# -gt 0 ]; do
    case "$1" in
        --session-id) session="$2"; shift 2 ;;
        --resume) resume="$2"; shift 2 ;;
        --fork-session) fork="yes"; shift ;;
        --permission-prompt-tool) permission="$2"; shift 2 ;;
        *) shift ;;
    esac
done

if [ -n "$permission" ] && [ -f reject ]; then
    printf "error: unknown option '--permission-prompt-tool'\n" >&2
    exit 1
fi

if [ -n "$permission" ] && [ ! -f drop ]; then
    tools='"Bash","Read","Edit","AskUserQuestion","EnterPlanMode","ExitPlanMode"'
else
    tools='"Bash","Read","Edit"'
fi

sid="$session"
if [ -z "$sid" ]; then
    sid="$resume"
fi
if [ "$fork" = "yes" ]; then
    sid="11111111-1111-4111-8111-111111111111"
fi

here=$(pwd)
turn=0
while IFS= read -r line; do
    if [ -z "$line" ]; then
        continue
    fi
    turn=$((turn + 1))

    if [ -f "said.$turn" ]; then
        cat "said.$turn"
        continue
    fi

    if [ -f hooked ]; then
        printf '{"type":"system","subtype":"hook_started","session_id":"%s",' "$sid"
        printf '"hook_name":"SessionStart:startup","hook_event":"SessionStart"}\n'
    fi

    printf '{"type":"system","subtype":"init","session_id":"%s","claude_code_version":"stub",' "$sid"
    printf '"model":"stub","cwd":"%s","permissionMode":"default","tools":[%s],' "$here" "$tools"
    printf '"capabilities":["interrupt_receipt_v1"],"slash_commands":["clear"]}\n'

    printf '{"type":"assistant","session_id":"%s","parent_tool_use_id":null,' "$sid"
    printf '"message":{"role":"assistant","content":[{"type":"text","text":"turn %s"}]}}\n' "$turn"

    printf '{"type":"result","subtype":"success","session_id":"%s","is_error":false,' "$sid"
    printf '"num_turns":%s,"result":"turn %s","permission_denials":[]}\n' "$turn" "$turn"
done
