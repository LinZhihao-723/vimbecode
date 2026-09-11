//! What the history panel draws around what was said, and the colours it draws it in.
//!
//! Everything here is chrome in design §9's sense: it is drawn and never yanked. Every row of the
//! history keeps [`GUTTER`] columns to the left of its text for a mark saying who said the block
//! the row belongs to -- `›` for the reader's prompt, `●` for Claude -- drawn on the block's first
//! row only. A prompt is drawn on a band across the whole of the panel, thinking is drawn dim, and
//! a call the session is waiting on the reader about is drawn in a colour nothing else is.
//!
//! Some rows hold no byte of any block: the blank row setting a turn apart from the one before it,
//! and the header naming a call above the rows it was called with. Such a row stands above the
//! entry it belongs to, so a reader scrolls onto it and past it like any other row, and the cursor
//! never rests on it because it draws nothing the cursor could rest on.
//!
//! Every colour is one of the 240 of the 256-colour palette a terminal theme does not redefine, so
//! a terminal that draws no more than 256 colours draws all of them.

use ratatui::style::{Color, Modifier, Style};

use crate::chat::block::{Block, Kind, Role};
use crate::chat::fold::Entry;
use crate::chat::transcript::Transcript;

/// The columns the history keeps to the left of every row's text for the row's mark.
pub const GUTTER: usize = 2;

/// The style the name of a tool is drawn in within a header.
pub const NAMED: Style = Style::new().add_modifier(Modifier::BOLD);

/// The marks a block's first row is drawn behind: the reader's prompt, what Claude said or called,
/// what a tool answered, and what Claude thought.
const PROMPT: &str = "›";
const SAID: &str = "●";
const ANSWER: &str = "⎿";
const THOUGHT: &str = "✻";

/// What the header of a call the session is waiting on says after the tool's name, and the
/// commands that answer it.
const WAITING_ON: &str = "is waiting on you --";
const ALLOWED: &str = "`:allow` or `:deny`";
const ANSWERED: &str = "`:answer <text>`";

/// The colours the history is drawn in.
const BAND: Color = Color::Indexed(237);
const PALE: Color = Color::Indexed(252);
const TEAL: Color = Color::Indexed(37);
const GREEN: Color = Color::Indexed(114);
const GREY: Color = Color::Indexed(245);
const ORANGE: Color = Color::Indexed(214);

/// How a prompt's rows are drawn, and the mark in front of it.
const PROMPTED: Style = Style::new().bg(BAND);
const PROMPT_MARK: Style = Style::new()
    .fg(PALE)
    .bg(BAND)
    .add_modifier(Modifier::BOLD);

/// How the mark in front of a reply is drawn.
const REPLY_MARK: Style = Style::new().fg(TEAL).add_modifier(Modifier::BOLD);

/// How the mark in front of a call is drawn.
const CALL_MARK: Style = Style::new().fg(GREEN).add_modifier(Modifier::BOLD);

/// How what a tool answered is drawn where it is folded away.
const ANSWERED_STYLE: Style = Style::new().fg(GREY);

/// How thinking is drawn.
const THINKING: Style = Style::new().add_modifier(Modifier::DIM.union(Modifier::ITALIC));

/// How a call the session is waiting on is drawn, and its header.
const WAITING: Style = Style::new().fg(ORANGE);
const WAITING_HEADER: Style = WAITING.add_modifier(Modifier::BOLD);

/// A row of the history that holds no byte of any block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Chrome {
    /// The blank row a turn is set apart from the one before it by.
    Gap,

    /// The header naming the call whose rows are drawn below it.
    Call(String),

    /// The header saying the call below it is waiting on the reader, and what answers it.
    Waiting(String),
}

impl Chrome {
    /// # Returns
    ///
    /// What the row says, which is nothing for a gap.
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Gap => "",
            Self::Call(text) | Self::Waiting(text) => text,
        }
    }

    /// # Returns
    ///
    /// How the row is drawn.
    #[must_use]
    pub fn label(&self) -> Label {
        match self {
            Self::Gap => Label::default(),
            Self::Call(_) => Label::named(SAID, CALL_MARK, Style::new()),
            Self::Waiting(_) => Label::new(SAID, WAITING_HEADER, WAITING_HEADER),
        }
    }
}

