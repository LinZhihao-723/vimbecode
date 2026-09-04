//! What a reader may do to a transcript, and what becomes of the keys that ask for more.
//!
//! The chat panel is the editor's own machinery pointed at what was said, and it shares all of it:
//! the same keybinding table, the same engine, the same motions, registers and selections. What it
//! does not share is the permission to write. A transcript is the record of an exchange that
//! already happened, and an editor that let `x` take a character out of it would be an editor
//! whose transcript no longer says what was said.
//!
//! The refusal is decided over the actions a keystroke asks for rather than over the keys
//! themselves, because the keys do not say what they do. `d` on its own asks for nothing at all,
//! `dgj` asks for a delete over a motion counted in screen lines, and a table that has been
//! rebound spells either of them with other keys entirely. So a keystroke is read the whole way
//! through the keybinding machine before any of it is run, and only one whose every action leaves
//! the text as it stands is typed at the engine. One that would write never reaches the engine at
//! all, which is what leaves the transcript byte-identical rather than merely restored to what it
//! held.
//!
//! Reading a keystroke before running it costs a second copy of the keybinding machine, kept in
//! step with the engine's by feeding it the very same keys. The keys of a sequence still being
//! typed are held back until the sequence completes, and a completed sequence either reaches the
//! engine whole or is dropped whole and the machine wound back to where the engine still stands.
//!
//! Two things are refused rather than one. An action that writes is the obvious one. The other is
//! the mode: `i` asks for nothing but a cursor split and a mode to stand in, so a panel reading
//! actions alone would grant it and then sit in insert mode refusing every character typed into
//! it. vim refuses `i` outright on a buffer that is `'nomodifiable'`, and so does this.
//!
//! What is left over is everything a reader came for. Every motion, the ones counted in screen
//! lines among them, every character search, every yank into every register and every visual
//! selection reaches the engine exactly as it would in a buffer that could be written to, because
//! none of them writes.
//!
//! What the panel is over is the blocks that were said rather than a string of them. The engine is
//! laid out over the transcript flattened -- a closed fold written as the one row of its summary,
//! every other block as its own source -- so vim's own motions move through the folded transcript
//! and a place in that text is a block and a byte of that block's source. That is the coordinate
//! `iac`, `yac` and `za` are already addressed in, which is what lets the keys the one table binds
//! them to be run here against the transcript itself rather than against a picture of it. A yank
//! taken that way fills the registers a plain yank fills, so there is one store to read. That store
//! is the file editor's own wherever the two were handed the same register file, because a reader
//! who yanks a code block out of an answer means to paste it into a file.
//!
//! Both keybinding machines read through the same table, the engine's included. The panel replays
//! at the engine the keys it held back while a sequence completed, so a table that answered `z`
//! here and nothing there would hand the engine a `z` it drops and a `dw` it runs, which is a
//! delete no policy ever saw.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::mem;
use std::num::NonZeroUsize;

use modalkit::actions::Action;
use modalkit::editing::context::EditContext;
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use vbc_layout::anchor::Wrapping;
use vbc_layout::position::LogicalPosition;

use crate::chat::block::RenderedRow;
use crate::chat::dispatch::{self, Command, Flattened};
use crate::chat::fold::{
    Command as Fold, Fold as Folded, Folds, Position as Placed, Row, Summary, Tag, View,
};
use crate::chat::object::{Kind as ObjectKind, Object, Position};
use crate::chat::selection::{Mode, Selection, Source};
use crate::chat::transcript::Transcript;
use crate::chat::yank::{self, Structure, Yank};
use crate::engine::{Engine, Error, Held, Position as Caret, Registers, Shape};
use crate::event::KeyEvent;
use crate::keys::Keys;
use crate::screen::Geometry;

/// What the status line says about a keystroke the panel would not run.
pub const REFUSAL: &str = "the transcript is read-only";

/// The window a panel measures its screen motions in where it was not told what window it is
/// being drawn in, which is the terminal every vim manual draws its examples in.
const DEFAULT_COLUMNS: usize = 80;
const DEFAULT_ROWS: usize = 24;

