//! The vim engine the editor's commands come from, and the seam keystrokes reach it through.
//!
//! vimbecode does not write its own motions, text objects, operators, registers, marks or counts:
//! modalkit's vim keybindings turn keystrokes into actions and modalkit's text runs them, which is
//! thousands of lines of vim semantics this workspace does not have to be right about. What lives
//! here is only the seam: the events an application loop already delivers are handed to the
//! keybinding machine, the actions it yields are run, and what they left behind is read back in
//! the terms the differential harness compares engines in.
//!
//! The engine is the authority on the text, the cursor, the mode and the registers, and on nothing
//! else. Where a line is drawn on a screen and how wide a grapheme is are the layout engine's
//! business, and the engine deliberately knows nothing about either: modalkit counts a line in
//! characters where a terminal counts it in cells, and reconciling the two is the work of the
//! [`shim`](crate::shim) an action passes through on its way from the keybinding machine to the
//! text. Everything a layout has no say in reaches the text exactly as it did before that shim
//! existed, and an engine can be built without one, which is what the seam is compared against.
//!
//! An action the seam does not run is reported rather than dropped. An engine that quietly ignores
//! what it was asked to do is an engine whose tests pass against a keystroke that did nothing, so
//! there is no arm here that swallows an action.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::num::NonZeroUsize;

use crossterm::event::{KeyCode, KeyModifiers};
use editor_types::context::Resolve;
use editor_types::prelude::{Register as Slot, TargetShape, ViewportContext};
use modalkit::actions::{Action, Editable, EditorAction};
use modalkit::editing::application::EmptyInfo;
use modalkit::editing::buffer::{CursorGroupId, EditBuffer};
use modalkit::editing::context::EditContext;
use modalkit::editing::cursor::Cursor;
use modalkit::editing::store::Store;
use modalkit::env::vim::keybindings::{default_vim_keys, VimMachine};
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use modalkit::keybindings::BindingMachine;
use vbc_layout::position::LogicalPosition;
use vbc_layout::width::graphemes;

use crate::event::{Event, KeyEvent};
use crate::screen::Geometry;
use crate::shim::{screen_motion, Shim};

/// The registers a run reads back, in the notation a register is addressed by in vim: the unnamed
/// register, the small-delete register, the yank register, the nine delete registers and the
/// twenty-six named ones, which are the ones the differential harness compares.
const READ_BACK: [char; 38] = [
    '"', '-', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g',
    'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
];

/// The identifier modalkit files the one text an engine edits under.
const ONLY_TEXT: &str = "vimbecode";

/// The columns a screen motion is measured in where an engine was not told what window it is being
/// typed at, which is the terminal every vim manual draws its examples in.
const DEFAULT_COLUMNS: usize = 80;

/// The screen lines a screen motion is measured in where an engine was not told what window it is
/// being typed at.
const DEFAULT_ROWS: usize = 24;

/// Where the cursor rests, counted the way the differential harness counts it: a zero-based line,
/// and a zero-based byte offset within that line.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Position {
    /// The zero-based index of the line the cursor is on.
    pub line: usize,

    /// The zero-based byte offset of the cursor within its line.
    pub column: usize,
}

/// How a register's text is laid out, which decides how a put reinserts it.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Shape {
    /// The text is inserted at the cursor, without starting a new line.
    Charwise,

    /// The text is inserted as whole lines, above or below the cursor's line.
    Linewise,

    /// The text is inserted as a rectangular block spanning consecutive lines.
    Blockwise,
}

impl From<TargetShape> for Shape {
    fn from(shape: TargetShape) -> Self {
        match shape {
            TargetShape::CharWise => Self::Charwise,
            TargetShape::LineWise => Self::Linewise,
            TargetShape::BlockWise => Self::Blockwise,
        }
    }
}

/// What a register holds, together with the layout a put would reinsert it with.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Held {
    /// The register's text, byte-exact.
    pub text: String,

    /// The layout the text is put back with.
    pub shape: Shape,
}

/// A vim engine: the keys typed at it so far, the text they have edited, and what they left in the
/// registers.
///
/// Keys go in one at a time and every action a key produces is run before the next key is read, so
/// an engine is never holding a half-run keystroke when its state is read back.
pub struct Engine {
    keys: VimMachine<TerminalKey>,
    text: EditBuffer<EmptyInfo>,
    store: Store<EmptyInfo>,
    group: CursorGroupId,
    window: ViewportContext<Cursor>,
    shim: Option<Shim>,
}

