//! The vimbecode editor.
//!
//! Owns the buffers, the vim-style modal state machine, and the commands that act on them, all
//! rendered through the layout engine.

pub mod event;
pub mod render;
