//! What a session said, read as the blocks the chat panel already draws.
//!
//! The panel does not learn a second model here. It consumes [`crate::chat::block::Block`] -- a
//! message, a fenced code block, a call to a tool, what the tool answered, a thinking block, a
//! diff -- and every one of those is something the stream already carries, so this module is a
//! translation and not a model of its own. A transcript built here is a transcript the folds, the
//! text objects and the yanks read without being told where it came from.
//!
//! Four of the translations are the reason the module exists rather than conveniences on top of
//! it.
//!
//! A fenced code block arrives inside the prose it was written in, as the three backticks a model
//! types. `iac` is the gesture this editor exists for, and it resolves against a block that *is*
//! code rather than against prose that happens to hold some, so the fences are read here and what
//! is between them becomes a block of its own holding exactly the bytes that were sent.
//!
//! Tool output arrives with terminal escapes still in it -- `cargo`, `ls` and `git` all colour
//! what they write, and the child forwards it as it was written. An escape is a rendition a
//! terminal was asked to select rather than text anybody said, so it is read as the style it names
//! and the block's source holds the text without it. Anything else puts escape bytes into what a
//! reader yanks.
//!
//! An edit arrives as `old_string` and `new_string`, which is the edit itself rather than an
//! account of one. The diff is computed from those two, so what a reader sees is the lines that
//! changed and what `yad` writes out is the patch that was applied. Reading a diff out of the
//! prose around the call would be reading a model's description of its own edit.
//!
//! And a subagent's work arrives tagged with `parent_tool_use_id`, at every depth. That tag is
//! what the panel's nested folds are keyed on, so the nesting is carried across rather than
//! rebuilt from the order the frames arrived in: the call that started a subagent folds away
//! everything the subagent did, and a call the subagent itself made folds away inside that. A tool
//! result is tagged with the call it answers for the same reason, which is what pairs the two.
//!
//! Two things the stream carries are deliberately not blocks. A local command's response is marked
//! `is_meta` and comes from the model named `<synthetic>`: it is the client answering rather than a
//! turn Claude took, so `/clear` and `/compact` leave a break in the history instead of an
//! assistant message nobody said. A break is a place in the sequence rather than a block, because
//! a block is something that was said and a cleared history is the absence of them; it is read off
//! the `conversation_reset` frame and off a compaction that reported success, neither of which is
//! prose. And the deltas of `stream_event` are passed over, because a completed `assistant` frame
//! follows every stream of them and a transcript that appended both would hold everything twice.
//!
//! What the user themselves asked never comes back at all -- the child echoes no prompt, and a
//! resumed session replays none of the history it was resumed into -- so a question is in the
//! transcript because the side that sent it put it there, which is what [`Conversation::asked`] is
//! for. That is also why a user frame the conversation itself is the parent of contributes no
//! prose: what such a frame carries is the client writing to its own history rather than a reader
//! typing. A compaction writes two of them, neither marked `is_meta` and neither anything anybody
//! said -- the summary the history was replaced by, and the stdout of the hook the compaction ran
//! -- and a transcript that read them would answer `/compact` with a thousand words of summary
//! attributed to the reader. The prose a user frame does carry is what a subagent was told to do,
//! and that arrives beneath the call that started the subagent.

use serde_json::Value;

use crate::chat::block::{Block, Kind as BlockKind, Role};
use crate::chat::fold::Tag;
use crate::chat::transcript::Transcript;

use super::event::{Event, Kind, Turn};

/// The frame naming a history thrown away, which is what `/clear` leaves behind.
pub const RESET_TYPE: &str = "conversation_reset";

/// The subtype of the system frame a compaction reports through, the field it reports in, and the
/// value saying the history really was replaced by a summary of itself.
pub const STATUS_SUBTYPE: &str = "status";
pub const COMPACT_RESULT: &str = "compact_result";
pub const COMPACTED: &str = "success";

/// The field marking a frame as a local command's own response, and the model such a response
/// names itself as coming from. Either of them says the frame is the client answering rather than
/// Claude.
pub const META_FIELD: &str = "is_meta";
pub const SYNTHETIC_MODEL: &str = "<synthetic>";

