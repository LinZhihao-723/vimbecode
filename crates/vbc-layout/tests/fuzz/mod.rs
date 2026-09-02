//! The layout fuzz harness: the case generator and search runner, the reference layout a search
//! is run over, and the deliberately broken layouts that prove each invariant can fail.

pub mod harness;
pub mod reference;
pub mod violations;
