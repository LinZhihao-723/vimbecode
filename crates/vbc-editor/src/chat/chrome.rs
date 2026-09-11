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
//! Every colour is one of [`crate::chat::palette`]'s, at the depth the terminal says it draws.

use ratatui::style::{Modifier, Style};

use crate::chat::block::{Block, Kind, Role};
use crate::chat::fold::Entry;
use crate::chat::palette::{Palette, Rgb, COMMENT, CYAN, FOREGROUND, GREEN, YELLOW};
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

/// base16 Ocean's lighter background, which a prompt's band is drawn in.
const BAND: Rgb = Rgb::new(0x34, 0x3d, 0x46);

/// How thinking is drawn.
const THINKING: Style = Style::new().add_modifier(Modifier::DIM.union(Modifier::ITALIC));

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
    /// How the row is drawn in `palette`.
    #[must_use]
    pub fn label(&self, palette: Palette) -> Label {
        match self {
            Self::Gap => Label::default(),
            Self::Call(_) => Label::named(SAID, marked(palette, GREEN), Style::new()),
            Self::Waiting(_) => {
                let waiting = marked(palette, YELLOW);

                Label::new(SAID, waiting, waiting)
            }
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
/// How a row of a block of `kind` is drawn in `palette`, where `first` says whether it is the
/// block's first.
#[must_use]
pub fn body(kind: &Kind, first: bool, palette: Palette) -> Label {
    let mark = |mark: &'static str| if first { mark } else { "" };

    match kind {
        Kind::Message(Role::User) => {
            let band = Style::new().bg(palette.color(BAND));
            let marked = band.patch(self::marked(palette, FOREGROUND));

            Label::new(mark(PROMPT), marked, band)
        }
        Kind::Message(Role::Assistant) => {
            Label::new(mark(SAID), marked(palette, CYAN), Style::new())
        }
        Kind::ToolResult => Label::new(mark(ANSWER), tinted(palette, COMMENT), Style::new()),
        Kind::Thinking => Label::new(mark(THOUGHT), THINKING, THINKING),
        Kind::Waiting { .. } => {
            let waiting = tinted(palette, YELLOW);

            Label::new("", waiting, waiting)
        }
        Kind::Code { .. } | Kind::Diff { .. } | Kind::ToolCall { .. } => Label::default(),
    }
}

/// # Returns
///
/// How the one row a closed fold headed by a block of `kind` is drawn in `palette`.
#[must_use]
pub fn folded(kind: &Kind, palette: Palette) -> Label {
    match kind {
        Kind::ToolCall { .. } => Label::named(SAID, marked(palette, GREEN), Style::new()),
        Kind::ToolResult => {
            let answered = tinted(palette, COMMENT);

            Label::new(ANSWER, answered, answered)
        }
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

/// # Returns
///
/// A style drawing its text in `rgb`, at `palette`'s depth.
fn tinted(palette: Palette, rgb: Rgb) -> Style {
    Style::new().fg(palette.color(rgb))
}

/// # Returns
///
/// The style a mark drawn in `rgb` is drawn in, at `palette`'s depth.
fn marked(palette: Palette, rgb: Rgb) -> Style {
    tinted(palette, rgb).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use crate::chat::block::{Block, Kind, Role};
    use crate::chat::fold::Entry;
    use crate::chat::palette::Palette;
    use crate::chat::transcript::Transcript;

    use super::{above, body, heads, Chrome, PROMPT, SAID};

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

    #[test]
    fn a_prompt_and_a_reply_are_marked_on_their_first_row_in_the_256_colours() {
        let prompt = body(&Kind::Message(Role::User), true, Palette::Indexed);
        let continued = body(&Kind::Message(Role::User), false, Palette::Indexed);
        let reply = body(&Kind::Message(Role::Assistant), true, Palette::Indexed);

        assert_eq!(
            (PROMPT, "", SAID),
            (prompt.mark(), continued.mark(), reply.mark())
        );
        assert!(matches!(prompt.style().bg, Some(Color::Indexed(16..))));
        assert_eq!(prompt.style(), continued.style());
        assert!(matches!(reply.marked().fg, Some(Color::Indexed(16..))));
        assert_eq!(None, reply.style().bg);
    }
}
