//! Text layout for vimbecode.
//!
//! Turns a logical buffer into the wrapped lines rendered on screen, and maps positions between
//! the two coordinate spaces.

pub mod anchor;
pub mod buffer;
pub mod invariants;
pub mod line;
pub mod screen;
pub mod viewport;
pub mod width;