impl Engine {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created engine in normal mode, editing `text` with the cursor on its first
    /// character, and measuring the screen motions typed at it in a window of the size a vim
    /// manual draws its examples in.
    ///
    /// # Panics
    ///
    /// Panics if the default window is zero columns wide or zero rows tall, which it is not.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let columns = NonZeroUsize::new(DEFAULT_COLUMNS).expect("the default columns are not zero");
        let rows = NonZeroUsize::new(DEFAULT_ROWS).expect("the default rows are not zero");

        Self::laid_out_in(text, Geometry::new(columns, rows))
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created engine like [`Engine::new`]'s, measuring the screen motions typed at it in
    /// `geometry`.
    #[must_use]
    pub fn laid_out_in(text: &str, geometry: Geometry) -> Self {
        Self::built(text, Some(Shim::new(geometry)))
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created engine like [`Engine::new`]'s with no shim installed, so that a screen
    /// motion is answered by modalkit's own width math as everything was answered before the seam
    /// existed. This is the engine the seam is compared against.
    #[must_use]
    pub fn bypassing_the_shim(text: &str) -> Self {
        Self::built(text, None)
    }

    /// # Returns
    ///
    /// The shim the engine's screen motions pass through, and `None` where the engine was built
    /// without one.
    #[must_use]
    pub fn shim(&self) -> Option<&Shim> {
        self.shim.as_ref()
    }

    /// Types one key at the engine and runs everything that key asks for.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Engine::run`]'s return values on failure.
    pub fn press(&mut self, key: KeyEvent) -> Result<(), Error> {
        self.keys.input_key(key.into());
        while let Some((action, context)) = self.keys.pop() {
            self.run(&action, &context)?;
        }

        Ok(())
    }

    /// Types a sequence of keys at the engine, one key at a time.
    ///
    /// # Type Parameters
    ///
    /// * `KeysType` - The keys to type, in the order they are typed.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Engine::press`]'s return values on failure.
    pub fn press_all<KeysType: IntoIterator<Item = KeyEvent>>(
        &mut self,
        keys: KeysType,
    ) -> Result<(), Error> {
        for key in keys {
            self.press(key)?;
        }

        Ok(())
    }

    /// Hands the engine one of the events an application loop delivers.
    ///
    /// A key is typed at the engine, and pasted text is typed at it character by character since
    /// text that arrived as a paste is only text once the mode says so. Everything else an
    /// application loop hears about is about the terminal rather than about the text, and the
    /// engine has nothing to do with it.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Engine::press`]'s return values on failure.
    pub fn handle(&mut self, event: &Event) -> Result<(), Error> {
        match event {
            Event::Key(key) => self.press(*key),
            Event::Paste(paste) => {
                for character in paste.text.chars() {
                    self.press(typed(character))?;
                }

                Ok(())
            }
            Event::Resize { .. } | Event::Redraw | Event::Notice(_) => Ok(()),
        }
    }

    /// # Returns
    ///
    /// The text being edited, which ends in a newline as vim's own text does.
    #[must_use]
    pub fn text(&self) -> String {
        self.text.get_text()
    }

    /// # Returns
    ///
    /// Where the cursor rests in the text.
    pub fn cursor(&mut self) -> Position {
        let cursor = self.text.get_leader(self.group);
        let text = self.text.get_text();
        let line = text.split('\n').nth(cursor.y).unwrap_or_default();

        Position {
            line: cursor.y,
            column: line
                .chars()
                .take(cursor.x)
                .map(char::len_utf8)
                .sum::<usize>(),
        }
    }

    /// # Returns
    ///
    /// The mode the engine is in.
    #[must_use]
    pub fn mode(&self) -> VimMode {
        self.keys.mode()
    }

    /// # Returns
    ///
    /// What every register holding text holds, keyed by the name it is addressed by. A register
    /// holding nothing is left out, as it is on the side an engine is compared against.
    #[must_use]
    pub fn registers(&self) -> BTreeMap<char, Held> {
        let mut held = BTreeMap::new();
        for name in READ_BACK {
            let Ok(cell) = self.store.registers.get(&slot(name)) else {
                continue;
            };
            let text = cell.value.to_string();
            if text.is_empty() {
                continue;
            }
            held.insert(
                name,
                Held {
                    text,
                    shape: cell.shape.into(),
                },
            );
        }

        held
    }

    /// Runs one of the actions a keystroke produced.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Unsupported`] if the action is not one this seam drives.
    /// * Forwards [`Engine::edit`]'s return values on failure.
    fn run(&mut self, action: &Action, context: &EditContext) -> Result<(), Error> {
        match action {
            Action::NoOp => Ok(()),
            Action::Editor(editor) => self.edit(editor, context),
            action => Err(Error::Unsupported {
                action: format!("{action:?}"),
            }),
        }
    }

    /// Runs one of the actions that edit the text, offering it to the shim on the way.
    ///
    /// This is the seam: an action asking about cells is the shim's to answer, and everything else
    /// reaches the text as it stands. The shim recognises and measures but does not answer yet, so
    /// for the time being every action goes on to modalkit whatever the shim made of it.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Unrunnable`] if the action could not be run against the text.
    fn edit(&mut self, editor: &EditorAction, context: &EditContext) -> Result<(), Error> {
        if let Some(shim) = self.shim.as_mut() {
            if let Some((motion, count)) = screen_motion(editor) {
                let cursor = self.text.get_leader(self.group);
                let held = self
                    .text
                    .get()
                    .get_line(cursor.y)
                    .map(|line| line.to_string())
                    .unwrap_or_default();
                let line = held.strip_suffix('\n').unwrap_or(&held);
                let at = LogicalPosition {
                    line: cursor.y,
                    grapheme: grapheme_offset(line, cursor.x),
                };
                shim.intercept(motion, context.resolve(&count), at, line);
            }
        }

        self.text
            .editor_command(
                editor,
                &(self.group, &self.window, context),
                &mut self.store,
            )
            .map(|_info| ())
            .map_err(|error| Error::Unrunnable {
                action: format!("{editor:?}"),
                message: error.to_string(),
            })
    }

    /// # Returns
    ///
    /// A newly created engine editing `text`, whose screen motions pass through `shim`.
    fn built(text: &str, shim: Option<Shim>) -> Self {
        let mut edited = EditBuffer::from_str(ONLY_TEXT.to_owned(), text);
        let group = edited.create_group();

        Self {
            keys: default_vim_keys(),
            text: edited,
            store: Store::default(),
            group,
            window: ViewportContext::default(),
            shim,
        }
    }
}

/// The ways a keystroke can fail to leave an engine in a state worth reading back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// An action could not be run against the text.
    Unrunnable {
        /// The action that could not be run.
        action: String,

        /// What modalkit reported.
        message: String,
    },