/// The tool whose call carries the text it replaced and the text it wrote, and the tool that
/// carries several such pairs at once.
pub const EDIT_TOOL: &str = "Edit";
pub const MULTI_EDIT_TOOL: &str = "MultiEdit";

/// The fields of a frame this reads: the message a turn is carried in, the content of that
/// message, the model that wrote it, and the call the frame arrived beneath.
const MESSAGE_FIELD: &str = "message";
const CONTENT_FIELD: &str = "content";
const MODEL_FIELD: &str = "model";
const PARENT_FIELD: &str = "parent_tool_use_id";

/// The field naming what a content item is, and the items this reads.
const ITEM_FIELD: &str = "type";
const TEXT_ITEM: &str = "text";
const THINKING_ITEM: &str = "thinking";
const TOOL_USE_ITEM: &str = "tool_use";
const TOOL_RESULT_ITEM: &str = "tool_result";

/// The fields a content item carries: the id a call is answered under, the tool it calls and what
/// it calls the tool with, and the id a result answers.
const ID_FIELD: &str = "id";
const NAME_FIELD: &str = "name";
const INPUT_FIELD: &str = "input";
const TOOL_USE_ID_FIELD: &str = "tool_use_id";

/// The fields an edit is read from, and the field a multiple edit holds its own pairs in.
const PATH_FIELD: &str = "file_path";
const OLD_FIELD: &str = "old_string";
const NEW_FIELD: &str = "new_string";
const EDITS_FIELD: &str = "edits";

/// The argument that says what a call to a tool does, tool by tool. A call is worth reading as the
/// one thing it asked for rather than as the whole of its input, and for every tool below that is
/// a single argument; a tool this does not name is written out as the input it was given, which
/// says less but loses nothing.
const ARGUMENTS: [(&str, &str); 11] = [
    ("Agent", "prompt"),
    ("Bash", "command"),
    ("Glob", "pattern"),
    ("Grep", "pattern"),
    ("NotebookEdit", "notebook_path"),
    ("Read", PATH_FIELD),
    ("Skill", "skill"),
    ("Task", "prompt"),
    ("WebFetch", "url"),
    ("WebSearch", "query"),
    ("Write", PATH_FIELD),
];

/// The characters a fence may be written with, the shortest run of one that opens or closes a
/// fenced region, and the deepest such a run may be indented and still be read as a fence. These
/// are the rules [`crate::chat::object`] reads a fence by, so a region this makes a block of is a
/// region `iac` would have found inside one. What the two do with an indented fence differs by
/// design: `iac` names a range of the prose it is resolved in and hands back the bytes in it, and
/// a block is the code itself and holds it as a markdown reader would draw it, which is without
/// the indentation the list around it was written under.
const FENCES: [char; 2] = ['`', '~'];
const FENCE: usize = 3;
const INDENT: usize = 3;

/// The character a fence may not name a language with, because a run of it is what opened the
/// fence.
const BACKTICK: char = '`';

/// The byte a logical line ends on, and the one that may precede it.
const SEPARATOR: char = '\n';
const CARRIAGE: char = '\r';

/// Why a conversation's history stops where it does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    /// `/clear`: the history was thrown away and the session carried on under a new conversation.
    Cleared,

    /// `/compact`: the history was replaced by a summary of itself.
    Compacted,
}

/// A place in the sequence of blocks where the session stopped carrying what came before it.
///
/// A break is a position rather than a block, because a block is something that was said and a
/// cleared history is the absence of one. It names how many blocks had been said when it happened,
/// so a break at the end of a transcript is as expressible as one in the middle of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Break {
    after: usize,
    reason: Reason,
}

impl Break {
    /// # Returns
    ///
    /// How many blocks the transcript held when the break happened, which is the index of the
    /// first block said after it.
    #[must_use]
    pub fn after(&self) -> usize {
        self.after
    }

    #[must_use]
    pub fn reason(&self) -> Reason {
        self.reason
    }
}

