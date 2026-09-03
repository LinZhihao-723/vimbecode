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
//! engine's. A cursor is written straight to it. An operator cannot be: it spans a range rather
//! than naming a place, and dropping it would leave the text byte for byte as it was. So the place
//! is written into a mark and the very same action is re-issued with only its target replaced,
//! which keeps the operator the keys asked for the operator that runs. The rules deciding what an
//! operator takes between two places are still vim's rather than this workspace's, but they are
//! not modalkit's -- an exclusive motion ending in the first column of a line covers to the end of
//! the line above, and covers whole lines where the cursor stood in an indent -- so they are
//! applied here, by choosing where the mark goes and whether the target jumps to a character or to
//! a line.
//!
//! The shift operators are the one family of edits the seam runs itself rather than handing on.
//! modalkit ships `EditBuffer::indent` as a stub that returns without touching the text, so `>>`,
//! `<<` and everything spelled with them reached the buffer and did nothing at all. What they do
//! instead is decided here: the target names whole logical lines however it was reached, and the
//! whitespace those lines are written out in is the [`indent`](crate::indent) module's, since a
//! step of a shift is measured in screen columns and a tab is worth as many of them as it takes to
//! reach the next tab stop. The lines are spliced rather than the text replaced, because a shift
//! has to be a change `u` takes back and replacing the text reinitializes the buffer's history. A
//! target this seam cannot turn into lines is refused for the same reason every other unanswered
//! action is, and so is the blockwise selection, whose shift vim lays down at the block's own left
//! column rather than at the start of the lines it spans.
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
//! vim abandons too, and the shim reports the walk that could not travel rather than leaving the
//! abandonment to be inferred.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::num::NonZeroUsize;

use crossterm::event::{KeyCode, KeyModifiers};
use editor_types::context::{EditContextBuilder, Resolve};
use editor_types::prelude::{
    Count, EditTarget, IndentChange, Mark, MoveDir1D, MovePosition, MoveType, RangeType,
    Register as Slot, Specifier, TargetShape, ViewportContext,
};
use editor_types::EditAction;
use modalkit::actions::{Action, Editable, EditorAction, InsertTextAction};
use modalkit::editing::application::EmptyInfo;
use modalkit::editing::buffer::{CursorGroupId, EditBuffer};
use modalkit::editing::context::EditContext;
use modalkit::editing::cursor::{Cursor, CursorGroup, CursorState};
use modalkit::editing::rope::EditRope;
use modalkit::editing::store::Store;
use modalkit::env::vim::keybindings::{default_vim_keys, VimMachine};
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use modalkit::keybindings::BindingMachine;
use vbc_layout::position::LogicalPosition;
use vbc_layout::width::graphemes;