/// What a panel lets the keystrokes typed at it do.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Policy {
    /// Nothing runs that could leave the transcript different from the way it arrived, and nothing
    /// puts the panel in a mode whose purpose is to write.
    #[default]
    ReadOnly,

    /// Everything a keystroke asks for runs. This is not a policy the chat panel is used under: it
    /// is here so that a refusal can be shown to be [`Policy::ReadOnly`]'s doing rather than a
    /// keystroke that happened to do nothing, by typing the same keys under it and watching the
    /// transcript change. What a panel under it draws is what the keys left, and which block a
    /// byte of that belongs to is no longer answerable, because an edit moves the text out from
    /// under the map the blocks were written into it by.
    Unrestricted,
}

impl Policy {
    /// # Returns
    ///
    /// Whether a keystroke asking for `actions` and leaving the keys standing in `mode` may reach
    /// the text.
    #[must_use]
    pub fn allows(self, actions: &[(Action, EditContext)], mode: VimMode) -> bool {
        if Self::Unrestricted == self {
            return true;
        }

        VimMode::Insert != mode
            && actions
                .iter()
                .all(|(action, context)| readable(action, context))
    }
}

/// A keystroke a policy would not let reach the text, spelled the way it was typed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    keys: String,
}

impl Refusal {
    /// # Returns
    ///
    /// The keys that were refused, spelled as a vim manual spells them.
    #[must_use]
    pub fn keys(&self) -> &str {
        &self.keys
    }
}

impl Display for Refusal {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(
            formatter,
            "{REFUSAL}: `{}` would change what was said",
            self.keys
        )
    }
}

/// One row of the panel as it is drawn: the row a closed fold is collapsed to, or a row of a
/// block drawn from that block's own source.
///
/// This is [`crate::chat::fold::Row`] with the summary carried rather than borrowed, because a
/// panel owns the transcript its rows are drawn from and a row that borrowed the view it came out
/// of could not leave the call that built it. A frame carries as many summaries as it has rows,
/// so what that copies is the window rather than the transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Drawn {
    /// The one row a closed fold is drawn in.
    Summary(Summary),

    /// A row of a block.
    Body {
        /// The index of the block the row was drawn from.
        block: usize,

        /// The row itself, naming the bytes of that block it shows.
        row: RenderedRow,
    },
}

/// What the selection the keys typed so far are making covers: the block it falls in, and the
/// selection over that block's own source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selected {
    block: usize,
    selection: Selection,
}

impl Selected {
    /// # Returns
    ///
    /// The index of the block the selection falls in.
    #[must_use]
    pub fn block(&self) -> usize {
        self.block
    }

    /// # Returns
    ///
    /// The selection over that block's source.
    #[must_use]
    pub fn selection(&self) -> &Selection {
        &self.selection
    }
}

/// A transcript panel: the blocks that were said, the folds over them, a vim engine over what
/// those folds leave drawn, and the policy deciding which of the keystrokes typed at it reach it.
///
/// The panel holds a [`Transcript`] rather than a string, which is what makes `yac` take the code
/// of the block the cursor is in rather than the rows the cursor is drawn among. What the engine
/// is laid out over is that transcript flattened: a closed fold written as the one row of its
/// summary and every other block as its own source, so a motion is over the folded transcript and
/// `j` crosses a fold in one step. Nothing here writes to the transcript at all -- a fold is a
/// state rather than an edit -- so what a refused keystroke leaves byte-identical is the blocks
/// themselves and not merely a rendering of them.
///
/// Keys go in one at a time as they do at an engine, and a key that only carries a sequence
/// further -- a count, a register, an operator waiting for its target -- is held back until the
/// sequence it belongs to completes, because what a sequence does is not known until then. A
/// sequence the table answers with a command over the transcript's own structure is run here and
/// never handed to the engine, whose own table has no entry for it; every other sequence is typed
/// at the engine exactly as it was typed here.
pub struct Panel {
    transcript: Transcript,
    tags: Vec<Tag>,
    folds: Folds,
    flattened: Flattened,
    engine: Engine,
    geometry: Geometry,
    policy: Policy,
    keys: Keys,
    agreed: Keys,
    held: Vec<KeyEvent>,
    resting: Position,
    refusal: Option<Refusal>,
    notice: Option<String>,
}

