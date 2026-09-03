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
//! The size of the window an engine is laid out in is handed on to modalkit all the same, because
//! a screen line is a screen line only relative to some window, and a motion counted in them has
//! nowhere to land without one.
//!
//! What the shim answers with is a place in the logical text, and turning that into an edit is the
//! engine's. It writes the place into a mark and re-issues the very action it was handed with only
//! its target replaced, so the operator the keys asked for is the operator modalkit runs. The rules
//! that decide what an operator takes between two places are still vim's rather than this
//! workspace's, but they are not modalkit's -- an exclusive motion ending in the first column of a
//! line covers the line above it, and covers whole lines where the cursor stood in an indent -- so
//! they are applied here, by choosing where the mark goes and whether the target jumps to a
//! character or to a line.
//!
//! An action the seam does not run is reported rather than dropped. An engine that quietly ignores
//! what it was asked to do is an engine whose tests pass against a keystroke that did nothing, so
//! there is no arm here that swallows an action. A motion the shim classifies as out of scope is
//! reported for the same reason: modalkit would answer it, in characters, at a place vim does not
//! put the cursor, and an editor that does that quietly is harder to trust than one that says so.
//! Whether a shim is installed makes no difference to that -- the classification is the editor's
//! decision about what it answers rather than the shim's about what it measures -- so the engine
//! the seam is compared against refuses exactly what the engine with the seam refuses. The one
//! action that runs as less than it was typed is the operator whose motion ran out of text, which
//! vim abandons too, and the shim records the motion that could not travel rather than leaving the
//! abandonment to be inferred.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::num::NonZeroUsize;

use crossterm::event::{KeyCode, KeyModifiers};
use editor_types::context::Resolve;
use editor_types::prelude::{
    Count, EditTarget, Mark, Register as Slot, Specifier, TargetShape, ViewportContext,
};
use editor_types::EditAction;
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
use crate::shim::{classified, Classification, Landing, ScreenMotion, Shim, Text};

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