use crate::event::{Event, KeyEvent};
use crate::indent::{indent_of, resting_column, Shift};
use crate::screen::Geometry;
use crate::shim::{classified, Classification, Landing, Shim, Text};

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
    shift: Shift,
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
    /// The same engine whose shift operators lay `shift`'s whitespace down.
    #[must_use]
    pub fn indenting_by(mut self, shift: Shift) -> Self {
        self.shift = shift;

        self
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
    /// This is the seam: a bare motion counted in cells is the shim's to answer and is written
    /// straight onto the cursor, a motion counted in cells that nothing here measures is refused,
    /// and everything else reaches the text as it stands. An operator applied to an intercepted
    /// motion spans a range rather than naming a place, so the shim's answer is written into a
    /// mark and the very same operator is re-issued against it.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::OutOfScope`] if the motion lands where display geometry says and nothing here
    ///   measures it.
    /// * [`Error::Unclassified`] if the motion is one the shim's audit does not name.
    /// * Forwards [`Engine::apply`]'s return values on failure.
    /// * Forwards [`Engine::reindent`]'s return values on failure.
    fn edit(&mut self, editor: &EditorAction, context: &EditContext) -> Result<(), Error> {
        match classified(editor) {
            Some((Classification::OutOfScope { keys }, _)) => {
                return Err(Error::OutOfScope {
                    keys: keys.to_owned(),
                });
            }
            Some((Classification::Unclassified, _)) => {
                return Err(Error::Unclassified {
                    action: format!("{editor:?}"),
                });
            }
            _ => {}
        }

        let landing = self.answered(editor, context);
        let EditorAction::Edit(operator, target) = editor else {
            return self.apply(editor, context);
        };
        let operator = context.resolve(operator);
        if let EditAction::Indent(change) = &operator {
            return self.reindent(change, target, landing, context);
        }
        let Some(landing) = landing else {
            return self.apply(editor, context);
        };
        if EditAction::Motion == operator {
            let at = self.placed(landing.at);
            self.text.set_leader(self.group, at);

            return Ok(());
        }
        let retargeted = self.retargeted(operator, landing);

        self.apply(&retargeted, context)
    }

    /// Runs one of vim's shift operators over the whole logical lines its target spans.
    ///
    /// A shift is linewise whatever the keys in front of it were: `>gj` carries the logical lines
    /// the screen motion crossed rather than the row it stopped on, and `v>` carries the lines a
    /// characterwise selection touched rather than the characters in it. The lines a target spans
    /// are therefore all the seam needs from it, which is why nothing is asked of modalkit here
    /// beyond where the cursor stands: `EditBuffer::indent` is a stub that leaves the text as it
    /// was, and running it would leave the buffer byte for byte the same and the keystroke
    /// silently undone.
    ///
    /// A count in front of the operator is not the count behind it. `3>>` shifts three lines by
    /// one step and `3>` in visual mode shifts the selection by three, which is the difference
    /// between the count the target carries and the count the change carries.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Engine::columns`]'s return values on failure.
    /// * Forwards [`Engine::spanned`]'s return values on failure.
    /// * Forwards [`Engine::apply`]'s return values on failure.
    fn reindent(
        &mut self,
        change: &IndentChange,
        target: &EditTarget,
        landing: Option<Landing>,
        context: &EditContext,
    ) -> Result<(), Error> {
        let columns = self.columns(change, context)?;
        let span = match landing {
            Some(landing) if !landing.complete => {
                let to = self.placed(landing.at);
                let carried = self.against(EditAction::Motion, to, false);

                return self.apply(&carried, context);
            }
            Some(landing) => Some(self.crossed(&landing)),
            None => self.spanned(target, context)?,
        };
        let Some((first, last)) = span else {
            return Ok(());
        };

        self.written(first, last, columns)
    }

    /// # Returns
    ///
    /// The columns a shift carries each of its lines by, which is negative for an outdent.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Unindentable`] if the change is vim's automatic reindent, which decides an
    ///   indent from the text around it rather than from a count and nothing here does.
    fn columns(&self, change: &IndentChange, context: &EditContext) -> Result<isize, Error> {
        let (steps, increasing) = match change {
            IndentChange::Increase(count) => (context.resolve(count), true),
            IndentChange::Decrease(count) => (context.resolve(count), false),
            IndentChange::Auto => {
                return Err(Error::Unindentable {
                    action: format!("{change:?}"),
                });
            }
        };
        let columns =
            isize::try_from(steps.saturating_mul(self.shift.step())).unwrap_or(isize::MAX);

        Ok(if increasing { columns } else { -columns })
    }

    /// # Returns
    ///
    /// The first and last logical lines a screen motion's answer crosses. A screen motion stops on
    /// a row rather than on a line, and an exclusive one stopping in the first column of a line
    /// stops short of that line altogether, which is the rule that decides whether `>4gj` out of a
    /// line taking three rows carries the line below it or leaves it where it was.
    fn crossed(&mut self, landing: &Landing) -> (usize, usize) {
        let cursor = self.text.get_leader(self.group);
        let to = self.placed(landing.at);
        let (near, far) = if to < cursor {
            (to, cursor)
        } else {
            (cursor, to)
        };
        if !landing.inclusive && 0 == far.x && near.y < far.y {
            return (near.y, far.y - 1);
        }

        (near.y, far.y)
    }

    /// # Returns
    ///
    /// * The first and last logical lines a shift's target spans.
    /// * `None` where the target ran out of text, which vim answers by leaving the operator undone
    ///   rather than by shifting the lines it did reach.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Unindentable`] if the target is one this seam does not turn into whole lines,
    ///   which is refused rather than shifted over a guess at the lines it covers. A blockwise
    ///   selection is one of them: vim lays its shift down at the block's own left column rather
    ///   than at the start of the lines the block spans, which is a different operation from the
    ///   one this module runs and not the linewise shift the lines would suggest.
    fn spanned(
        &mut self,
        target: &EditTarget,
        context: &EditContext,
    ) -> Result<Option<(usize, usize)>, Error> {
        let cursor = self.text.get_leader(self.group);
        let last = self.text.get_lines().saturating_sub(1);

        match target {
            EditTarget::Range(RangeType::Line, _, count) => {
                let lines = context.resolve(count);
                if cursor.y >= last && lines > 1 {
                    return Ok(None);
                }

                Ok(Some((
                    cursor.y,
                    last.min(cursor.y + lines.saturating_sub(1)),
                )))
            }
            EditTarget::Motion(MoveType::Line(MoveDir1D::Next), count) => {
                if cursor.y >= last {
                    return Ok(None);
                }

                Ok(Some((
                    cursor.y,
                    last.min(cursor.y + context.resolve(count)),
                )))
            }
            EditTarget::Motion(MoveType::Line(MoveDir1D::Previous), count) => {
                if 0 == cursor.y {
                    return Ok(None);
                }

                Ok(Some((
                    cursor.y.saturating_sub(context.resolve(count)),
                    cursor.y,
                )))
            }
            EditTarget::Motion(MoveType::BufferPos(MovePosition::Beginning), _) => {
                Ok(Some(sorted(cursor.y, 0)))
            }
            EditTarget::Motion(MoveType::BufferPos(MovePosition::End), _) => {
                Ok(Some(sorted(cursor.y, last)))
            }
            EditTarget::Motion(MoveType::BufferLineOffset, count) => {
                let line = last.min(context.resolve(count).saturating_sub(1));

                Ok(Some(sorted(cursor.y, line)))
            }
            EditTarget::Selection => {
                let Some((one, other, shape)) = self.text.get_leader_selection(self.group) else {
                    return Ok(Some((cursor.y, cursor.y)));
                };
                if TargetShape::BlockWise == shape {
                    return Err(Error::Unindentable {
                        action: format!("{shape:?} {target:?}"),
                    });
                }

                Ok(Some(sorted(one.y, other.y)))
            }
            target => Err(Error::Unindentable {
                action: format!("{target:?}"),
            }),
        }
    }

    /// Writes the lines from `first` to `last` out with their indents carried `columns` columns,
    /// and leaves the cursor where vim leaves it: on the first non-blank of the first line the
    /// shift covered.
    ///
    /// Each line's indent is spliced rather than the whole text replaced. `EditBuffer::set_text`
    /// would be the shorter way to write the lines out and is the wrong one: it reinitializes the
    /// buffer's undo history, so a shift would leave `u` with nothing to undo and would take every
    /// edit made before it out of reach as well. Undo is not a stub the way `indent` is -- the
    /// keybindings issue a checkpoint after every edit and the buffer runs it -- so the shift is
    /// written through the same insert and delete the rest of the buffer's edits go through, and
    /// the checkpoint that follows the keystroke makes the whole of it one change.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Engine::apply`]'s return values on failure.
    fn written(&mut self, first: usize, last: usize, columns: isize) -> Result<(), Error> {
        for line in first..=last {
            let held = self.line(line);
            let Some(shifted) = self.shift.shifted(&held, columns) else {
                continue;
            };
            if shifted == held {
                continue;
            }
            self.respelled(line, indent_of(&held).chars().count(), indent_of(&shifted))?;
        }

        let followers = self.text.get_followers(self.group);
        let resting = Cursor::new(first, resting_column(&self.line(first)));
        let members = followers.into_iter().map(CursorState::Location).collect();
        self.text.set_group(
            self.group,
            CursorGroup::new(CursorState::Location(resting), members),
        );

        Ok(())
    }

    /// Replaces the `held` characters `line` begins with by the blanks `laid`, which is one line's
    /// share of a shift.
    ///
    /// The new indent goes in first and the old one is taken away after it. Neither half is
    /// allowed near a register: vim's own `>>` leaves every register holding what it held, so the
    /// delete is spelled against the black hole rather than the unnamed register a delete reaches
    /// for by default.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Engine::apply`]'s return values on failure.
    fn respelled(&mut self, line: usize, held: usize, laid: &str) -> Result<(), Error> {
        let context = EditContextBuilder::default()
            .register(Some(Slot::Blackhole))
            .build();
        if !laid.is_empty() {
            self.text.set_leader(self.group, Cursor::new(line, 0));
            let written = EditorAction::InsertText(InsertTextAction::Transcribe(
                laid.to_owned(),
                MoveDir1D::Previous,
                Count::Exact(1),
            ));
            self.apply(&written, &context)?;
        }
        if 0 < held {
            self.text
                .set_leader(self.group, Cursor::new(line, laid.chars().count()));
            let taken = EditorAction::Edit(
                Specifier::Exact(EditAction::Delete),
                EditTarget::Motion(MoveType::Column(MoveDir1D::Next, false), Count::Exact(held)),
            );
            self.apply(&taken, &context)?;
        }

        Ok(())
    }

    /// Puts the scratch mark where a screen motion's answer says the motion goes, and the cursor
    /// at the near end of what an operator applied over it takes.
    ///
    /// The rules deciding what an operator takes between two places are vim's rather than
    /// modalkit's: `g$`, and any motion behind a `$`, takes the grapheme it stops on where the
    /// others stop in front of theirs; an exclusive motion ending in the first column of a line
    /// takes to the end of the line above instead; and a delete reaching from an indent to the end
    /// of a later line takes whole lines. A motion that ran out of text leaves the operator undone
    /// and carries the cursor alone, as vim does.
    ///
    /// # Returns
    ///
    /// The action running `operator` over the answer, which is the operator asked for over a
    /// target modalkit reads off the mark rather than measures.
    fn retargeted(&mut self, operator: EditAction, landing: Landing) -> EditorAction {
        let cursor = self.text.get_leader(self.group);
        let to = self.placed(landing.at);
        if !landing.complete {
            return self.against(EditAction::Motion, to, false);
        }

        let (near, mut far) = if to < cursor {
            (to, cursor)
        } else {
            (cursor, to)
        };
        if landing.inclusive {
            far = self.past(far);
        } else if 0 == far.x && near.y < far.y {
            let above = far.y - 1;
            if in_indent(&self.line(near.y), near.x) {
                self.text.set_leader(self.group, near);

                return self.against(operator, Cursor::new(above, 0), true);
            }
            far = Cursor::new(above, self.line(above).chars().count());
        }
        let linewise = EditAction::Delete == operator
            && near.y < far.y
            && blank_from(&self.line(far.y), far.x)
            && in_indent(&self.line(near.y), near.x);
        self.text.set_leader(self.group, near);

        self.against(operator, far, linewise)
    }

    /// # Returns
    ///
    /// The cursor one grapheme past `cursor`, which is where an operator taking the grapheme the
    /// cursor stands on has to be carried to for a target that stops in front of its mark.
    fn past(&self, cursor: Cursor) -> Cursor {
        let line = self.line(cursor.y);
        let grapheme = grapheme_offset(&line, cursor.x);

        Cursor::new(cursor.y, char_offset(&line, grapheme + 1))
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
    fn placed(&self, at: LogicalPosition) -> Cursor {
        Cursor::new(at.line, char_offset(&self.line(at.line), at.grapheme))
    }

    /// # Returns
    ///
    /// The text of the logical line at `line` without its line ending, which is empty past the
    /// last line of the text.
    fn line(&self, line: usize) -> String {
        self.text.get().line(line).unwrap_or_default().into_owned()
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

    /// Offers an action to the shim and reads back what the layout engine makes of it.
    ///
    /// # Returns
    ///
    /// Where the action goes, and [`None`] for an action the shim does not answer, which is every
    /// action of an engine built without one.
    fn answered(&mut self, editor: &EditorAction, context: &EditContext) -> Option<Landing> {
        let shim = self.shim.as_mut()?;
        let Some((Classification::Intercepted(motion), count)) = classified(editor) else {
            shim.note(editor);

            return None;
        };
        let cursor = self.text.get_leader(self.group);
        let text = self.text.get();
        let at = LogicalPosition {
            line: cursor.y,
            grapheme: grapheme_offset(&text.line(cursor.y).unwrap_or_default(), cursor.x),
        };
        let landing = shim.intercept(motion, context.resolve(&count), at, text);
        if !bare(editor, context) {
            shim.note(editor);
        }

        landing
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
            shift: Shift::default().with_tab_stop(geometry.metrics().tab_stop()),
        }
    }
}

impl Text for EditRope {
    fn line_count(&self) -> usize {
        self.get_lines().max(1)
    }

    fn line(&self, index: usize) -> Option<Cow<'_, str>> {
        self.get_line(index).map(|line| {
            let held = line.to_string();

            Cow::Owned(held.strip_suffix('\n').unwrap_or(&held).to_owned())
        })
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

    /// An indenting command whose lines this seam does not work out.
    Unindentable {
        /// The change or target nothing here shifts by.
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
            Self::Unindentable { action } => write!(
                formatter,
                "`{action}` is an indenting command whose lines this editor does not work out, so \
                 it is refused rather than shifted over a guess at the lines it covers"
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

/// # Returns
///
/// Whether `action` is a motion with no operator applied to it, which is the only shape of action
/// a chain of screen motions carries its column across. An operator applied to the same motion
/// leaves the cursor at the near end of what it took, which ends the chain as any other edit does.
fn bare(action: &EditorAction, context: &EditContext) -> bool {
    match action {
        EditorAction::Edit(operation, _) => EditAction::Motion == context.resolve(operation),
        _ => false,
    }
}

/// # Returns
///
/// The number of characters of `line` in front of the grapheme at `grapheme`, which is the column
/// modalkit keeps a cursor in.
fn char_offset(line: &str, grapheme: usize) -> usize {
    graphemes(line)
        .take(grapheme)
        .map(|cluster| cluster.chars().count())
        .sum()
}

/// # Returns
///
/// Whether `line` holds nothing but blanks from its character `column` onwards, which is where vim
/// turns a delete spanning more than one line into one over whole lines.
fn blank_from(line: &str, column: usize) -> bool {
    line.chars()
        .skip(column)
        .all(|held| matches!(held, ' ' | '\t'))
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
/// The two lines in the order a span holds them, nearest first.
fn sorted(one: usize, other: usize) -> (usize, usize) {
    (one.min(other), one.max(other))
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
