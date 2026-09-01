//! The differential test harness for the vimbecode editor.
//!
//! Replays a sequence of keystrokes against both the editor and a real vim process, and reports
//! where the two disagree.

pub mod corpus;
pub mod runner;
pub mod state;
pub mod vim;
