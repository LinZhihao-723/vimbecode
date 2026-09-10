//! The round trips: what a session asks of the reader, and the one thing the reader asks of it.
//!
//! A session that has nobody to ask does not fail. It stops. The first tool call needing approval
//! writes a `control_request` and then says nothing else at all -- no assistant frame, no result,
//! no error -- until that request is answered, and a client that read the request as one more
//! event to draw would sit in front of a stream that had gone quiet for a reason it never
//! mentioned. So an answer is not a courtesy here. It is the only thing that starts the session
//! again.
//!
//! Both directions are the same envelope, correlated by `request_id`. That correlation is the
//! whole of the protocol's ordering guarantee: an answer names the request it answers, and nothing
//! anywhere says the answers must come back in the order the requests went out. What is
//! outstanding is therefore a set rather than a stack, which is [`super::queue`]'s to hold, and
//! what an answer is built from is the request itself rather than whichever request arrived last.
//!
//! Three things a session can ask arrive through the one gate, and they are not the same question.
//!
//! A tool call is the plain case: a name, the input it would run with, and an approval that lets
//! it. Approving carries the input back, because the answer is what the tool is actually run with
//! and a reader who changed it is the reason that field exists.
//!
//! A question is not. `AskUserQuestion` comes through the same gate and looks exactly like an
//! approval, and answering it the way an approval is answered resolves it as *"the user did not
//! answer the questions"* -- measured, against claude 2.1.263, which then asked again. What the
//! model reads is [`ANSWERS_FIELD`] inside the input the answer carries back, keyed by the
//! question's own text. An `allow` is permission for the tool to run; the answer is a value, and
//! the two travel in the same frame without being the same thing.
//!
//! A plan is the third: `ExitPlanMode` carries the plan document, approving it flips the session's
//! permission mode, and the flip is announced by a `system` frame rather than by the answer. Both
//! it and a question are flagged [`INTERACTIVE_FIELD`], which is the session saying that a person
//! and not a policy is meant to decide.
//!
//! The one request going the other way is the interrupt, and it takes an argument that is not
//! optional. Asked to interrupt with `cancel_queued` false, claude 2.1.263 aborts the turn, writes
//! a receipt naming what is still queued -- and then, some twenty milliseconds later, starts a new
//! turn on the queue it kept: the reader pressed Stop and the session carried on. Asked with it
//! true, the receipt additionally names what it threw away and nothing follows.
//! [`Request::interrupt`] therefore has no parameter to get wrong.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::error::Error;
use super::event::{Event, CONTROL_REQUEST_TYPE, CONTROL_RESPONSE_TYPE};
use super::frame::Frame;
use super::identity::SessionId;

/// The subtype a session asks a tool question under, which is every question it asks.
pub const CAN_USE_TOOL_SUBTYPE: &str = "can_use_tool";

/// The subtype the reader's own request goes out under.
pub const INTERRUPT_SUBTYPE: &str = "interrupt";

/// The subtype an answered round trip carries, in both directions.
pub const SUCCESS_SUBTYPE: &str = "success";

/// The tools whose approval is not the answer they are asking for. Approving either runs it; what
/// [`QUESTION_TOOL`] does with the run is read out of the input the approval carries back, and
/// what [`PLAN_TOOL`] does with it is change the session's permission mode.
pub const QUESTION_TOOL: &str = "AskUserQuestion";
pub const PLAN_TOOL: &str = "ExitPlanMode";

/// The field a question's answers go in, inside the input an approval carries back, keyed by the
/// text of the question each answers. An approval carrying no such field is read by the session as
/// the reader having declined to answer.
pub const ANSWERS_FIELD: &str = "answers";

/// The field a session flags the questions a person is meant to decide with.
pub const INTERACTIVE_FIELD: &str = "requires_user_interaction";

/// What an answer does with the call it answers.
pub const ALLOW_BEHAVIOUR: &str = "allow";
pub const DENY_BEHAVIOUR: &str = "deny";

/// The field an interrupt says whether the queue goes with the turn in, which is the difference
/// between a session that stops and one that carries on from where the reader could not see it.
pub const CANCEL_QUEUED_FIELD: &str = "cancel_queued";

