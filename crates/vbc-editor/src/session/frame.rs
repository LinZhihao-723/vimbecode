//! What goes into a session: one JSON object, on one line, naming what it is.
//!
//! The protocol has no error channel in this direction. A line the child cannot make sense of --
//! one missing `"type":"user"`, most of all -- is dropped, with no error frame, no acknowledgement
//! and no exit code, so a turn we framed wrongly is indistinguishable from a turn Claude chose not
//! to answer, and the two would be told apart only by waiting for a reply that is never coming.
//!
//! That is why a frame is a type rather than a string. The one thing the child requires and will
//! not complain about is checked here, before the line is written, so the silent drop has nothing
//! to be reached through: a value that names no type is a [`Result`] error on this side instead of
//! nothing at all on the other.

use serde_json::{json, Value};

use super::error::Error;

/// The field every frame the child accepts must carry.
pub const TYPE_FIELD: &str = "type";

/// The type naming a turn taken by the user, which is the frame a message is sent as.
pub const USER_TYPE: &str = "user";

/// One frame bound for the session, which is known to name its type.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame(Value);

impl Frame {
    /// # Returns
    ///
    /// A newly created frame carrying one message from the user.
    ///
    /// # Panics
    ///
    /// Panics if the frame it builds does not name its type, which would mean this function and
    /// [`Frame::checked`] disagree about what a frame is.
    #[must_use]
    pub fn user(text: &str) -> Self {
        Self::checked(json!({
            TYPE_FIELD: USER_TYPE,
            "message": {
                "role": USER_TYPE,
                "content": [{"type": "text", "text": text}],
            },
        }))
        .expect("a user frame names its type")
    }

    /// Takes a value on as a frame, once it is known to be one the child will read.
    ///
    /// # Returns
    ///
    /// The frame on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Untyped`] if the value is not an object, or its type is missing, not a string,
    ///   or empty. The child drops every one of those without saying so.
    pub fn checked(value: Value) -> Result<Self, Error> {
        let named = value
            .get(TYPE_FIELD)
            .and_then(Value::as_str)
            .is_some_and(|name| !name.is_empty());
        if !named {
            return Err(Error::Untyped {
                frame: value.to_string(),
            });
        }

        Ok(Self(value))
    }

    #[must_use]
    pub fn value(&self) -> &Value {
        &self.0
    }

    /// # Returns
    ///
    /// The frame as it goes over the pipe: the object on one line, and the newline that ends it.
    /// A newline inside the frame's own text is escaped by the encoding rather than written out,
    /// so the line this returns holds exactly one.
    #[must_use]
    pub fn line(&self) -> String {
        format!("{}\n", self.0)
    }
}
