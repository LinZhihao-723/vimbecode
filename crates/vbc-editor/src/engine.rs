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
//! characters where a terminal counts it in cells, and reconciling the two is the work of the shim
//! that sits above this seam rather than of the seam itself.
//!
//! An action the seam does not run is reported rather than dropped. An engine that quietly ignores
//! what it was asked to do is an engine whose tests pass against a keystroke that did nothing, so
//! there is no arm here that swallows an action.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};

use crossterm::event::{KeyCode, KeyModifiers};
use editor_types::prelude::{Register as Slot, TargetShape, ViewportContext};
use modalkit::actions::{Action, Editable};
use modalkit::editing::application::EmptyInfo;
use modalkit::editing::buffer::{CursorGroupId, EditBuffer};
use modalkit::editing::context::EditContext;
use modalkit::editing::cursor::Cursor;
use modalkit::editing::store::Store;
use modalkit::env::vim::keybindings::{default_vim_keys, VimMachine};
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use modalkit::keybindings::BindingMachine;

use crate::event::{Event, KeyEvent};

/// The registers a run reads back, in the notation a register is addressed by in vim: the unnamed
/// register, the small-delete register, the yank register, the nine delete registers and the
/// twenty-six named ones, which are the ones the differential harness compares.
const READ_BACK: [char; 38] = [
    '"', '-', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g',
    'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
];

/// The identifier modalkit files the one text an engine edits under.
const ONLY_TEXT: &str = "vimbecode";

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
}

impl Engine {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created engine in normal mode, editing `text` with the cursor on its first
    /// character.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut edited = EditBuffer::from_str(ONLY_TEXT.to_owned(), text);
        let group = edited.create_group();

        Self {
            keys: default_vim_keys(),
            text: edited,
            store: Store::default(),
            group,
            window: ViewportContext::default(),
        }
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
    /// * [`Error::Unrunnable`] if the action could not be run against the text.
    /// * [`Error::Unsupported`] if the action is not one this seam drives.
    fn run(&mut self, action: &Action, context: &EditContext) -> Result<(), Error> {
        match action {
            Action::NoOp => Ok(()),
            Action::Editor(editor) => self
                .text
                .editor_command(
                    editor,
                    &(self.group, &self.window, context),
                    &mut self.store,
                )
                .map(|_info| ())
                .map_err(|error| Error::Unrunnable {
                    action: format!("{editor:?}"),
                    message: error.to_string(),
                }),
            action => Err(Error::Unsupported {
                action: format!("{action:?}"),
            }),
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