/// The mark a screen motion's answer reaches modalkit through. A mark is the only place an edit
/// target names a position from, and this one is named by a character no keystroke can ask for, so
/// re-issuing an action against it disturbs none of the marks a text is edited with.
const SCRATCH: Mark = Mark::BufferNamed('\u{0}');

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
        let shim = Shim::new(geometry.clone());

        Self::built(text, &geometry, Some(shim))
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created engine like [`Engine::laid_out_in`]'s with no shim installed, so that a
    /// screen motion is answered by modalkit's own width math as everything was answered before
    /// the seam existed. This is the engine the seam is compared against, and it is laid out in
    /// the same window so that the shim is the only thing the comparison holds.
    #[must_use]
    pub fn bypassing_the_shim(text: &str, geometry: &Geometry) -> Self {
        Self::built(text, geometry, None)
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
    /// This is the seam: an action asking about cells is the shim's to answer, an action asking
    /// about cells that nothing here answers is refused, and everything else reaches the text as
    /// it stands. What the shim answers with is a place in the logical text, which reaches
    /// modalkit as a mark the very same action is re-issued against, so the operator the keys
    /// asked for is the operator that runs and only the target under it is replaced.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::OutOfScope`] if the motion lands where display geometry says and nothing here
    ///   measures it.
    /// * [`Error::Unclassified`] if the motion is one the shim's audit does not name.
    /// * Forwards [`Engine::apply`]'s return values on failure.
    fn edit(&mut self, editor: &EditorAction, context: &EditContext) -> Result<(), Error> {
        match classified(editor) {
            Some((Classification::OutOfScope { keys }, _)) => Err(Error::OutOfScope {
                keys: keys.to_owned(),
            }),
            Some((Classification::Unclassified, _)) => Err(Error::Unclassified {
                action: format!("{editor:?}"),
            }),
            Some((Classification::Intercepted(motion), count)) => {
                match self.answered(editor, motion, &count, context) {
                    Some(answered) => self.apply(&answered, context),
                    None => self.apply(editor, context),
                }
            }
            Some((Classification::Characterwise, _)) | None => self.apply(editor, context),
        }
    }

    /// Offers an intercepted motion to the shim and turns what it answers into an action modalkit
    /// runs against a mark rather than against a width it would have to measure for itself.
    ///
    /// # Returns
    ///
    /// The action to run in place of `editor`, and `None` where the shim does not answer the
    /// motion, which leaves the action to reach the text exactly as it did before the seam
    /// existed.
    fn answered(
        &mut self,
        editor: &EditorAction,
        motion: ScreenMotion,
        count: &Count,
        context: &EditContext,
    ) -> Option<EditorAction> {
        let EditorAction::Edit(operator, _) = editor else {
            return None;
        };
        let cursor = self.text.get_leader(self.group);
        let at = LogicalPosition {
            line: cursor.y,
            grapheme: grapheme_offset(&line_of(&self.text, cursor.y), cursor.x),
        };
        let landing = {
            let shim = self.shim.as_mut()?;
            shim.answer(motion, context.resolve(count), at, &Lines(&self.text))?
        };

        Some(self.retargeted(context.resolve(operator), cursor, landing))
    }

    /// Puts the scratch mark where a screen motion's answer says the motion goes, moving the
    /// cursor to the near end of what an operator takes where vim moves it there too.
    ///
    /// The one screen motion that takes the grapheme it stops on is `g$`, which never runs
    /// backwards, so the grapheme an operator has to be carried past is always the far end of what
    /// it takes.
    ///
    /// # Returns
    ///
    /// The action running `operator` over the answer, which is the operator asked for over a
    /// target modalkit reads off the mark rather than measures.
    fn retargeted(
        &mut self,
        operator: EditAction,
        cursor: Cursor,
        landing: Landing,
    ) -> EditorAction {
        let to = self.placed(landing.at);
        if EditAction::Motion == operator || !landing.complete {
            return self.against(EditAction::Motion, to, false);
        }

        let (near, mut far) = if to < cursor {
            (to, cursor)
        } else {
            (cursor, to)
        };
        if landing.inclusive {
            far = self.placed(LogicalPosition {
                line: landing.at.line,
                grapheme: landing.at.grapheme + 1,
            });
        } else if 0 == far.x && near.y < far.y {
            let above = far.y - 1;
            if in_indent(&line_of(&self.text, near.y), near.x) {
                self.text.set_leader(self.group, near);

                return self.against(operator, Cursor::new(above, 0), true);
            }
            far = Cursor::new(above, line_of(&self.text, above).chars().count());
        }
        self.text.set_leader(self.group, near);

        self.against(operator, far, false)
    }

    /// Puts the scratch mark at `mark`.
    ///
    /// # Returns
    ///
    /// The action running `operator` from the cursor to that mark, over whole lines where
    /// `linewise` says so and over the characters between them where it does not.
    fn against(&mut self, operator: EditAction, mark: Cursor, linewise: bool) -> EditorAction {
        self.store.cursors.set_mark(self.text.id(), SCRATCH, mark);
        let target = if linewise {
            EditTarget::LineJump(Specifier::Exact(SCRATCH))
        } else {
            EditTarget::CharJump(Specifier::Exact(SCRATCH))
        };

        EditorAction::Edit(Specifier::Exact(operator), target)
    }

    /// # Returns
    ///
    /// The cursor standing where `at` does, counted the way modalkit counts a column: in the
    /// characters of the line rather than in its graphemes.
    fn placed(&mut self, at: LogicalPosition) -> Cursor {
        Cursor::new(
            at.line,
            character_offset(&line_of(&self.text, at.line), at.grapheme),
        )
    }

    /// Runs one of the actions that edit the text against the text as it stands.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Unrunnable`] if the action could not be run against the text.
    fn apply(&mut self, editor: &EditorAction, context: &EditContext) -> Result<(), Error> {
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
    /// A newly created engine editing `text` in the window `geometry` describes, whose screen
    /// motions pass through `shim`.
    fn built(text: &str, geometry: &Geometry, shim: Option<Shim>) -> Self {
        let mut edited = EditBuffer::from_str(ONLY_TEXT.to_owned(), text);
        let group = edited.create_group();
        let window = ViewportContext {
            corner: Cursor::default(),
            dimensions: (geometry.columns().get(), geometry.window().height().get()),
            wrap: true,
        };

        Self {
            keys: default_vim_keys(),
            text: edited,
            store: Store::default(),
            group,
            window,
            shim,
        }
    }
}

/// The ways a keystroke can fail to leave an engine in a state worth reading back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// A motion that lands where display geometry says, which nothing here measures.
    OutOfScope {
        /// The keys vim's manual names the motion by.
        keys: String,
    },

    /// A motion the shim's audit does not name.
    Unclassified {
        /// The action nothing here classifies.
        action: String,
    },

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
            Self::OutOfScope { keys } => write!(
                formatter,
                "`{keys}` lands at a screen column this editor does not measure yet, so it is \
                 refused rather than answered by counting characters"
            ),
            Self::Unclassified { action } => write!(
                formatter,
                "`{action}` is a motion the screen-motion audit does not classify; classify it \
                 in `vbc_editor::shim::classify`"
            ),
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

/// The engine's own text, in the terms a screen motion is measured against.
struct Lines<'text>(&'text EditBuffer<EmptyInfo>);

impl Text for Lines<'_> {
    fn lines(&self) -> usize {
        self.0.get_lines()
    }

    fn line(&self, line: usize) -> Option<String> {
        (line < self.lines()).then(|| line_of(self.0, line))
    }
}

/// # Returns
///
/// The text of the logical line `line` of `text`, without its line ending, and empty past the last
/// line.
fn line_of(text: &EditBuffer<EmptyInfo>, line: usize) -> String {
    let held = text
        .get()
        .get_line(line)
        .map(|held| held.to_string())
        .unwrap_or_default();

    held.strip_suffix('\n').unwrap_or(&held).to_owned()
}

/// # Returns
///
/// The number of characters of `line` in front of its grapheme `grapheme`, which is the line's own
/// character count where the line holds fewer graphemes than that.
fn character_offset(line: &str, grapheme: usize) -> usize {
    graphemes(line)
        .take(grapheme)
        .map(|held| held.chars().count())
        .sum()
}

/// # Returns
///
/// Whether the character `column` of `line` falls in the line's indent, which is where vim asks an
/// operator's start to stand for a motion ending in the first column of a line to cover whole
/// lines rather than characters.
fn in_indent(line: &str, column: usize) -> bool {
    column
        <= line
            .chars()
            .take_while(|held| matches!(held, ' ' | '\t'))
            .count()
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