    /// An action is not one this seam drives.
    Unsupported {
        /// The action nothing here runs.
        action: String,
    },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Unrunnable { action, message } => {
                write!(formatter, "`{action}` could not be run: {message}")
            }
            Self::Unsupported { action } => {
                write!(formatter, "`{action}` is not an action the engine runs")
            }
        }
    }
}

impl StdError for Error {}

/// # Returns
///
/// The key event a terminal reports when `character` is typed with no modifier held.
#[must_use]
pub fn typed(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
}

/// # Returns
///
/// The grapheme of `line` holding the character `characters` characters into it, which is the
/// offset past the line's last grapheme where the line is shorter than that. A grapheme is one or
/// more characters, so a cursor modalkit put in the middle of a combining sequence or an emoji is
/// reported here as standing on the whole of it, which is where a screen draws it.
fn grapheme_offset(line: &str, characters: usize) -> usize {
    let mut counted = 0;
    let mut offset = 0;
    for grapheme in graphemes(line) {
        counted += grapheme.chars().count();
        if characters < counted {
            return offset;
        }
        offset += 1;
    }

    offset
}

/// # Returns
///
/// The register modalkit addresses by the name vim addresses it by.
fn slot(name: char) -> Slot {
    match name {
        '"' => Slot::Unnamed,
        '-' => Slot::SmallDelete,
        '0' => Slot::LastYanked,
        '1'..='9' => Slot::RecentlyDeleted(name as usize - '1' as usize),
        name => Slot::Named(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line whose graphemes are not its characters: a family emoji joined by zero-width joiners,
    /// an accented letter written as a combining sequence, and a plain letter.
    const CLUSTERED: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}e\u{301}x";

    #[test]
    fn a_character_inside_a_cluster_is_reported_as_the_cluster_it_falls_in() {
        assert_eq!(0, grapheme_offset(CLUSTERED, 0));
        assert_eq!(0, grapheme_offset(CLUSTERED, 4));
        assert_eq!(1, grapheme_offset(CLUSTERED, 5));
        assert_eq!(1, grapheme_offset(CLUSTERED, 6));
        assert_eq!(2, grapheme_offset(CLUSTERED, 7));
    }

    #[test]
    fn a_character_past_the_end_of_a_line_is_reported_as_the_offset_past_its_last_grapheme() {
        assert_eq!(3, grapheme_offset(CLUSTERED, 8));
        assert_eq!(3, grapheme_offset(CLUSTERED, 99));
        assert_eq!(0, grapheme_offset("", 0));
    }
}
