//! The transcript of a conversation, modelled as the things that were said rather than as a grid
//! of cells.
//!
//! A transcript is not text. It is a sequence of semantic blocks -- a message, a fenced code
//! block, a call to a tool, what the tool answered, a thinking block, a diff -- and the whole
//! reason to put a vim editor in front of one is that a motion should address those blocks as
//! objects instead of addressing the cells they happen to be drawn in.
//!
//! Every block holds the source it was built from and the spans styling it, each of which names a
//! byte range of that source. Rendering is a projection over a window of those bytes and the
//! source is the truth: a rendered row names the bytes it draws, so a selection is a range of the
//! source and a yank is a slice of the string the block was built from, with nothing to un-render.
//!
//! Two kinds of block are not simply the text they arrived as, and both are here because dropping
//! either would lose what the block was saying. A diff is computed from the text an edit replaced
//! and the text it wrote, so that a reader sees the lines that changed rather than both versions
//! reprinted whole. And tool output arrives coloured with ANSI escapes, which are read as the
//! styles they name, so that neither the escapes nor the colour they carry ends up in the text a
//! reader yanks.
//!
//! Both of those are also where a transcript stops being small. Tool output is whatever `cargo`
//! wrote, and an edit is to whatever file was edited, so neither the rendering of a block nor the
//! diffing of one may cost what the block holds: rendering costs the window it is asked for and
//! diffing costs bounded memory, both measured rather than argued.

pub mod ansi;
pub mod block;
pub mod diff;
pub mod fold;
pub mod transcript;
