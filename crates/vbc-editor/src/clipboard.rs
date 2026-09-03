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
//! between and the rules about starting it, keeping it and letting it go; and the write path,
//! which does not go through the helper at all, because putting text on the clipboard is worth a
//! program that cannot stall where reading it is worth a process that need not start.
//!
//! The round trip across those two is not byte-exact. The writer rewrites every line ending to
//! CRLF, so what is read back is put through the write path's normalization rather than trusted,
//! and what the editor pastes is what the editor yanked even though what sits on the clipboard in
//! between is not.

pub mod clip;
pub mod helper;
pub mod protocol;