/// What a session has said so far: the blocks of it, how they nest, where its history was broken,
/// and what it announced about itself.
///
/// A conversation is fed one frame at a time and only ever grows, which is what a transcript does
/// too. Nothing here draws anything: what comes out of it is the transcript and the tags a panel
/// is built over.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conversation {
    transcript: Transcript,
    tags: Vec<Tag>,
    breaks: Vec<Break>,
    session: Option<String>,
    model: Option<String>,
    commands: Vec<String>,
    ended: Option<Turn>,
}

impl Conversation {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created conversation nothing has been said in.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts a question the user asked into the transcript.
    ///
    /// The child echoes no prompt, so a question is in the transcript because the side that sent
    /// it put it there or it is not in the transcript at all.
    pub fn asked(&mut self, text: &str) {
        self.push(
            Block::new(BlockKind::Message(Role::User), text.to_owned()),
            Tag::untagged(),
        );
    }

    /// Reads one frame the session wrote into whatever it says.
    pub fn read(&mut self, event: &Event) {
        match event.kind() {
            Kind::Init(init) => {
                self.session = Some(init.session_id.clone());
                self.model = Some(init.model.clone());
                self.commands.clone_from(&init.slash_commands);
            }
            Kind::Assistant => self.said(event.raw()),
            Kind::User => self.answered(event.raw()),
            Kind::Turn(turn) => self.ended = Some(turn.clone()),
            Kind::System(subtype) if STATUS_SUBTYPE == subtype.as_str() => {
                if compacted(event.raw()) {
                    self.broke(Reason::Compacted);
                }
            }
            Kind::Other(name) if RESET_TYPE == name.as_str() => self.broke(Reason::Cleared),
            Kind::Control | Kind::System(_) | Kind::Other(_) => {}
        }
    }

    /// Reads every frame of `events`, in the order they arrived.
    pub fn read_all(&mut self, events: &[Event]) {
        for event in events {
            self.read(event);
        }
    }

