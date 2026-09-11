#!/bin/sh
# A stand-in for the `claude` binary, standing in for the one thing about it that cannot be tested
# against the real one without running somebody's repository on this machine.
#
# Headless Claude Code has no workspace-trust dialog. In a directory it has never seen it runs the
# project's `SessionStart` hook -- and it runs it on the way up, before anything it could fail at,
# which is why a spawn that was aborted has already run it. `--setting-sources user` is the one
# flag that takes the project's hooks, its memory and its MCP servers out together. This
# reproduces both of those and nothing else:
#
#   .claude/session-start   the project's own code, run unless the session was restricted
#   fail                    quit the way a session that could not start quits -- after the hook,
#                           which is where the whole of the gate's timing is
#
# `.claude/session-start` stands in for what a project's `.claude/settings.json` declares. What it
# is is beside the point: it is code the project owns and the reader did not write, run by the
# child rather than by the test.
#
# Everything a session is otherwise asked is the other stand-in's to answer, so this hands over to
# it once it has done its own part -- which means what a test reads about the spawn, its arguments
# and its environment is the same file `stub.sh` writes.

restricted="no"
previous=""
for argument in "$@"; do
    if [ "$previous" = "--setting-sources" ] && [ "$argument" = "user" ]; then
        restricted="yes"
    fi
    previous="$argument"
done

if [ "$restricted" = "no" ] && [ -x .claude/session-start ]; then
    ./.claude/session-start
fi

if [ -f fail ]; then
    printf 'the session could not be started\n' >&2
    exit 1
fi

exec "$(dirname "$0")/stub.sh" "$@"
