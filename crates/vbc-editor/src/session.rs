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

pub mod client;
pub mod error;
pub mod event;
pub mod frame;
pub mod identity;
pub mod probe;
pub mod spawn;