    /// Records that the history stopped where the transcript now ends, for the reason `reason`.
    pub fn broke(&mut self, reason: Reason) {
        self.breaks.push(Break {
            after: self.transcript.len(),
            reason,
        });
    }

    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// # Returns
    ///
    /// The tag every block arrived with, in the order the blocks were said, which is what the
    /// panel's folds nest by.
    #[must_use]
    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }

    /// # Returns
    ///
    /// Every place the history was broken, in the order the breaks happened.
    #[must_use]
    pub fn breaks(&self) -> &[Break] {
        &self.breaks
    }

    /// # Returns
    ///
    /// The identifier the session last announced itself under, or `None` where it has not
    /// announced itself yet. A cleared or forked session announces a new one.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.session.as_deref()
    }

    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// # Returns
    ///
    /// The slash commands the session answers, which is the catalog a client's own help is written
    /// from.
    #[must_use]
    pub fn commands(&self) -> &[String] {
        &self.commands
    }

    /// # Returns
    ///
    /// How the last turn ended, with what it cost and what it used, or `None` where no turn has
    /// ended yet.
    #[must_use]
    pub fn ended(&self) -> Option<&Turn> {
        self.ended.as_ref()
    }

    /// # Returns
    ///
    /// The transcript and the tags of it, which is what a panel is built over.
    #[must_use]
    pub fn into_panel(self) -> (Transcript, Vec<Tag>) {
        (self.transcript, self.tags)
    }

    /// Reads an assistant frame into the blocks it holds: the prose and whatever is fenced inside
    /// it, what Claude was thinking, and the calls it made.
    fn said(&mut self, raw: &Value) {
        if meta(raw) {
            return;
        }

        let beneath = parent(raw);
        if let Some(plain) = plain(raw) {
            self.prose(plain, beneath.as_deref());
            return;
        }

        for item in items(raw) {
            match named(item, ITEM_FIELD) {
                TEXT_ITEM => self.prose(named(item, TEXT_ITEM), beneath.as_deref()),
                THINKING_ITEM => {
                    let thought = named(item, THINKING_ITEM);
                    if !thought.trim().is_empty() {
                        let block = Block::new(BlockKind::Thinking, thought.to_owned());
                        self.push(block, Tag::new(None, beneath.clone()));
                    }
                }
                TOOL_USE_ITEM => self.called(item, beneath.as_deref()),
                _ => {}
            }
        }
    }

    /// Reads a user frame into the blocks it holds, which are what the tools answered and, beneath
    /// a subagent's own call, the prompt that subagent was given.
    fn answered(&mut self, raw: &Value) {
        if meta(raw) {
            return;
        }

        let beneath = parent(raw);
        if let Some(plain) = plain(raw) {
            self.told(plain, beneath.as_deref());
            return;
        }

        for item in items(raw) {
            match named(item, ITEM_FIELD) {
                TOOL_RESULT_ITEM => {
                    let answers = item
                        .get(TOOL_USE_ID_FIELD)
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let under = answers.clone().or_else(|| beneath.clone());
                    let block = Block::from_ansi(BlockKind::ToolResult, &reported(item));
                    self.push(block, Tag::new(answers, under));
                }
                TEXT_ITEM => self.told(named(item, TEXT_ITEM), beneath.as_deref()),
                _ => {}
            }
        }
    }

    /// Reads one run of assistant prose into the message and code blocks it is written as, each of
    /// them said beneath the call `beneath`.
    fn prose(&mut self, said: &str, beneath: Option<&str>) {
        for (kind, source) in written(said) {
            let block = match kind {
                BlockKind::Code { language } => Block::code(language, source),
                kind => Block::new(kind, source),
            };
            self.push(block, Tag::new(None, beneath.map(str::to_owned)));
        }
    }

    /// Appends what a subagent was told to do, beneath the call `beneath` that started it.
    ///
    /// A user frame the conversation itself is the parent of says nothing: its prose is the client
    /// writing to its own history -- a compaction's summary, a hook's stdout -- and never a
    /// question a reader asked, so `said` is passed over where `beneath` names no call.
    fn told(&mut self, said: &str, beneath: Option<&str>) {
        let Some(beneath) = beneath else {
            return;
        };

        let block = Block::new(BlockKind::Message(Role::User), said.to_owned());
        self.push(block, Tag::new(None, Some(beneath.to_owned())));
    }

    /// Reads one call to a tool into the block it is: the diff an edit already carries, or the
    /// call itself.
    fn called(&mut self, item: &Value, beneath: Option<&str>) {
        let tag = Tag::new(
            item.get(ID_FIELD)
                .and_then(Value::as_str)
                .map(str::to_owned),
            beneath.map(str::to_owned),
        );
        let name = named(item, NAME_FIELD);
        let input = item.get(INPUT_FIELD).unwrap_or(&Value::Null);

        let edits = edited(name, input);
        if edits.is_empty() {
            let block = Block::new(
                BlockKind::ToolCall {
                    name: name.to_owned(),
                },
                requested(name, input),
            );
            self.push(block, tag);
            return;
        }

        for edit in edits {
            self.push(edit, tag.clone());
        }
    }

    /// Appends `block` to the transcript, said under `tag`.
    fn push(&mut self, block: Block, tag: Tag) {
        self.transcript.push(block);
        self.tags.push(tag);
    }
}

/// A fence that has been opened: the character it was written with, how long its run is, how deep
/// the line it was written on was indented, what language it named, and the byte its body starts
/// at.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Opened {
    mark: char,
    run: usize,
    indent: usize,
    language: Option<String>,
    body: usize,
}

/// # Returns
///
/// Whether the frame is a local command's own response rather than a turn Claude took, which is
/// what `/clear`, `/compact` and every other command the client answers itself arrive as.
fn meta(raw: &Value) -> bool {
    let marked = raw
        .get(META_FIELD)
        .and_then(Value::as_bool)
        .unwrap_or_default();

    marked
        || raw
            .get(MESSAGE_FIELD)
            .is_some_and(|message| SYNTHETIC_MODEL == named(message, MODEL_FIELD))
}