/// The fields of the envelope both directions share.
const REQUEST_ID_FIELD: &str = "request_id";
const REQUEST_FIELD: &str = "request";
const RESPONSE_FIELD: &str = "response";
const SUBTYPE_FIELD: &str = "subtype";
const ERROR_FIELD: &str = "error";

/// The fields a session's question carries.
const TOOL_NAME_FIELD: &str = "tool_name";
const DESCRIPTION_FIELD: &str = "description";
const TOOL_USE_ID_FIELD: &str = "tool_use_id";
const INPUT_FIELD: &str = "input";
const SUGGESTIONS_FIELD: &str = "permission_suggestions";

/// The fields an answer carries.
const BEHAVIOUR_FIELD: &str = "behavior";
const UPDATED_INPUT_FIELD: &str = "updatedInput";
const MESSAGE_FIELD: &str = "message";

/// The fields a question is read from, and the fields one of its options is.
const QUESTIONS_FIELD: &str = "questions";
const QUESTION_FIELD: &str = "question";
const HEADER_FIELD: &str = "header";
const OPTIONS_FIELD: &str = "options";
const MULTIPLE_FIELD: &str = "multiSelect";
const LABEL_FIELD: &str = "label";

/// The field a plan is carried in.
const PLAN_FIELD: &str = "plan";

/// The fields an interrupt's receipt carries. Only the first of them is written where the
/// interrupt was not asked to empty the queue, which is the receipt's own account of having kept
/// it.
const STILL_QUEUED_FIELD: &str = "still_queued";
const CANCELLED_FIELD: &str = "cancelled";

/// One question a session is waiting on an answer to before it does anything else.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    request_id: String,
    tool: String,
    description: String,
    tool_use_id: String,
    input: Value,
    suggestions: Vec<Value>,
    interactive: bool,
}

impl Ask {
    /// # Returns
    ///
    /// The question a frame is the session asking, or `None` where the frame is anything else --
    /// including a control frame going the other way, which is this client's own request coming
    /// back to it.
    #[must_use]
    pub fn read(event: &Event) -> Option<Self> {
        let raw = event.raw();
        if CONTROL_REQUEST_TYPE != raw.get("type").and_then(Value::as_str)? {
            return None;
        }

        let request = raw.get(REQUEST_FIELD)?;
        if CAN_USE_TOOL_SUBTYPE != request.get(SUBTYPE_FIELD).and_then(Value::as_str)? {
            return None;
        }

        Some(Self {
            request_id: raw
                .get(REQUEST_ID_FIELD)
                .and_then(Value::as_str)?
                .to_owned(),
            tool: text(request, TOOL_NAME_FIELD),
            description: text(request, DESCRIPTION_FIELD),
            tool_use_id: text(request, TOOL_USE_ID_FIELD),
            input: request.get(INPUT_FIELD).cloned().unwrap_or(Value::Null),
            suggestions: request
                .get(SUGGESTIONS_FIELD)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            interactive: request
                .get(INTERACTIVE_FIELD)
                .and_then(Value::as_bool)
                .unwrap_or_default(),
        })
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub fn tool_use_id(&self) -> &str {
        &self.tool_use_id
    }

    #[must_use]
    pub fn input(&self) -> &Value {
        &self.input
    }

    #[must_use]
    pub fn suggestions(&self) -> &[Value] {
        &self.suggestions
    }

    /// # Returns
    ///
    /// Whether the session says a person rather than a policy is meant to decide this one.
    #[must_use]
    pub fn interactive(&self) -> bool {
        self.interactive
    }

    /// # Returns
    ///
    /// What the reader is being asked, which is a tool call for every tool but the two that ask
    /// for something else through the same gate.
    #[must_use]
    pub fn subject(&self) -> Subject {
        match self.tool.as_str() {
            QUESTION_TOOL => Subject::Questions(Question::read_all(&self.input)),
            PLAN_TOOL => Subject::Plan(text(&self.input, PLAN_FIELD)),
            _ => Subject::Tool,
        }
    }

    /// # Returns
    ///
    /// The answer to this question, ready to be sent. It is built from the question rather than
    /// named after it so that the identifier an answer is correlated by, and the input an approval
    /// carries back, cannot be taken from a different one.
    #[must_use]
    pub fn answer(&self, decision: &Decision) -> Answer {
        Answer {
            request_id: self.request_id.clone(),
            response: decision.response(&self.input),
        }
    }
}

/// What a session is asking for, which is not always what the gate it arrives through says.
#[derive(Clone, Debug, PartialEq)]
pub enum Subject {
    /// Approval for a tool call, which is the plain case and the common one.
    Tool,

