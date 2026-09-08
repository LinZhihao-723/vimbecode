//! What comes back out of a session: one NDJSON line in, one typed event out.
//!
//! An event keeps the frame it was decoded from. The protocol carries far more than this milestone
//! reads -- token deltas, subagent nesting, tool results with terminal escapes still in them -- and
//! a client that dropped what it did not yet understand would have to be rewritten to understand
//! it. So the typing here is a surface rather than a filter: the fields the session's own identity
//! and a turn's ending are decided by are lifted out, and everything else waits in
//! [`Event::raw`] for the milestone that reads it.
//!
//! Two frames are worth naming even so. The init frame is the session announcing what it is, and
//! it is where the identifier a forked session ended up running under is read from, and where the
//! permission flag is checked for. The result frame is a turn ending, which is the only thing that
//! says a turn is over: assistant frames stop arriving because there are no more, not because a
//! last one is marked.

use serde_json::Value;

use super::error::Error;

/// The frame naming a session announcing itself.
pub const INIT_TYPE: &str = "system";
pub const INIT_SUBTYPE: &str = "init";

/// The frame naming the end of a turn.
pub const RESULT_TYPE: &str = "result";

/// The frames naming what the assistant said and what was said back to it.
pub const ASSISTANT_TYPE: &str = "assistant";
pub const USER_TYPE: &str = "user";

/// The frames the control protocol is carried in, both directions.
pub const CONTROL_REQUEST_TYPE: &str = "control_request";
pub const CONTROL_RESPONSE_TYPE: &str = "control_response";

/// The longest prefix of an unreadable line an error carries. A frame can hold a megabyte of tool
/// output, and none of it says anything more about why the line would not decode than its opening
/// does.
const REPORTED_LINE: usize = 200;

/// One frame the session wrote, as it arrived and as far as it has been read.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    kind: Kind,
    raw: Value,
}

impl Event {
    /// Decodes one NDJSON line the session wrote.
    ///
    /// # Returns
    ///
    /// The event the line carries on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Undecodable`] if the line is not a JSON object, or names no type.
    pub fn decoded(line: &str) -> Result<Self, Error> {
        let raw: Value = serde_json::from_str(line).map_err(|error| Error::Undecodable {
            line: shortened(line),
            reason: error.to_string(),
        })?;
        let Some(name) = raw.get("type").and_then(Value::as_str) else {
            return Err(Error::Undecodable {
                line: shortened(line),
                reason: "it names no type".to_owned(),
            });
        };

        let subtype = raw
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let kind = match (name, subtype) {
            (INIT_TYPE, INIT_SUBTYPE) => Kind::Init(Init::read(&raw)),
            (INIT_TYPE, subtype) => Kind::System(subtype.to_owned()),
            (RESULT_TYPE, _) => Kind::Turn(Turn::read(&raw)),
            (ASSISTANT_TYPE, _) => Kind::Assistant,
            (USER_TYPE, _) => Kind::User,
            (CONTROL_REQUEST_TYPE | CONTROL_RESPONSE_TYPE, _) => Kind::Control,
            (name, _) => Kind::Other(name.to_owned()),
        };

        Ok(Self { kind, raw })
    }

    #[must_use]
    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    #[must_use]
    pub fn raw(&self) -> &Value {
        &self.raw
    }

    /// # Returns
    ///
    /// The session the frame belongs to, which every frame but a control response carries.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.raw.get("session_id").and_then(Value::as_str)
    }

    /// # Returns
    ///
    /// What the session announced about itself, where the frame is an init frame.
    #[must_use]
    pub fn init(&self) -> Option<&Init> {
        match &self.kind {
            Kind::Init(init) => Some(init),
            _ => None,
        }
    }

    /// # Returns
    ///
    /// How a turn ended, where the frame is a result frame.
    #[must_use]
    pub fn turn(&self) -> Option<&Turn> {
        match &self.kind {
            Kind::Turn(turn) => Some(turn),
            _ => None,
        }
    }
}

/// Which frame an event is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Kind {
    /// The session announcing itself, which it does once at the head of every turn.
    Init(Init),

    /// What the assistant said.
    Assistant,

    /// What was said to it, tool results included.
    User,

    /// A turn ending.
    Turn(Turn),

    /// A round trip of the control protocol, either direction.
    Control,

    /// A system frame that is not an init frame, named by its subtype.
    System(String),

    /// A frame this milestone does not read, named by its type.
    Other(String),
}

/// What a session announces about itself at the head of a turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Init {
    /// The identifier the session is actually running under, which a forked session's spawn was
    /// not given.
    pub session_id: String,

    /// The version of the `claude` binary answering.
    pub version: String,

    /// The model the turn runs on.
    pub model: String,

    /// The directory the session works in.
    pub directory: String,

    /// The permission mode, as it appears on the wire. `manual` appears here as `default`.
    pub permission_mode: String,

    /// Every tool the session offers, which is where the permission flag is read off.
    pub tools: Vec<String>,

    /// The protocol extensions the binary speaks.
    pub capabilities: Vec<String>,

    /// The slash commands the session answers.
    pub slash_commands: Vec<String>,
}

impl Init {
    /// # Returns
    ///
    /// What an init frame announces, with a field the frame does not carry read as empty: a
    /// missing field is a release that stopped sending it, and every one of these is answered
    /// better by what depends on it than by a decode that fails here.
    fn read(raw: &Value) -> Self {
        Self {
            session_id: text(raw, "session_id"),
            version: text(raw, "claude_code_version"),
            model: text(raw, "model"),
            directory: text(raw, "cwd"),
            permission_mode: text(raw, "permissionMode"),
            tools: texts(raw, "tools"),
            capabilities: texts(raw, "capabilities"),
            slash_commands: texts(raw, "slash_commands"),
        }
    }
}

/// How a turn ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Turn {
    /// Whether it ended by finishing, which is `success`, or some other way.
    pub subtype: String,

    /// Whether the session is reporting the turn as an error.
    pub failed: bool,

    /// How many turns the session has taken.
    pub turns: u64,

    /// What the assistant last said, where the turn ended with the assistant speaking.
    pub text: String,

    /// The tool calls that were denied over the turn.
    pub denials: usize,
}

impl Turn {
    /// # Returns
    ///
    /// How a result frame says its turn ended.
    fn read(raw: &Value) -> Self {
        Self {
            subtype: text(raw, "subtype"),
            failed: raw
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
            turns: raw
                .get("num_turns")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            text: text(raw, "result"),
            denials: raw
                .get("permission_denials")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
        }
    }
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
/// A field of a frame holding an array of strings, which is empty where the frame does not carry
/// it. An entry that is not a string is left out rather than reported: the arrays this reads are
/// catalogs, and a catalog with one unreadable entry still says what the rest of it says.
fn texts(raw: &Value, field: &str) -> Vec<String> {
    raw.get(field)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// # Returns
///
/// As much of a line as an error about it is worth carrying.
fn shortened(line: &str) -> String {
    let kept: String = line.chars().take(REPORTED_LINE).collect();
    if kept.len() < line.len() {
        return format!("{kept}...");
    }

    kept
}
