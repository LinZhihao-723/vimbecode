#!/bin/sh
# A stand-in for the clipboard writer, which keeps every byte it was handed instead of putting them
# on a clipboard.
#
# The real writer is `clip.exe`, and what the write path owes it is an exact sequence of bytes:
# UTF-16LE, little end first, with no byte order mark in front of them. Whether those are the bytes
# that left is a question about this side of the pipe alone, and it has an answer on any machine,
# which is what this is for. Windows is asked the other question -- whether those bytes come back as
# the same text -- and it can only be asked where there is a Windows to ask.
#
#   $1  the file every byte of standard input is written to, verbatim
#   $2  when it is "refuse", the writer exits non-zero instead of taking the text

set -u

if [ "${2:-}" = "refuse" ]; then
    exit 1
fi

cat >"$1"