/// # Returns
///
/// Whether a system status frame is reporting a compaction that replaced the history.
fn compacted(raw: &Value) -> bool {
    raw.get(COMPACT_RESULT)
        .and_then(Value::as_str)
        .is_some_and(|result| COMPACTED == result)
}

/// # Returns
///
/// The call the frame arrived beneath, which is what a subagent's every frame carries at every
/// depth, or `None` where the frame is the conversation's own.
fn parent(raw: &Value) -> Option<String> {
    raw.get(PARENT_FIELD)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// # Returns
///
/// The content of a frame's message where it is written as one string rather than as a list of
/// items, and `None` where it is not.
fn plain(raw: &Value) -> Option<&str> {
    raw.get(MESSAGE_FIELD)
        .and_then(|message| message.get(CONTENT_FIELD))
        .and_then(Value::as_str)
}

/// # Returns
///
/// The content items of a frame's message, which is empty where the frame carries none.
fn items(raw: &Value) -> &[Value] {
    raw.get(MESSAGE_FIELD)
        .and_then(|message| message.get(CONTENT_FIELD))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// # Returns
///
/// A string field of a value, which is empty where the value does not carry it or carries it as
/// something other than a string.
fn named<'value>(value: &'value Value, field: &str) -> &'value str {
    value.get(field).and_then(Value::as_str).unwrap_or_default()
}

/// # Returns
///
/// A value read as the string it is, which is empty where it is not one.
fn textual(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// # Returns
///
/// What a tool answered, as the terminal was sent it. A result carries either one string or a list
/// of items, of which the ones that are text are what the tool wrote and the rest -- a reference
/// to a tool, an image -- say nothing this can put in a block.
fn reported(item: &Value) -> String {
    let content = item.get(CONTENT_FIELD).unwrap_or(&Value::Null);
    if let Some(said) = content.as_str() {
        return said.to_owned();
    }

    let Some(parts) = content.as_array() else {
        return String::new();
    };

    let mut written = Vec::new();
    for part in parts {
        if TEXT_ITEM == named(part, ITEM_FIELD) {
            written.push(named(part, TEXT_ITEM));
        }
    }

    written.join(&SEPARATOR.to_string())
}

/// # Returns
///
/// The diff blocks a call to an editing tool already carries, which is empty where the call is to
/// a tool that does not edit or carries no pair of texts to diff.
fn edited(name: &str, input: &Value) -> Vec<Block> {
    let path = named(input, PATH_FIELD).to_owned();
    if EDIT_TOOL == name {
        let Some((old, new)) = input.get(OLD_FIELD).zip(input.get(NEW_FIELD)) else {
            return Vec::new();
        };

        return vec![Block::diff(path, textual(old), textual(new))];
    }
    if MULTI_EDIT_TOOL != name {
        return Vec::new();
    }

    let Some(edits) = input.get(EDITS_FIELD).and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut diffs = Vec::new();
    for edit in edits {
        if let Some((old, new)) = edit.get(OLD_FIELD).zip(edit.get(NEW_FIELD)) {
            diffs.push(Block::diff(path.clone(), textual(old), textual(new)));
        }
    }

    diffs
}

/// # Returns
///
/// What a call to a tool asked it to do: the one argument that says so where the tool has one, and
/// the whole input written out where it has not.
fn requested(name: &str, input: &Value) -> String {
    let argument = ARGUMENTS
        .iter()
        .find(|(tool, _)| *tool == name)
        .map(|(_, field)| *field);
    let said = argument
        .and_then(|field| input.get(field))
        .and_then(Value::as_str);
    if let Some(said) = said {
        return said.to_owned();
    }
    if Value::Null == *input {
        return String::new();
    }

    serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string())
}