impl Panel {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created read-only panel showing `transcript` with every fold of it closed, the
    /// cursor on its first character, measuring the screen motions typed at it in the window a vim
    /// manual draws its examples in.
    ///
    /// # Panics
    ///
    /// Panics if the default window is zero columns wide or zero rows tall, which it is not.
    #[must_use]
    pub fn new(transcript: Transcript) -> Self {
        let columns = NonZeroUsize::new(DEFAULT_COLUMNS).expect("the default columns are not zero");
        let rows = NonZeroUsize::new(DEFAULT_ROWS).expect("the default rows are not zero");

        Self::laid_out_in(transcript, Geometry::new(columns, rows))
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created panel like [`Panel::new`]'s, measuring the screen motions typed at it in
    /// `geometry` and wrapping its rows the way `geometry` says.
    #[must_use]
    pub fn laid_out_in(transcript: Transcript, geometry: Geometry) -> Self {
        let tags = vec![Tag::untagged(); transcript.len()];

        Self::over(transcript, tags, geometry)
    }

    /// # Returns
    ///
    /// The same panel over blocks that arrived tagged with the calls they were said beneath, so
    /// that the folds nest the way those calls did.
    #[must_use]
    pub fn tagged(mut self, tags: Vec<Tag>) -> Self {
        self.tags = tags;
        self.folds.rebuild(&self.transcript, &self.tags);
        self.reflow(0);

        self
    }

    /// # Returns
    ///
    /// The same panel governed by `policy` rather than by [`Policy::ReadOnly`].
    #[must_use]
    pub fn governed_by(mut self, policy: Policy) -> Self {
        self.policy = policy;

        self
    }

    /// # Returns
    ///
    /// The same panel yanking into `registers` rather than into a file of its own, so that `yac`
    /// here is what `p` puts in the file editor handed the same registers.
    #[must_use]
    pub fn sharing(mut self, registers: Registers) -> Self {
        self.engine = self.engine.sharing(registers);

        self
    }

    /// # Returns
    ///
    /// The policy the panel refuses keystrokes by.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Types one key at the panel, running everything it asks for that the policy allows.
    ///
    /// A keystroke the policy refuses reaches neither the engine nor the text, and what the status
    /// line says about it is what [`Panel::refusal`] answers with until the next key is typed.
    ///
    /// A keystroke the table answers with a command over the transcript's structure is run here.
    /// Such a command asks for nothing that could write -- an object resolves a range, a
    /// structural yank reads one, and a fold is a state held beside the transcript rather than a
    /// change to it -- so the policy has nothing to refuse it by, and the keys are not handed on
    /// to the engine, whose own table binds them to nothing at all.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Engine::press_all`]'s return values on failure.
    pub fn press(&mut self, key: KeyEvent) -> Result<(), Error> {
        self.refusal = None;
        self.notice = None;
        self.held.push(key);
        self.keys.input_key(key.into());
        let mut asked = Vec::new();
        while let Some(popped) = self.keys.pop() {
            asked.push(popped);
        }
        let mut commands = Vec::new();
        while let Some(command) = self.keys.pop_chat() {
            commands.push(command);
        }
        if !self.policy.allows(&asked, self.keys.mode()) {
            self.keys = self.agreed.clone();
            self.refusal = Some(Refusal {
                keys: spelled(&mem::take(&mut self.held)),
            });

            return Ok(());
        }
        if !commands.is_empty() {
            self.held.clear();
            self.agreed = self.keys.clone();
            for command in commands {
                self.run(command);
            }
            self.adopt();

            return Ok(());
        }
        if asked.is_empty() {
            return Ok(());
        }
        let typed = mem::take(&mut self.held);
        let ran = self.engine.press_all(typed);
        self.agreed = self.keys.clone();
        self.adopt();

        ran
    }

    /// Types a sequence of keys at the panel, one key at a time.
    ///
    /// # Type Parameters
    ///
    /// * `KeysType` - The keys to type, in the order they are typed.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Panel::press`]'s return values on failure.
    pub fn press_all<KeysType: IntoIterator<Item = KeyEvent>>(
        &mut self,
        keys: KeysType,
    ) -> Result<(), Error> {
        for key in keys {
            self.press(key)?;
        }

        Ok(())
    }

    /// Lays the panel out in `geometry` instead, which is what a resized terminal asks of it.
    pub fn resize(&mut self, geometry: Geometry) {
        self.engine.resize(geometry.clone());
        self.geometry = geometry;
    }

    /// # Returns
    ///
    /// The blocks that were said, which no keystroke typed at the panel changes.
    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// # Returns
    ///
    /// The folds over those blocks and which of them are open.
    #[must_use]
    pub fn folds(&self) -> &Folds {
        &self.folds
    }

    /// # Returns
    ///
    /// The transcript as the folds leave it drawn, which is the text the engine is laid out over
    /// and ends in a newline as vim's own text does.
    #[must_use]
    pub fn text(&self) -> String {
        self.engine.text()
    }

    /// Draws `rows` rows of the folded transcript from `from` downward.
    ///
    /// Each block is asked for the window of it that falls in those rows and for nothing below,
    /// so a panel scrolled to the bottom of what a tool wrote costs the screen rather than the
    /// scroll.
    ///
    /// # Returns
    ///
    /// The rows drawn, top to bottom, which are fewer than were asked for where the transcript
    /// ends inside them.
    #[must_use]
    pub fn rows(&self, from: Placed, rows: usize) -> Vec<Drawn> {
        let view = View::of(&self.folds, &self.transcript);
        let wrapping = self.wrapping();

        view.render(from, rows, &wrapping)
            .into_iter()
            .map(|row| match row {
                Row::Summary(summary) => Drawn::Summary(summary.clone()),
                Row::Body { block, row } => Drawn::Body { block, row },
            })
            .collect()
    }

    /// # Returns
    ///
    /// The row of the folded transcript below `at`, or [`None`] where `at` is its last row. A
    /// closed fold is one row, so a step over one costs nothing however much it hides.
    #[must_use]
    pub fn below(&self, at: Placed) -> Option<Placed> {
        View::of(&self.folds, &self.transcript).down(at, &self.wrapping())
    }

    /// # Returns
    ///
    /// The row of the folded transcript above `at`, or [`None`] where `at` is its first row.
    #[must_use]
    pub fn above(&self, at: Placed) -> Option<Placed> {
        View::of(&self.folds, &self.transcript).up(at, &self.wrapping())
    }

    /// # Returns
    ///
    /// Where the cursor rests in the text the folds leave drawn.
    pub fn cursor(&mut self) -> Caret {
        self.engine.cursor()
    }

    /// # Returns
    ///
    /// The block the cursor rests in and the byte of that block's source it rests on, which is
    /// the start of the block a closed fold heads wherever the cursor rests on that fold's row.
    #[must_use]
    pub fn at(&self) -> Position {
        self.resting
    }

    /// # Returns
    ///
    /// What the selection the keys typed so far are making covers, or [`None`] where they are
    /// making none. The selection is over the source of the block it was started in, so what it
    /// covers is the logical lines of what was said rather than the rows they were drawn in.
    pub fn selection(&mut self) -> Option<Selected> {
        let (cursor, anchor, shape) = self.engine.selection()?;
        let origin = self.flattened.caret_at(anchor);
        let moving = self.flattened.caret_at(cursor);
        let block = self.flattened.at(origin)?.block();
        let start = self.flattened.offset_of(block, 0)?;
        let source = self.transcript.block(block)?;
        let over = Source::new(source.source(), self.geometry.metrics());
        let selection = Selection::between(
            selected(shape),
            over,
            origin.saturating_sub(start),
            moving.saturating_sub(start),
        );

        Some(Selected { block, selection })
    }

    /// # Returns
    ///
    /// The mode the panel is in, which is the mode the keys typed at it left the keybinding
    /// machine in rather than the mode a refused keystroke asked for.
    #[must_use]
    pub fn mode(&self) -> VimMode {
        self.keys.mode()
    }

    /// # Returns
    ///
    /// What every register holding text holds, keyed by the name it is addressed by. A structural
    /// yank fills the same registers a plain one does, so there is one store to read rather than
    /// one for each way of taking something out.
    #[must_use]
    pub fn registers(&self) -> BTreeMap<char, Held> {
        self.engine.registers()
    }

    /// # Returns
    ///
    /// What the register named `name` holds, or [`None`] where it holds nothing. The clipboard's
    /// register is one a structural yank fills and one [`Panel::registers`] does not answer with,
    /// because that is the set a run is compared against vim's own by.
    #[must_use]
    pub fn register(&self, name: char) -> Option<Held> {
        self.engine.register(name)
    }

    /// # Returns
    ///
    /// The keystroke the policy last refused, whose [`Display`] is what the status line says, and
    /// [`None`] where the last key typed was one the panel ran.
    #[must_use]
    pub fn refusal(&self) -> Option<&Refusal> {
        self.refusal.as_ref()
    }

    /// # Returns
    ///
    /// What the last keystroke asked the transcript for and could not be given, and [`None`]
    /// where it was given what it asked for.
    #[must_use]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// # Returns
    ///
    /// A newly created read-only panel over `transcript`, reading the keys typed at its engine a
    /// second time so that what a keystroke asks for is known before any of it is run.
    ///
    /// Both machines read through the very same table, because the panel replays at the engine
    /// the keys it held back while a sequence completed: a table that answered `z` here and
    /// nothing there would hand the engine a `z` it drops and a `dw` it runs, which is a delete
    /// nothing refused.
    fn over(transcript: Transcript, tags: Vec<Tag>, geometry: Geometry) -> Self {
        let folds = Folds::of(&transcript, &tags);
        let flattened = {
            let view = View::of(&folds, &transcript);

            Flattened::of(&view, &transcript)
        };
        let keys = Keys::new(dispatch::bindings());
        let engine =
            Engine::laid_out_in(flattened.text(), geometry.clone()).bound_by(dispatch::bindings());

        Self {
            transcript,
            tags,
            folds,
            flattened,
            engine,
            geometry,
            policy: Policy::default(),
            agreed: keys.clone(),
            keys,
            held: Vec::new(),
            resting: Position::new(0, 0),
            refusal: None,
            notice: None,
        }
    }

    /// Reads back which block the cursor now rests in and which byte of its source, which is what
    /// every command over the transcript's structure is resolved at.
    fn adopt(&mut self) {
        let caret = self.engine.cursor();
        let offset = self.flattened.caret_at(caret);
        self.resting = self
            .flattened
            .at(offset)
            .unwrap_or_else(|| Position::new(0, 0));
    }

    /// Runs one command over the transcript's own structure.
    fn run(&mut self, command: Command) {
        match command {
            Command::Object(object) => self.select(object),
            Command::Yank(structure) => self.take(structure),
            Command::Fold(fold) => self.fold(fold),
        }
    }

    /// Takes the object `object` at the cursor as the selection the keys are making.
    ///
    /// The object is resolved in the transcript's own coordinates and carried back into the text
    /// the engine is laid out over, so what is selected is the block's source and what is
    /// highlighted is wherever that source is drawn.
    fn select(&mut self, object: Object) {
        let at = self.resting;
        let Some(region) = object.resolve(&self.transcript, at) else {
            self.notice = Some(format!("there is no {} here", spoken(object.kind())));

            return;
        };
        let range = region.range().clone();
        let (Some(start), Some(end)) = (
            self.flattened.offset_of(region.block(), range.start),
            self.flattened
                .offset_of(region.block(), range.end.saturating_sub(1)),
        ) else {
            self.notice = Some(format!("the {} here is folded away", spoken(object.kind())));

            return;
        };
        self.engine.select(
            self.flattened.caret_of(end),
            self.flattened.caret_of(start),
            Shape::Charwise,
        );
    }

    /// Takes what the cursor is in out of the transcript whole, into the registers a plain yank
    /// fills.
    fn take(&mut self, structure: Structure) {
        let at = self.resting;
        let Some(yank) = Yank::structural(&self.transcript, at, structure) else {
            self.notice = Some(format!("there is no {} here", taken(structure)));

            return;
        };
        yank::file(self.engine.register_file(), &yank);
    }

    /// Opens or closes the folds at the cursor and lays the engine out over what they now leave
    /// drawn.
    fn fold(&mut self, command: Fold) {
        let at = self.resting.block();
        self.folds.apply(command, at);
        self.reflow(at);
    }

    /// Writes the folded transcript out again and hands it to the engine, resting the cursor at
    /// the top of the entry that now draws the block `at`, which is the row of the fold that
    /// covers it where a fold closed over it.
    fn reflow(&mut self, at: usize) {
        let flattened = {
            let view = View::of(&self.folds, &self.transcript);

            Flattened::of(&view, &self.transcript)
        };
        let resting = flattened.start_of(at).or_else(|| {
            self.folds
                .covering(at)
                .filter(|fold| !self.folds.is_open(fold.head()))
                .map(Folded::head)
                .find_map(|head| flattened.start_of(head))
        });
        let caret = flattened.caret_of(resting.unwrap_or(0));
        self.engine.reload(flattened.text());
        self.engine.place(LogicalPosition {
            line: caret.line,
            grapheme: 0,
        });
        self.flattened = flattened;
        self.adopt();
    }

    /// # Returns
    ///
    /// How the panel wraps the rows it draws.
    fn wrapping(&self) -> Wrapping {
        Wrapping::new(
            self.geometry.columns(),
            self.geometry.metrics(),
            self.geometry.options().clone(),
        )
    }
}

/// # Returns
///
/// The shape a selection of `shape` takes over the source of a block.
fn selected(shape: Shape) -> Mode {
    match shape {
        Shape::Charwise => Mode::Charwise,
        Shape::Linewise => Mode::Linewise,
        Shape::Blockwise => Mode::Blockwise,
    }
}

/// # Returns
///
/// What a notice calls the thing a text object addresses.
fn spoken(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Code => "code block",
        ObjectKind::Message => "message",
        ObjectKind::ToolResult => "tool result",
    }
}