/// How one row of the history is drawn: the mark in its gutter and the style of that mark, the
/// style the rest of the row is drawn in beneath whatever the block's own spans paint, and whether
/// the row opens with the name of a tool.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Label {
    mark: &'static str,
    marked: Style,
    style: Style,
    titled: bool,
}

impl Label {
    #[must_use]
    pub fn mark(&self) -> &'static str {
        self.mark
    }

    #[must_use]
    pub fn marked(&self) -> Style {
        self.marked
    }

    #[must_use]
    pub fn style(&self) -> Style {
        self.style
    }

    /// # Returns
    ///
    /// Whether the row opens with the name of a tool, which is drawn in [`NAMED`].
    #[must_use]
    pub fn titled(&self) -> bool {
        self.titled
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A label drawing `mark` in `marked` and the row in `style`.
    const fn new(mark: &'static str, marked: Style, style: Style) -> Self {
        Self {
            mark,
            marked,
            style,
            titled: false,
        }
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A label like [`Label::new`]'s, of a row opening with the name of a tool.
    const fn named(mark: &'static str, marked: Style, style: Style) -> Self {
        Self {
            titled: true,
            ..Self::new(mark, marked, style)
        }
    }
}

/// # Returns
///
/// How a row of a block of `kind` is drawn, where `first` says whether it is the block's first.
#[must_use]
pub fn body(kind: &Kind, first: bool) -> Label {
    let marked = |mark: &'static str, marked: Style, style: Style| {
        Label::new(if first { mark } else { "" }, marked, style)
    };

    match kind {
        Kind::Message(Role::User) => marked(PROMPT, PROMPT_MARK, PROMPTED),
        Kind::Message(Role::Assistant) => marked(SAID, REPLY_MARK, Style::new()),
        Kind::ToolResult => marked(ANSWER, ANSWERED_STYLE, Style::new()),
        Kind::Thinking => marked(THOUGHT, THINKING, THINKING),
        Kind::Waiting { .. } => Label::new("", WAITING, WAITING),
        Kind::Code { .. } | Kind::Diff { .. } | Kind::ToolCall { .. } => Label::default(),
    }
}

/// # Returns
///
/// How the one row a closed fold headed by a block of `kind` is drawn.
#[must_use]
pub fn folded(kind: &Kind) -> Label {
    match kind {
        Kind::ToolCall { .. } => Label::named(SAID, CALL_MARK, Style::new()),
        Kind::ToolResult => Label::new(ANSWER, ANSWERED_STYLE, ANSWERED_STYLE),
        Kind::Thinking => Label::new(THOUGHT, THINKING, THINKING),
        _ => Label::default(),
    }
}

/// # Returns
///
/// Whether a row of chrome stands above the entry `entry` of `entries`, which is what
/// [`above`] answers without writing the row out.
#[must_use]
pub fn heads(transcript: &Transcript, entries: &[Entry], entry: usize) -> bool {
    let Some(block) = drawn(transcript, entries, entry) else {
        return false;
    };

    match block.kind() {
        Kind::Message(Role::User) => 0 < entry,
        Kind::ToolCall { .. } | Kind::Diff { .. } | Kind::Waiting { .. } => true,
        Kind::Message(Role::Assistant) | Kind::Code { .. } | Kind::ToolResult | Kind::Thinking => {
            false
        }
    }
}

/// # Returns
///
/// The row of chrome standing above the entry `entry` of `entries`: the gap above every prompt but
/// the first, and the header above the rows of an open call, a diff, and a call the session is
/// waiting on -- or `None` where the entry stands under no chrome.
#[must_use]
pub fn above(transcript: &Transcript, entries: &[Entry], entry: usize) -> Option<Chrome> {
    let block = drawn(transcript, entries, entry)?;

    match block.kind() {
        Kind::Message(Role::User) => (0 < entry).then_some(Chrome::Gap),
        Kind::ToolCall { .. } | Kind::Diff { .. } => {
            block.header(transcript.directory()).map(Chrome::Call)
        }
        Kind::Waiting { name, words } => Some(Chrome::Waiting(format!(
            "{name} {WAITING_ON} {}",
            answered_by(*words)
        ))),
        Kind::Message(Role::Assistant) | Kind::Code { .. } | Kind::ToolResult | Kind::Thinking => {
            None
        }
    }
}