    /// An answer to the questions it carries, which no approval on its own gives.
    Questions(Vec<Question>),

    /// Approval of the plan it carries, which also changes the session's permission mode.
    Plan(String),
}

/// One question a session put to the reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Question {
    /// The question itself, which is also the key its answer is written under.
    pub question: String,

    /// The few words the session heads it with.
    pub header: String,

    /// The answers it offers. A reader is not held to them: what goes back is text.
    pub options: Vec<Choice>,

    /// Whether more than one of the options may be chosen.
    pub multiple: bool,
}

impl Question {
    /// # Returns
    ///
    /// Every question a call to [`QUESTION_TOOL`] carries, which is empty where it carries none.
    fn read_all(input: &Value) -> Vec<Self> {
        input
            .get(QUESTIONS_FIELD)
            .and_then(Value::as_array)
            .map(|asked| asked.iter().map(Self::read).collect())
            .unwrap_or_default()
    }

    /// # Returns
    ///
    /// One question, with a field it does not carry read as empty.
    fn read(raw: &Value) -> Self {
        Self {
            question: text(raw, QUESTION_FIELD),
            header: text(raw, HEADER_FIELD),
            options: raw
                .get(OPTIONS_FIELD)
                .and_then(Value::as_array)
                .map(|offered| offered.iter().map(Choice::read).collect())
                .unwrap_or_default(),
            multiple: raw
                .get(MULTIPLE_FIELD)
                .and_then(Value::as_bool)
                .unwrap_or_default(),
        }
    }
}

/// One answer a session offered to a question of its own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Choice {
    /// The answer, as it would be written back.
    pub label: String,

    /// What the session says the answer means.
    pub description: String,
}

impl Choice {
    /// # Returns
    ///
    /// One offered answer, with a field it does not carry read as empty.
    fn read(raw: &Value) -> Self {
        Self {
            label: text(raw, LABEL_FIELD),
            description: text(raw, DESCRIPTION_FIELD),
        }
    }
}

/// What the reader decided about a question the session asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// The call runs, with the input it was asked with.
    Allowed,

    /// The call runs, with an input the reader changed. This is the whole input rather than a
    /// change to it, because it is what the tool is run with.
    Changed(Value),

    /// The questions are answered, each under its own text. This is the only shape an answer
    /// reaches the model in: an approval carrying none of them is read as the reader declining to
    /// answer, whatever else it carries.
    Answered(BTreeMap<String, String>),

    /// The call does not run, and the model is told this much about why.
    Denied(String),
}

impl Decision {
    /// # Returns
    ///
    /// A newly created decision answering one question with one answer, which is the shape almost
    /// every question a session asks has.
    #[must_use]
    pub fn answering(question: &str, answer: &str) -> Self {
        Self::Answered(BTreeMap::from([(question.to_owned(), answer.to_owned())]))
    }

    /// # Returns
    ///
    /// What the decision is on the wire, given the input the question was asked with.
    fn response(&self, asked: &Value) -> Value {
        match self {
            Self::Allowed => allowing(asked.clone()),
            Self::Changed(input) => allowing(input.clone()),
            Self::Answered(answers) => {
                let mut input = match asked {
                    Value::Object(fields) => fields.clone(),
                    _ => Map::new(),
                };
                input.insert(ANSWERS_FIELD.to_owned(), json!(answers));

                allowing(Value::Object(input))
            }
            Self::Denied(message) => json!({
                BEHAVIOUR_FIELD: DENY_BEHAVIOUR,
                MESSAGE_FIELD: message,
            }),
        }
    }
}

/// One answer, bound to the question it answers.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    request_id: String,
    response: Value,
}

