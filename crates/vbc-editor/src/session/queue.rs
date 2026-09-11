//! What the session is waiting on, held so that answering one of them answers only that one.
//!
//! A queue and not a prompt. The protocol correlates an answer to its question by `request_id` and
//! promises nothing about the order the two arrive in, so what is outstanding is a set of
//! questions each of which can be answered whenever the reader gets to it. A client that held the
//! newest one and called it "the" pending request would answer the wrong question the first time
//! two were open, and would have no way at all to answer the older one afterwards.
//!
//! Measured against claude 2.1.263, two parallel tool calls in one message do not put two
//! questions here: the session asks about the first, waits, and asks about the second only once
//! the first is answered -- even for two calls it then runs together. That is a fact about a
//! release rather than about the protocol, and it is the reason this is the module with the
//! cheapest tests and the fewest assumptions: the shape that survives a release which stops
//! serialising them is the one that never depended on it.
//!
//! Nothing here answers anything. The queue is read by whatever draws it and drained by whatever
//! sends, because a session with a question outstanding writes nothing else at all until it is
//! answered -- so the answer has to come from a reader who is being shown the question, and a
//! drawing that blocked on one would be a drawing that cannot show the next.

use super::control::{Answer, Ask};
use super::event::Event;

/// The questions a session is waiting on answers to, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queue {
    outstanding: Vec<Ask>,
}

impl Queue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes a question on where the event is one.
    ///
    /// # Returns
    ///
    /// Whether the event was a question the session is waiting on.
    pub fn read(&mut self, event: &Event) -> bool {
        let Some(ask) = Ask::read(event) else {
            return false;
        };
        self.outstanding.push(ask);

        true
    }

    /// Takes on every question a run of events holds.
    ///
    /// # Returns
    ///
    /// How many of them there were.
    pub fn read_all(&mut self, events: &[Event]) -> usize {
        events.iter().filter(|event| self.read(event)).count()
    }

    #[must_use]
    pub fn outstanding(&self) -> &[Ask] {
        &self.outstanding
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.outstanding.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.outstanding.is_empty()
    }

    /// # Returns
    ///
    /// The question that has been waiting longest, or `None` where the session is waiting on
    /// nothing.
    #[must_use]
    pub fn oldest(&self) -> Option<&Ask> {
        self.outstanding.first()
    }

    /// # Returns
    ///
    /// The question `request_id` names, or `None` where nothing outstanding is named by it.
    #[must_use]
    pub fn get(&self, request_id: &str) -> Option<&Ask> {
        self.outstanding
            .iter()
            .find(|ask| request_id == ask.request_id())
    }

    /// Strikes off the question an answer answers.
    ///
    /// # Returns
    ///
    /// Whether the answer named a question that was outstanding. An answer that named none is one
    /// the session was not waiting on, which is worth knowing about because it is not worth
    /// sending.
    pub fn answered(&mut self, answer: &Answer) -> bool {
        self.take(answer.request_id()).is_some()
    }

    /// # Returns
    ///
    /// The question `request_id` names, taken off the queue, or `None` where nothing outstanding
    /// is named by it.
    pub fn take(&mut self, request_id: &str) -> Option<Ask> {
        let at = self
            .outstanding
            .iter()
            .position(|ask| request_id == ask.request_id())?;

        Some(self.outstanding.remove(at))
    }
}