/// # Returns
///
/// The blocks a run of assistant prose is written as, in the order they were written: the fenced
/// regions as code blocks holding exactly the bytes between their fences, and what is written
/// around them as messages. A run that is only the line endings separating two fences is no block
/// at all, so nothing here puts an empty message between them.
fn written(said: &str) -> Vec<(BlockKind, String)> {
    let mut blocks = Vec::new();
    let mut prose = 0;
    let mut open: Option<Opened> = None;
    let mut offset = 0;

    for line in said.split_inclusive(SEPARATOR) {
        let start = offset;
        offset += line.len();
        let bare = line.trim_end_matches([CARRIAGE, SEPARATOR]);

        match &open {
            None => {
                if let Some(fence) = opens(bare) {
                    prosaic(&mut blocks, &said[prose..start]);
                    open = Some(Opened {
                        body: offset,
                        ..fence
                    });
                }
            }
            Some(fence) if closes(bare, fence) => {
                blocks.push((
                    BlockKind::Code {
                        language: fence.language.clone(),
                    },
                    bodied(&said[fence.body..start], fence.indent),
                ));
                open = None;
                prose = offset;
            }
            Some(_) => {}
        }
    }

    match open {
        Some(fence) => blocks.push((
            BlockKind::Code {
                language: fence.language,
            },
            bodied(&said[fence.body..], fence.indent),
        )),
        None => prosaic(&mut blocks, &said[prose..]),
    }

    blocks
}

/// # Returns
///
/// The fence a line opens, or `None` where the line opens none.
fn opens(line: &str) -> Option<Opened> {
    let (mark, run, indent, rest) = fenced(line)?;
    if FENCE > run {
        return None;
    }

    let info = rest.trim();
    if BACKTICK == mark && info.contains(BACKTICK) {
        return None;
    }

    Some(Opened {
        mark,
        run,
        indent,
        language: info.split_whitespace().next().map(str::to_owned),
        body: 0,
    })
}

/// # Returns
///
/// Whether a line closes the fence `fence`, which takes a run of the same character at least as
/// long as the one that opened it and nothing but blanks after it.
fn closes(line: &str, fence: &Opened) -> bool {
    let Some((mark, run, _, rest)) = fenced(line) else {
        return false;
    };

    mark == fence.mark && run >= fence.run && rest.trim().is_empty()
}

/// # Returns
///
/// The character a line's fence is written with, how long the run of it is, how deep the line is
/// indented, and what follows that run, or `None` where the line is not a fence line at all.
fn fenced(line: &str) -> Option<(char, usize, usize, &str)> {
    let text = line.trim_start_matches(' ');
    let indent = line.len() - text.len();
    if INDENT < indent {
        return None;
    }

    let mark = text.chars().next().filter(|first| FENCES.contains(first))?;
    let run = text.chars().take_while(|written| *written == mark).count();

    Some((mark, run, indent, &text[run..]))
}

/// # Returns
///
/// A fenced region's body as a block holds it: the bytes between the fences, without the line
/// ending that carries the closing one, and with the `indent` spaces the opening fence was written
/// under taken off the front of every line that has them.
///
/// A fence written inside a numbered list is indented and the code it holds is not, which is how a
/// model writes one and how a markdown reader draws it back. A body that kept those spaces would
/// be a block that pasted back one indent deeper than the code that was sent.
fn bodied(body: &str, indent: usize) -> String {
    let body = body
        .strip_suffix(SEPARATOR)
        .map_or(body, |kept| kept.strip_suffix(CARRIAGE).unwrap_or(kept));
    if 0 == indent {
        return body.to_owned();
    }

    let mut written = String::with_capacity(body.len());
    for line in body.split_inclusive(SEPARATOR) {
        let deeper = line.len() - line.trim_start_matches(' ').len();
        written.push_str(&line[deeper.min(indent)..]);
    }

    written
}

/// Appends `said` to `blocks` as an assistant message, unless it is written from nothing but the
/// line endings that separated it from what was fenced around it.
fn prosaic(blocks: &mut Vec<(BlockKind, String)>, said: &str) {
    let written = said.trim_matches([CARRIAGE, SEPARATOR]);
    if written.is_empty() {
        return;
    }

    blocks.push((BlockKind::Message(Role::Assistant), written.to_owned()));
}