impl Answer {
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// # Returns
    ///
    /// The answer as it goes back to the session.
    ///
    /// # Panics
    ///
    /// Panics if the frame it builds does not name its type, which would mean this function and
    /// [`Frame::checked`] disagree about what a frame is.
    #[must_use]
    pub fn frame(&self) -> Frame {
        Frame::checked(json!({
            "type": CONTROL_RESPONSE_TYPE,
            RESPONSE_FIELD: {
                SUBTYPE_FIELD: SUCCESS_SUBTYPE,
                REQUEST_ID_FIELD: self.request_id,
                RESPONSE_FIELD: self.response,
            },
        }))
        .expect("a control response names its type")
    }
}

/// One request going the other way: something the reader asks of the session.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    request_id: String,
    request: Value,
}

impl Request {
    /// # Returns
    ///
    /// A newly created request that aborts the turn in flight and empties the queue behind it.
    ///
    /// Emptying it is not a choice this offers. A session interrupted without it aborts the turn,
    /// answers, and then starts the next queued turn a moment later, which is a session carrying
    /// on after the reader stopped it.
    #[must_use]
    pub fn interrupt() -> Self {
        Self {
            request_id: SessionId::generated().to_string(),
            request: json!({
                SUBTYPE_FIELD: INTERRUPT_SUBTYPE,
                CANCEL_QUEUED_FIELD: true,
            }),
        }
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// # Returns
    ///
    /// The request as it goes out to the session.
    ///
    /// # Panics
    ///
    /// Panics if the frame it builds does not name its type, which would mean this function and
    /// [`Frame::checked`] disagree about what a frame is.
    #[must_use]
    pub fn frame(&self) -> Frame {
        Frame::checked(json!({
            "type": CONTROL_REQUEST_TYPE,
            REQUEST_ID_FIELD: self.request_id,
            REQUEST_FIELD: self.request,
        }))
        .expect("a control request names its type")
    }
}

/// What a session says it did with an interrupt.
///
/// The receipt is written before the aborted turn's own result frame, so it is the first account
/// there is of a turn having been stopped rather than having ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    /// What the session still holds, which is what a reader would otherwise watch run after
    /// stopping it.
    pub still_queued: Vec<Value>,

    /// What the interrupt threw away. A session that was not asked to empty its queue does not
    /// write this field at all, so its absence is the session saying it kept what it had.
    pub cancelled: Option<Vec<Value>>,
}

impl Receipt {
    /// # Returns
    ///
    /// The receipt for `request_id` where the frame is the session's answer to it, and `None`
    /// where it is anything else: the answer arrives on the one stream everything else does, and
    /// it is not necessarily the next thing on it.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Refused`] if the session answered the request with an error instead of a
    ///   receipt.
    #[must_use]
    pub fn read(event: &Event, request_id: &str) -> Option<Result<Self, Error>> {
        let raw = event.raw();
        if CONTROL_RESPONSE_TYPE != raw.get("type").and_then(Value::as_str)? {
            return None;
        }

        let response = raw.get(RESPONSE_FIELD)?;
        if request_id != response.get(REQUEST_ID_FIELD).and_then(Value::as_str)? {
            return None;
        }

        if SUCCESS_SUBTYPE != response.get(SUBTYPE_FIELD).and_then(Value::as_str)? {
            return Some(Err(Error::Refused {
                request: request_id.to_owned(),
                reason: text(response, ERROR_FIELD),
            }));
        }

        let receipt = response.get(RESPONSE_FIELD).unwrap_or(&Value::Null);

        Some(Ok(Self {
            still_queued: entries(receipt, STILL_QUEUED_FIELD).unwrap_or_default(),
            cancelled: entries(receipt, CANCELLED_FIELD),
        }))
    }
}

/// # Returns
///
/// An answer that lets a call run with `input`.
fn allowing(input: Value) -> Value {
    json!({
        BEHAVIOUR_FIELD: ALLOW_BEHAVIOUR,
        UPDATED_INPUT_FIELD: input,
    })
}

/// # Returns
///
/// A string field of a frame, which is empty where the frame does not carry it.
fn text(raw: &Value, field: &str) -> String {
    raw.get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// # Returns
///
/// An array field of a frame, or `None` where the frame does not carry one. The entries are kept
/// as they arrived: what a session puts in these is undocumented and was empty every time it was
/// measured, and a reader that named their shape would be inventing it.
fn entries(raw: &Value, field: &str) -> Option<Vec<Value>> {
    Some(raw.get(field)?.as_array()?.clone())
}
