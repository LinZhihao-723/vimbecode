//! The Claude Code session vimbecode drives, spoken over the subprocess stream protocol.
//!
//! vimbecode does not wrap or scrape Claude Code's terminal UI. It owns one long-lived `claude`
//! child per session, writes NDJSON to its stdin and reads NDJSON from its stdout, and renders
//! everything itself. What that costs, and what it buys, is written down in the modules below;
//! three facts shape all of them.
//!
//! The first is that `--permission-prompt-tool stdio` is undocumented and load-bearing. It is
//! absent from `claude --help`, and without it every tool call needing approval is auto-denied by
//! a one-way notification with no way to answer -- under `--permission-mode manual` too, so the
//! flag and not the mode is the gate. A client that lost the flag to a future release would go on
//! working, quietly, while denying everything, so [`probe`] exists to make that loud.
//!
//! The second is that the protocol has no error channel. A line missing `"type":"user"` is dropped
//! by the child with no error and no acknowledgement, so a frame we got wrong is indistinguishable
//! from a turn Claude chose not to answer. [`frame`] therefore refuses to emit such a line at all:
//! the failure is caught on this side, where it can still be reported, because it cannot be caught
//! on the other.
//!
//! The third is that the child inherits our environment, and inheriting `CLAUDE_CODE_*` from a
//! session that is itself Claude Code silently disables the child's transcript persistence -- a
//! completed multi-turn session leaves no file on disk. [`spawn`] strips those and sets the one
//! variable that has no flag, and does both to the [`std::process::Command`] rather than to the
//! process, so nothing about this depends on how vimbecode itself was started.
//!
//! Before any of that there is a fourth, which is not about the protocol but about what running
//! one costs somebody. Headless Claude Code skips the workspace-trust dialog: in a directory it
//! has never seen it reads the project's memory, runs the project's session hook and starts the
//! project's MCP servers without asking and without recording that it did. [`trust`] is the gate
//! that puts the question back, and it runs before the child exists rather than after, because a
//! spawn that failed had already run all three.
//!
//! What comes back out of all that is a stream of frames, and what the chat panel reads is a
//! sequence of blocks it already knows how to draw. [`blocks`] is the whole of the distance
//! between the two: the panel learns no second model, and a transcript built there is one the
//! folds, the text objects and the yanks read without being told a session was behind it.
//!
//! None of that is yet something a program can hold while it draws. Every round trip below is one
//! somebody waits on, and an editor cannot wait: a turn takes minutes and the keys go on arriving
//! through all of it. [`live`] is the session an application owns rather than the session a test
//! drives -- the child read on a thread of its own, the frames folded into blocks as they land,
//! the questions still outstanding drawn under them, and the gate run in the one order that puts
//! it in front of the child rather than behind it.
//!
//! And not all of it comes back out. A session that wants to write a file asks first, and until it
//! is answered it writes nothing else at all -- so [`control`] is what keeps a session running
//! rather than a feature on top of one, and [`queue`] is what holds its questions while a reader
//! decides, one at a time and in whatever order they get to them.

pub mod blocks;
pub mod client;
pub mod control;
pub mod error;
pub mod event;
pub mod frame;
pub mod identity;
pub mod live;
pub mod probe;
pub mod queue;
pub mod spawn;
pub mod stored;
pub mod trust;
