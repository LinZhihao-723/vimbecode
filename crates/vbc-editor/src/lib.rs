//! The vimbecode editor.
//!
//! Owns the buffers, the vim-style modal state machine, and the commands that act on them, all
//! rendered through the layout engine.

pub mod app;
pub mod clipboard;
pub mod engine;
pub mod event;
pub mod gutter;
pub mod keys;
pub mod render;
pub mod screen;
pub mod shim;
pub mod style;
