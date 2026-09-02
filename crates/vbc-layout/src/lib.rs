//! Text layout for vimbecode.
//!
//! Turns a logical buffer into the wrapped lines rendered on screen, and maps positions between
//! the two coordinate spaces.
//!
//! The crate hands out the parts a layout is assembled from and the invariants one is held to, and
//! deliberately no layout of its own: the obvious one lays every line of the buffer out on every
//! call, which is what [`anchor`] exists to avoid, so the only such layout lives in the tests.

pub mod anchor;
pub mod buffer;
pub mod invariants;
pub mod line;
pub mod position;
pub mod viewport;
pub mod width;