/// # Returns
///
/// What a notice calls the thing a structural yank takes.
fn taken(structure: Structure) -> &'static str {
    match structure {
        Structure::Code => "code block",
        Structure::Message => "message",
        Structure::ToolResult => "tool result",
        Structure::Diff => "diff",
    }
}

/// # Returns
///
/// Whether `action`, run under `context`, leaves the text as it stands. Moving a reader about the
/// text, searching it, scrolling it, redrawing it and saying something about it are all as
/// harmless as a motion, and an editing action is harmless where modalkit says it is. Everything
/// else is not, this module's failure to recognize an action included: a panel that guesses is a
/// panel that eventually guesses wrong, and never writing is the whole of what it promises.
fn readable(action: &Action, context: &EditContext) -> bool {
    match action {
        Action::NoOp
        | Action::Jump(..)
        | Action::KeywordLookup(_)
        | Action::RedrawScreen
        | Action::Scroll(_)
        | Action::Search(..)
        | Action::ShowInfoMessage(_)
        | Action::Suspend => true,
        Action::Editor(editor) => editor.is_readonly(context),
        _ => false,
    }
}

/// # Returns
///
/// `keys` spelled the way a vim manual spells a sequence of keystrokes.
fn spelled(keys: &[KeyEvent]) -> String {
    keys.iter()
        .map(|key| TerminalKey::from(*key).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use editor_types::prelude::{
        Count, EditTarget, MoveDir1D, MoveDir2D, MoveDirMod, PasteStyle, PositionList, RepeatType,
        ScrollSize, ScrollStyle, Specifier,
    };
    use modalkit::actions::{
        Action, EditAction, EditorAction, HistoryAction, InsertTextAction, MacroAction,
    };
    use modalkit::editing::context::EditContext;
    use modalkit::env::vim::VimMode;

    use super::Policy;

    #[test]
    fn a_read_only_policy_allows_the_actions_that_leave_the_text_as_it_stands() {
        for action in readable() {
            assert!(
                Policy::ReadOnly
                    .allows(&[(action.clone(), EditContext::default())], VimMode::Normal),
                "`{action:?}` writes nothing and is refused all the same"
            );
        }
    }

    #[test]
    fn a_read_only_policy_refuses_the_actions_that_would_write() {
        for action in writing() {
            assert!(
                !Policy::ReadOnly
                    .allows(&[(action.clone(), EditContext::default())], VimMode::Normal),
                "`{action:?}` would change the text and is allowed all the same"
            );
        }
    }

    #[test]
    fn a_read_only_policy_refuses_an_action_it_cannot_read_as_harmless() {
        for action in unread() {
            assert!(
                !Policy::ReadOnly
                    .allows(&[(action.clone(), EditContext::default())], VimMode::Normal),
                "`{action:?}` is an action nothing here reads, and is allowed all the same"
            );
        }
    }

    #[test]
    fn a_read_only_policy_refuses_a_keystroke_that_would_leave_the_panel_writing() {
        assert!(!Policy::ReadOnly.allows(&[], VimMode::Insert));
        for mode in [
            VimMode::Normal,
            VimMode::Visual,
            VimMode::Select,
            VimMode::OperationPending,
        ] {
            assert!(
                Policy::ReadOnly.allows(&[], mode),
                "a reader cannot stand in `{mode:?}`"
            );
        }
    }

    #[test]
    fn a_read_only_policy_refuses_a_keystroke_where_any_one_of_its_actions_would_write() {
        let mixed = [
            (readable().remove(0), EditContext::default()),
            (writing().remove(0), EditContext::default()),
        ];

        assert!(!Policy::ReadOnly.allows(&mixed, VimMode::Normal));
    }

    #[test]
    fn an_unrestricted_policy_allows_everything_a_read_only_one_refuses() {
        for action in writing().into_iter().chain(unread()) {
            assert!(
                Policy::Unrestricted.allows(&[(action, EditContext::default())], VimMode::Insert)
            );
        }
    }

    /// # Returns
    ///
    /// Actions that leave the text exactly as it stands.
    fn readable() -> Vec<Action> {
        vec![
            Action::NoOp,
            EditorAction::Edit(
                Specifier::Exact(EditAction::Motion),
                EditTarget::CurrentPosition,
            )
            .into(),
            EditorAction::Edit(
                Specifier::Exact(EditAction::Yank),
                EditTarget::CurrentPosition,
            )
            .into(),
            EditorAction::History(HistoryAction::Checkpoint).into(),
            Action::Jump(PositionList::JumpList, MoveDir1D::Next, Count::Exact(1)),
            Action::RedrawScreen,
            Action::Scroll(ScrollStyle::Direction2D(
                MoveDir2D::Up,
                ScrollSize::Cell,
                Count::Exact(1),
            )),
            Action::Search(MoveDirMod::Same, Count::Exact(1)),
        ]
    }

    /// # Returns
    ///
    /// Actions that would leave the text different from the way it arrived.
    fn writing() -> Vec<Action> {
        vec![
            EditorAction::Edit(
                Specifier::Exact(EditAction::Delete),
                EditTarget::CurrentPosition,
            )
            .into(),
            EditorAction::InsertText(InsertTextAction::Paste(
                PasteStyle::Side(MoveDir1D::Next),
                Count::Exact(1),
            ))
            .into(),
            EditorAction::History(HistoryAction::Undo(Count::Exact(1))).into(),
            EditorAction::History(HistoryAction::Redo(Count::Exact(1))).into(),
        ]
    }

    /// # Returns
    ///
    /// Actions this module reads as neither, which a panel that promises never to write cannot
    /// let through on the grounds that it does not recognize them.
    fn unread() -> Vec<Action> {
        vec![
            Action::Macro(MacroAction::Execute(Count::Exact(1))),
            Action::Repeat(RepeatType::EditSequence),
            Action::Repeat(RepeatType::LastAction),
        ]
    }
}