/// # Returns
///
/// The commands that answer a call the session is waiting on, which is `:answer` where it is
/// answered in words and `:allow` or `:deny` where it is not.
#[must_use]
pub fn answered_by(words: bool) -> &'static str {
    if words {
        ANSWERED
    } else {
        ALLOWED
    }
}

/// # Returns
///
/// A header split into the name of the tool it names and what follows the name.
#[must_use]
pub fn titled(header: &str) -> (&str, &str) {
    header
        .find('(')
        .map_or((header, ""), |bracket| header.split_at(bracket))
}

/// # Returns
///
/// The block the entry `entry` of `entries` draws in rows of its own, or `None` where it draws a
/// closed fold or there is no such entry.
fn drawn<'transcript>(
    transcript: &'transcript Transcript,
    entries: &[Entry],
    entry: usize,
) -> Option<&'transcript Block> {
    match entries.get(entry)? {
        Entry::Body(block) => transcript.block(*block),
        Entry::Summary(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::chat::block::{Block, Kind, Role};
    use crate::chat::fold::Entry;
    use crate::chat::transcript::Transcript;

    use super::{above, heads, Chrome};

    #[test]
    fn every_prompt_but_the_first_is_set_apart_by_a_gap() {
        let transcript: Transcript = [
            Block::new(Kind::Message(Role::User), "one".to_owned()),
            Block::new(Kind::Message(Role::Assistant), "two".to_owned()),
            Block::new(Kind::Message(Role::User), "three".to_owned()),
        ]
        .into_iter()
        .collect();
        let entries = [Entry::Body(0), Entry::Body(1), Entry::Body(2)];

        assert_eq!(
            vec![None, None, Some(Chrome::Gap)],
            (0..3)
                .map(|entry| above(&transcript, &entries, entry))
                .collect::<Vec<Option<Chrome>>>()
        );
        for entry in 0..3 {
            assert_eq!(
                above(&transcript, &entries, entry).is_some(),
                heads(&transcript, &entries, entry)
            );
        }
    }

    #[test]
    fn a_call_is_headed_by_its_tool_and_a_path_inside_the_session_relative_to_it() {
        let mut transcript: Transcript = [
            Block::new(
                Kind::ToolCall {
                    name: "Write".to_owned(),
                },
                "/work/project/src/hello.rs".to_owned(),
            ),
            Block::diff("/work/project/src/main.rs".to_owned(), "a\n", "b\n"),
            Block::new(
                Kind::ToolCall {
                    name: "Read".to_owned(),
                },
                "/elsewhere/notes.txt".to_owned(),
            ),
        ]
        .into_iter()
        .collect();
        transcript.set_directory("/work/project".to_owned());
        let entries = [Entry::Body(0), Entry::Body(1), Entry::Body(2)];

        assert_eq!(
            vec![
                Some(Chrome::Call("Write(src/hello.rs)".to_owned())),
                Some(Chrome::Call("Edit(src/main.rs)".to_owned())),
                Some(Chrome::Call("Read(/elsewhere/notes.txt)".to_owned())),
            ],
            (0..3)
                .map(|entry| above(&transcript, &entries, entry))
                .collect::<Vec<Option<Chrome>>>()
        );
    }

    #[test]
    fn a_waiting_call_says_which_command_answers_it() {
        let transcript: Transcript = [
            Block::new(
                Kind::Waiting {
                    name: "Write".to_owned(),
                    words: false,
                },
                "waiting".to_owned(),
            ),
            Block::new(
                Kind::Waiting {
                    name: "AskUserQuestion".to_owned(),
                    words: true,
                },
                "waiting".to_owned(),
            ),
        ]
        .into_iter()
        .collect();
        let entries = [Entry::Body(0), Entry::Body(1)];

        assert_eq!(
            Some("Write is waiting on you -- `:allow` or `:deny`"),
            above(&transcript, &entries, 0).as_ref().map(Chrome::text)
        );
        assert_eq!(
            Some("AskUserQuestion is waiting on you -- `:answer <text>`"),
            above(&transcript, &entries, 1).as_ref().map(Chrome::text)
        );
    }
}
