//! The system clipboard the editor yanks to and pastes from.
//!
//! Under WSL the system clipboard is Windows', and every way of reaching it from a Linux process
//! costs a Windows process. The interop floor alone is around sixty milliseconds and a one-shot
//! `Get-Clipboard` costs about two hundred and thirty, so a clipboard served by spawning something
//! stalls the editor visibly on every paste. It is served by a long-lived helper instead, spoken
//! to over a pipe that stays open for the life of the session, and this module holds the language
//! that pipe carries.
//!
//! Three things make that up: the wire, which is the frames the two sides exchange and the answers
//! a request can come back with; the helper's life, which is the process those frames travel
//! between and the rules about starting it, keeping it and letting it go; and the reading, which
//! is the worker thread every one of those exchanges happens on and the deadlines the render loop
//! holds them to.

pub mod helper;
pub mod protocol;
pub mod reader;
