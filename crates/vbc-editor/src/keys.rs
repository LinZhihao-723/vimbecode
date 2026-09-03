//! The table this editor's keys are bound by, and the machine that reads keystrokes through it.
//!
//! modalkit's editing actions, registers, marks and text objects are the editor's and stay that
//! way. What is not modalkit's any more is the table that turns keystrokes into them. Its own
//! table binds `gUgU` and `gUU` as four- and three-key sequences of normal mode, which makes `gU`
//! a prefix of a longer normal-mode sequence rather than an operator waiting for a motion: `gUgj`
//! walks down that prefix, finds nothing at `gUg`, and abandons the operator, so the keys that
//! ask to upper-case down a display line move the cursor instead. That is the table's shape
//! rather than the actions it names, which is why it is replaced here rather than worked around.
//!
//! So the shape is different. An operator fires the moment it is typed and holds what it is
//! waiting for; every key after it is looked up in the operator-pending table, which is where the
//! motions and the text objects live. A doubled operator is not a sequence of normal mode but the
//! one thing an operator answers itself: typing the operator's own keys again, or its last key
//! alone, runs it over whole lines. `dd`, `guu`, `gUgU` and `>>` all arrive that way, and `gUgj`
//! reaches `gj` in the operator-pending table with the operator still held, which is the whole
//! point of the arrangement.
//!
//! No bound sequence is a prefix of another within the same table, which is what lets the machine
//! fire a match as soon as it completes rather than waiting to see whether a longer one follows.
//! That is a rule about the table rather than a hope about it, and it is checked.
//!
//! The table is data rather than code: which keys a step is reached by, and what a step does, are
//! separate. So the `g` the display motions and the case operators hang off is a character the
//! table is built with rather than one it is written in, and any single binding can be replaced,
//! added or removed before a machine is built from it. What the machine emits is modalkit's
//! [`Action`] together with the [`EditContext`] it runs under, which is what the engine above
//! already runs, so nothing downstream of the seam can tell where the actions came from.
//!
//! What is deliberately not bound: windows, tabs, scrolling, macros, marks, regular-expression
//! search, command mode and select mode, because this editor drives none of them. Nor are the
//! motions and the text objects modalkit's own text cannot answer, which are left unbound rather
//! than bound to a keystroke that reaches the text and quietly changes nothing -- the harder of
//! the two to notice. `iw` and `aw` name one range apiece because modalkit's text draws no
//! distinction between them.

use std::collections::VecDeque;
use std::str::FromStr;

use editor_types::context::{EditContext, EditContextBuilder};
use editor_types::prelude::{
    Case, Char, Count, CursorCloseTarget, CursorEnd, EditTarget, IndentChange, InsertStyle,
    JoinStyle, MoveDir1D, MoveDirMod, MovePosition, MoveType, PasteStyle, RangeType, Register,
    SearchType, SelectionCursorChange, SelectionSplitStyle, Specifier, TargetShape,
    TargetShapeFilter, WordStyle,
};
use modalkit::actions::{
    Action, CursorAction, EditAction, EditorAction, HistoryAction, InsertTextAction,
    SelectionAction,
};
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use modalkit::keybindings::InputKey;

/// The character the display motions and the case operators hang off in vim, and the one the
/// table is built with where a caller asks for no other.
pub const PREFIX: char = 'g';

/// The character a register is named after in vim, which is the one the table reads a register
/// prefix by.
pub const REGISTER_PREFIX: char = '"';

/// One key of a bound sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edge {
    /// The sequence continues only with this key.
    Key(TerminalKey),

    /// The sequence continues with any key, whose character the step is handed.
    Any,
}

/// What a bound key sequence changes about the context its actions run under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Change {
    /// The operator an action resolves [`Specifier::Contextual`] against.
    Operation(EditAction),

    /// Where an edit leaves the cursor.
    CursorEnd(CursorEnd),

    /// The shape a selection takes, where the shape it already has passes the filter.
    Shape(TargetShapeFilter, TargetShape),

    /// The style text is inserted with, which is what says a mode is an inserting one.
    Insert(InsertStyle),

    /// The register an edit reads and writes.
    Register(Register),

    /// The direction a character search runs in and whether it stops on its character, taking the
    /// character to search for from the key the sequence was completed by.
    SearchChar(MoveDir1D, bool),

    /// The character a replace writes, taken from the key the sequence was completed by.
    ReplaceChar,
}

/// One of the actions a bound key sequence produces.
#[derive(Clone, Debug)]
pub enum Emit {
    /// The action, whether or not a count was typed.
    Always(Action),

    /// The action the sequence produces with no count typed, and the one it produces with one.
    Counted(Action, Action),
}

/// What a bound key sequence does.
#[derive(Clone, Debug)]
pub enum Step {
    /// Change the context, produce the actions, and leave the machine in the mode named.
    Run {
        /// What the sequence changes about the context first.
        changes: Vec<Change>,

        /// What the sequence produces.
        emits: Vec<Emit>,

        /// The mode the machine is left in, and [`None`] to leave it where it stands.
        mode: Option<VimMode>,
    },

    /// Hold an operator until a later key names what it runs over.
    Operator {
        /// The operator the motion or text object that follows is run under.
        operation: EditAction,

        /// The mode the machine is left in once it has run.
        mode: VimMode,

        /// The style the text it leaves behind is inserted with, for an operator that ends in an
        /// inserting mode.
        insert: Option<InsertStyle>,

        /// What the operator does when its own keys, or its last key alone, are typed again.
        doubled: Box<Step>,
    },

    /// Start a visual selection of a shape, or end the one that already has that shape.
    Visual(TargetShape),
}

/// One entry of the table: the keys it is reached by, the mode they are read in, and what they do.
#[derive(Clone, Debug)]
pub struct Binding {
    /// The mode the sequence is looked up in.
    pub mode: VimMode,

    /// The keys the sequence is typed by.
    pub keys: Vec<Edge>,

    /// The operator this entry belongs to, for an operator-pending entry only one operator reads
    /// its keys this way in, such as the `w` of `cw`.
    pub operator: Option<Vec<TerminalKey>>,

    /// What the sequence does.
    pub step: Step,
}

/// The table a machine reads its keystrokes through.
#[derive(Clone, Debug)]
pub struct Bindings {
    prefix: char,
    register: TerminalKey,
    entries: Vec<Binding>,
}

impl Bindings {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The editor's own vim table, built around [`PREFIX`].
    ///
    /// # Panics
    ///
    /// Panics if a sequence of the table names a key that cannot be parsed, which is a fault in
    /// the table rather than in what a caller asked for.
    #[must_use]
    pub fn vim() -> Self {
        Self::prefixed(PREFIX)
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// The editor's own vim table, built around `prefix` wherever vim uses `g`, so that the
    /// display motions and the case operators are reached through a character of the caller's
    /// choosing.
    ///
    /// # Panics
    ///
    /// Panics if a sequence of the table names a key that cannot be parsed, which is a fault in
    /// the table rather than in what a caller asked for.
    #[must_use]
    pub fn prefixed(prefix: char) -> Self {
        let mut bindings = Self {
            prefix,
            register: key(REGISTER_PREFIX),
            entries: Vec::new(),
        };
        for (modes, keys, step) in table() {
            for mode in modes {
                bindings.bind(mode, &keys, step.clone());
            }
        }
        for (operator, keys, step) in operator_table() {
            bindings.bind_under(&operator, &keys, step);
        }

        bindings
    }

    /// # Returns
    ///
    /// The character the table's display motions and case operators hang off.
    #[must_use]
    pub fn prefix(&self) -> char {
        self.prefix
    }

    /// # Returns
    ///
    /// Every entry of the table, in the order it was built in.
    #[must_use]
    pub fn entries(&self) -> &[Binding] {
        &self.entries
    }

    /// Binds `keys` in `mode` to `step`, replacing whatever those keys were bound to in it.
    ///
    /// # Panics
    ///
    /// Panics if `keys` names a key that cannot be parsed.
    pub fn bind(&mut self, mode: VimMode, keys: &str, step: Step) {
        let edges = edges(keys, self.prefix);
        self.entries
            .retain(|bound| bound.mode != mode || bound.keys != edges || bound.operator.is_some());
        self.entries.push(Binding {
            mode,
            keys: edges,
            operator: None,
            step,
        });
    }

    /// Binds `keys` in the operator-pending table, for the operator typed by `operator` alone.
    ///
    /// # Panics
    ///
    /// Panics if either sequence names a key that cannot be parsed.
    pub fn bind_under(&mut self, operator: &str, keys: &str, step: Step) {
        let under = keyed(&edges(operator, self.prefix));
        let edges = edges(keys, self.prefix);
        self.entries.retain(|bound| {
            bound.mode != VimMode::OperationPending
                || bound.keys != edges
                || bound.operator.as_ref() != Some(&under)
        });
        self.entries.push(Binding {
            mode: VimMode::OperationPending,
            keys: edges,
            operator: Some(under),
            step,
        });
    }

    /// Removes everything `keys` reach in `mode`, which for the operator-pending table includes
    /// the entries an operator of its own reads those keys by.
    ///
    /// # Panics
    ///
    /// Panics if `keys` names a key that cannot be parsed.
    pub fn unbind(&mut self, mode: VimMode, keys: &str) {
        let edges = edges(keys, self.prefix);
        self.entries
            .retain(|bound| bound.mode != mode || bound.keys != edges);
    }
}

impl Default for Bindings {
    fn default() -> Self {
        Self::vim()
    }
}

/// The keys typed at an editor so far, read through a table.
///
/// A key goes in one at a time and the actions it completes come out one at a time, each with the
/// context it is to run under, which is what the engine above drives modalkit's editing through.
#[derive(Clone, Debug)]
pub struct Keys {
    bindings: Bindings,
    mode: VimMode,
    count: Option<usize>,
    counting: Option<usize>,
    register: Option<Register>,
    register_append: bool,
    operation: EditAction,
    cursor_end: Option<CursorEnd>,
    postmode: Option<VimMode>,
    replace: Option<Char>,
    typed: Option<Char>,
    shape: Option<TargetShape>,
    insert: Option<InsertStyle>,
    charsearch: Option<Char>,
    charsearch_params: (MoveDir1D, bool),
    pending: Vec<TerminalKey>,
    operator: Option<Operator>,
    reading_register: bool,
    queue: VecDeque<(Action, EditContext)>,
}

impl Keys {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created machine in normal mode, reading its keystrokes through `bindings`.
    #[must_use]
    pub fn new(bindings: Bindings) -> Self {
        Self {
            bindings,
            mode: VimMode::Normal,
            count: None,
            counting: None,
            register: None,
            register_append: false,
            operation: EditAction::Motion,
            cursor_end: None,
            postmode: None,
            replace: None,
            typed: None,
            shape: None,
            insert: None,
            charsearch: None,
            charsearch_params: (MoveDir1D::Next, false),
            pending: Vec::new(),
            operator: None,
            reading_register: false,
            queue: VecDeque::new(),
        }
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created machine reading its keystrokes through the editor's own vim table.
    #[must_use]
    pub fn vim() -> Self {
        Self::new(Bindings::vim())
    }

    /// # Returns
    ///
    /// The mode the machine is in. An operator waiting for what it runs over is held beside the
    /// mode rather than as one, which is where vim keeps it too.
    #[must_use]
    pub fn mode(&self) -> VimMode {
        self.mode
    }

    /// Types one key at the machine, queueing whatever it completes.
    pub fn input_key(&mut self, typed: TerminalKey) {
        if self.reading_register {
            self.reading_register = false;
            if let Some((register, append)) = register_of(typed) {
                self.register = Some(register);
                self.register_append = append;
            }

            return;
        }
        if self.pending.is_empty() && self.counts() {
            if let Some(digit) = digit_of(typed) {
                if 0 != digit || self.counting.is_some() {
                    let counting = self.counting.unwrap_or(0);
                    self.counting = Some(counting.saturating_mul(10).saturating_add(digit));

                    return;
                }
            }
        }
        self.save_counting();
        if self.pending.is_empty() && self.registers() && typed == self.bindings.register {
            self.reading_register = true;

            return;
        }
        self.matched(typed);
    }

    /// # Returns
    ///
    /// The next action the keys typed so far completed, together with the context it runs under,
    /// and [`None`] once they are exhausted.
    pub fn pop(&mut self) -> Option<(Action, EditContext)> {
        self.queue.pop_front()
    }

    /// Looks `typed` up in the table, firing what it completes and abandoning what it kills.
    fn matched(&mut self, typed: TerminalKey) {
        self.pending.push(typed);
        if let Some(step) = self.doubled() {
            self.fire(&step, None);

            return;
        }
        let (complete, partial) = self.candidates();
        if let Some((step, edges)) = complete {
            self.fire(&step, Some(&edges));

            return;
        }
        if partial {
            return;
        }
        self.pending.pop();
        if self.pending.is_empty() && self.operator.is_none() {
            self.unmapped(typed);

            return;
        }
        self.abandon();
    }

    /// # Returns
    ///
    /// What the operator waiting for a target does when its own keys, or its last key alone, are
    /// typed again, and [`None`] where the keys typed so far are not that.
    fn doubled(&self) -> Option<Step> {
        let operator = self.operator.as_ref()?;
        let last = operator.keys.last()?;
        if operator.keys == self.pending || [*last] == self.pending.as_slice() {
            return Some(operator.doubled.clone());
        }

        None
    }

    /// # Returns
    ///
    /// What the keys typed so far complete together with the keys it is bound by, and whether a
    /// longer sequence still has them as a prefix.
    fn candidates(&self) -> (Option<(Step, Vec<Edge>)>, bool) {
        let mode = self.lookup();
        let held = self.operator.as_ref().map(|operator| &operator.keys);
        let mut complete: Option<(Step, Vec<Edge>)> = None;
        let mut partial = held.is_some_and(|keys| keys.starts_with(&self.pending));
        for binding in &self.bindings.entries {
            if binding.mode != mode {
                continue;
            }
            if let Some(under) = &binding.operator {
                if held != Some(under) {
                    continue;
                }
            }
            match matched(&binding.keys, &self.pending) {
                Some(true) => {
                    if complete.is_none() || binding.operator.is_some() {
                        complete = Some((binding.step.clone(), binding.keys.clone()));
                    }
                }
                Some(false) => partial = true,
                None => {}
            }
        }

        (complete, partial)
    }

    /// Runs what a completed key sequence asks for, `edges` being the keys it was bound by.
    fn fire(&mut self, step: &Step, edges: Option<&[Edge]>) {
        let keys = std::mem::take(&mut self.pending);
        self.typed = edges
            .and_then(|edges| any_of(edges, &keys))
            .map(Char::Single);
        match step {
            Step::Operator {
                operation,
                mode,
                insert,
                doubled,
            } => {
                self.operation = operation.clone();
                self.postmode = Some(*mode);
                if let Some(style) = insert {
                    self.insert = Some(*style);
                }
                self.operator = Some(Operator {
                    keys,
                    doubled: (**doubled).clone(),
                });
            }
            Step::Visual(shape) => {
                self.operator = None;
                if Some(*shape) == self.shape {
                    self.goto(VimMode::Normal);
                } else {
                    self.shape = Some(*shape);
                    self.emit(
                        &[Emit::Always(
                            EditorAction::Edit(
                                Specifier::Exact(EditAction::Motion),
                                EditTarget::CurrentPosition,
                            )
                            .into(),
                        )],
                        Some(VimMode::Visual),
                    );
                }
            }
            Step::Run {
                changes,
                emits,
                mode,
            } => {
                self.operator = None;
                for change in changes {
                    self.change(change);
                }
                self.emit(emits, *mode);
            }
        }
    }

    /// Queues `emits` with the context they run under and leaves the machine in the mode the
    /// sequence asked for.
    fn emit(&mut self, emits: &[Emit], mode: Option<VimMode>) {
        let counted = self.count.is_some();
        let actions: Vec<Action> = emits
            .iter()
            .map(|emit| match emit {
                Emit::Always(action) => action.clone(),
                Emit::Counted(bare, with_count) => {
                    if counted {
                        with_count.clone()
                    } else {
                        bare.clone()
                    }
                }
            })
            .collect();
        let next = self.postmode.take().or(mode);
        if actions.is_empty() {
            if let Some(mode) = next {
                self.goto(mode);
            }

            return;
        }
        let context = self.take();
        for action in actions {
            self.queue.push_back((action, context.clone()));
        }
        self.goto(next.unwrap_or(self.mode));
    }

    /// Leaves the machine in `mode`, queueing what entering it asks for.
    fn goto(&mut self, mode: VimMode) {
        let previous = self.mode;
        self.mode = mode;
        let actions = self.entered(previous);
        if actions.is_empty() {
            return;
        }
        let context = self.take();
        for action in actions {
            self.queue.push_back((action, context.clone()));
        }
    }

    /// # Returns
    ///
    /// What entering the mode the machine now stands in asks for, having come from `previous`.
    fn entered(&mut self, previous: VimMode) -> Vec<Action> {
        match self.mode {
            VimMode::Normal => {
                self.shape = None;
                self.insert = None;
                let checkpoint: Action = EditorAction::History(HistoryAction::Checkpoint).into();
                if VimMode::Normal == previous {
                    return vec![checkpoint];
                }
                let target = if VimMode::Insert == previous {
                    EditTarget::Motion(
                        MoveType::Column(MoveDir1D::Previous, false),
                        Count::Exact(1),
                    )
                } else {
                    EditTarget::CurrentPosition
                };

                vec![
                    EditorAction::Cursor(CursorAction::Close(CursorCloseTarget::Followers)).into(),
                    EditorAction::Edit(Specifier::Exact(EditAction::Motion), target).into(),
                    checkpoint,
                ]
            }
            VimMode::Insert => {
                self.shape = None;
                if matches!(previous, VimMode::Normal | VimMode::Insert) {
                    return Vec::new();
                }

                vec![EditorAction::Edit(
                    Specifier::Exact(EditAction::Motion),
                    EditTarget::CurrentPosition,
                )
                .into()]
            }
            VimMode::Visual => {
                self.insert = None;

                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Applies one of the changes a key sequence makes to the context its actions run under.
    fn change(&mut self, change: &Change) {
        match change {
            Change::Operation(operation) => self.operation = operation.clone(),
            Change::CursorEnd(end) => self.cursor_end = Some(*end),
            Change::Shape(filter, shape) => match self.shape {
                Some(held) if filter.matches(&held) => self.shape = Some(*shape),
                None => self.shape = Some(*shape),
                Some(_) => {}
            },
            Change::Insert(style) => self.insert = Some(*style),
            Change::Register(register) => self.register = Some(register.clone()),
            Change::SearchChar(direction, inclusive) => {
                self.charsearch_params = (*direction, *inclusive);
                self.charsearch = self.typed.take();
            }
            Change::ReplaceChar => self.replace = self.typed.take(),
        }
    }

    /// Queues what a key no binding answers asks for, which is text in an inserting mode and
    /// nothing anywhere else.
    fn unmapped(&mut self, typed: TerminalKey) {
        if VimMode::Insert != self.mode {
            return;
        }
        let Some(character) = typed.get_char() else {
            return;
        };
        let action: Action = EditorAction::InsertText(InsertTextAction::Type(
            Specifier::Exact(Char::Single(character)),
            MoveDir1D::Previous,
            Count::Exact(1),
        ))
        .into();
        let context = self.take();
        self.queue.push_back((action, context));
    }

    /// Abandons a sequence no binding can complete, leaving the text as it stands.
    fn abandon(&mut self) {
        self.pending.clear();
        self.operator = None;
        self.postmode = None;
        if VimMode::Insert != self.mode {
            self.insert = None;
        }
        let _abandoned = self.take();
    }

    /// # Returns
    ///
    /// The context the actions of the sequence just completed run under, leaving the machine
    /// holding nothing of that sequence.
    fn take(&mut self) -> EditContext {
        let search_char = self
            .charsearch
            .clone()
            .map(|held| (self.charsearch_params.0, self.charsearch_params.1, held));
        let context = EditContextBuilder::default()
            .operation(std::mem::replace(&mut self.operation, EditAction::Motion))
            .count(self.count.take())
            .mark(None)
            .typed_char(self.typed.take())
            .cursor_end(self.cursor_end.take().unwrap_or(CursorEnd::Auto))
            .replace_char(self.replace.take())
            .search_char(search_char)
            .search_regex_dir(MoveDir1D::Next)
            .target_shape(self.shape)
            .insert_style(self.insert)
            .last_column(self.insert.is_some())
            .register(self.register.take())
            .register_append(std::mem::take(&mut self.register_append))
            .search_incremental(false)
            .build();
        self.counting = None;

        context
    }

    /// Folds the digits typed so far into the count the next action runs with.
    fn save_counting(&mut self) {
        let Some(counting) = self.counting.take() else {
            return;
        };
        self.count = Some(match self.count {
            None => counting,
            Some(count) => count.saturating_mul(counting),
        });
    }

    /// # Returns
    ///
    /// The table the next key is looked up in.
    fn lookup(&self) -> VimMode {
        if self.operator.is_some() {
            VimMode::OperationPending
        } else {
            self.mode
        }
    }

    /// # Returns
    ///
    /// Whether a digit typed where the machine stands starts or continues a count.
    fn counts(&self) -> bool {
        matches!(
            self.lookup(),
            VimMode::Normal | VimMode::Visual | VimMode::OperationPending
        )
    }

    /// # Returns
    ///
    /// Whether the register prefix typed where the machine stands names a register.
    fn registers(&self) -> bool {
        matches!(self.lookup(), VimMode::Normal | VimMode::Visual)
    }
}

impl Default for Keys {
    fn default() -> Self {
        Self::vim()
    }
}

/// One entry of a table before it is bound: the modes a sequence is read in, the keys it is
/// spelled by, and what it does.
type Entry = (Vec<VimMode>, String, Step);

/// The modes every sequence is read in, which for a motion is normal, visual and
/// operator-pending.
const MOTION_MODES: [VimMode; 3] = [VimMode::Normal, VimMode::Visual, VimMode::OperationPending];

/// The modes a text object is read in, which is everywhere a range rather than a place is what an
/// action wants.
const OBJECT_MODES: [VimMode; 2] = [VimMode::Visual, VimMode::OperationPending];

/// The modes a selection is started or ended from.
const SELECTING_MODES: [VimMode; 2] = [VimMode::Normal, VimMode::Visual];

/// The mode a normal-mode sequence is read in.
const NORMAL_MODES: [VimMode; 1] = [VimMode::Normal];

/// The mode a visual-mode sequence is read in.
const VISUAL_MODES: [VimMode; 1] = [VimMode::Visual];

/// The mode an inserting sequence is read in.
const INSERT_MODES: [VimMode; 1] = [VimMode::Insert];

/// An operator waiting for the motion or the text object it runs over.
#[derive(Clone, Debug)]
struct Operator {
    keys: Vec<TerminalKey>,
    doubled: Step,
}

/// # Returns
///
/// Every entry of the editor's own vim table that is not an operator's own.
fn table() -> Vec<Entry> {
    let mut table = motion_table();
    table.extend(object_table());
    table.extend(selection_table());
    table.extend(normal_table());
    table.extend(visual_table());
    table.extend(insert_table());

    table
}

/// # Returns
///
/// The entries naming a place in a text or on a screen, which are the ones a cursor, a selection
/// and an operator all read the same way.
fn motion_table() -> Vec<Entry> {
    let mut entries = Vec::new();
    for (keys, move_type) in [
        (
            "b",
            MoveType::WordBegin(WordStyle::Little, MoveDir1D::Previous),
        ),
        (
            "B",
            MoveType::WordBegin(WordStyle::Big, MoveDir1D::Previous),
        ),
        ("e", MoveType::WordEnd(WordStyle::Little, MoveDir1D::Next)),
        ("E", MoveType::WordEnd(WordStyle::Big, MoveDir1D::Next)),
        ("h", MoveType::Column(MoveDir1D::Previous, false)),
        ("l", MoveType::Column(MoveDir1D::Next, false)),
        (" ", MoveType::Column(MoveDir1D::Next, true)),
        ("j", MoveType::Line(MoveDir1D::Next)),
        ("k", MoveType::Line(MoveDir1D::Previous)),
        ("w", MoveType::WordBegin(WordStyle::Little, MoveDir1D::Next)),
        ("W", MoveType::WordBegin(WordStyle::Big, MoveDir1D::Next)),
        ("H", MoveType::ViewportPos(MovePosition::Beginning)),
        ("M", MoveType::ViewportPos(MovePosition::Middle)),
        ("L", MoveType::ViewportPos(MovePosition::End)),
        ("-", MoveType::FirstWord(MoveDir1D::Previous)),
        ("+", MoveType::FirstWord(MoveDir1D::Next)),
        ("<Enter>", MoveType::FirstWord(MoveDir1D::Next)),
        ("|", MoveType::LineColumnOffset),
        ("{prefix}j", MoveType::ScreenLine(MoveDir1D::Next)),
        ("{prefix}k", MoveType::ScreenLine(MoveDir1D::Previous)),
        (
            "{prefix}e",
            MoveType::WordEnd(WordStyle::Little, MoveDir1D::Previous),
        ),
        (
            "{prefix}E",
            MoveType::WordEnd(WordStyle::Big, MoveDir1D::Previous),
        ),
        ("{prefix}o", MoveType::BufferByteOffset),
    ] {
        entries.push(entry(
            &MOTION_MODES,
            keys,
            motion(move_type, Count::Contextual),
        ));
    }
    for (keys, move_type, count) in [
        (
            "0",
            MoveType::LinePos(MovePosition::Beginning),
            Count::Exact(0),
        ),
        ("$", MoveType::LinePos(MovePosition::End), Count::MinusOne),
        ("_", MoveType::FirstWord(MoveDir1D::Next), Count::MinusOne),
        (
            "{prefix}0",
            MoveType::ScreenLinePos(MovePosition::Beginning),
            Count::Exact(0),
        ),
        (
            "{prefix}^",
            MoveType::ScreenFirstWord(MoveDir1D::Next),
            Count::Exact(0),
        ),
        (
            "{prefix}$",
            MoveType::ScreenLinePos(MovePosition::End),
            Count::MinusOne,
        ),
        (
            "{prefix}_",
            MoveType::FinalNonBlank(MoveDir1D::Next),
            Count::MinusOne,
        ),
        (
            "{prefix}m",
            MoveType::ScreenLinePos(MovePosition::Middle),
            Count::Exact(0),
        ),
    ] {
        entries.push(entry(&MOTION_MODES, keys, motion(move_type, count)));
    }
    entries.push(entry(
        &MOTION_MODES,
        "^",
        run(
            vec![Change::Shape(TargetShapeFilter::ALL, TargetShape::CharWise)],
            vec![target(
                Specifier::Contextual,
                EditTarget::Motion(MoveType::FirstWord(MoveDir1D::Next), Count::Exact(0)),
            )],
            None,
        ),
    ));
    for (keys, bare, counted) in [
        (
            "G",
            MoveType::BufferPos(MovePosition::End),
            MoveType::BufferLineOffset,
        ),
        (
            "{prefix}g",
            MoveType::BufferPos(MovePosition::Beginning),
            MoveType::BufferLineOffset,
        ),
        ("%", MoveType::ItemMatch, MoveType::BufferLinePercent),
    ] {
        entries.push(entry(
            &MOTION_MODES,
            keys,
            counted_motion(bare, Count::Contextual, counted, Count::Contextual),
        ));
    }
    entries.push(entry(
        &MOTION_MODES,
        "{prefix}M",
        counted_motion(
            MoveType::LinePos(MovePosition::Middle),
            Count::MinusOne,
            MoveType::LinePercent,
            Count::Contextual,
        ),
    ));
    for (keys, direction, inclusive) in [
        ("f{any}", MoveDir1D::Next, true),
        ("F{any}", MoveDir1D::Previous, true),
        ("t{any}", MoveDir1D::Next, false),
        ("T{any}", MoveDir1D::Previous, false),
    ] {
        entries.push(entry(
            &MOTION_MODES,
            keys,
            run(
                vec![Change::SearchChar(direction, inclusive)],
                vec![target(Specifier::Contextual, char_search(MoveDirMod::Same))],
                None,
            ),
        ));
    }
    for (keys, modifier) in [(";", MoveDirMod::Same), (",", MoveDirMod::Flip)] {
        entries.push(entry(
            &MOTION_MODES,
            keys,
            run(
                Vec::new(),
                vec![target(Specifier::Contextual, char_search(modifier))],
                None,
            ),
        ));
    }

    entries
}

/// # Returns
///
/// The entries naming a range around the cursor rather than a place to travel to, which is what
/// `iw` and `i(` are.
fn object_table() -> Vec<Entry> {
    let mut objects: Vec<(String, RangeType, bool)> = [
        ("aw", RangeType::Word(WordStyle::Little), true),
        ("iw", RangeType::Word(WordStyle::Little), true),
        ("aW", RangeType::Word(WordStyle::Big), true),
        ("iW", RangeType::Word(WordStyle::Big), true),
    ]
    .into_iter()
    .map(|(keys, range, inclusive)| (keys.to_owned(), range, inclusive))
    .collect();
    for (opening, closing, extra) in [
        ('(', ')', Some('b')),
        ('[', ']', None),
        ('{', '}', Some('B')),
        ('<', '>', None),
    ] {
        for (around, inclusive) in [("a", true), ("i", false)] {
            let range = RangeType::Bracketed(opening, closing);
            for named in [Some(opening), Some(closing), extra].into_iter().flatten() {
                objects.push((format!("{around}{named}"), range.clone(), inclusive));
            }
        }
    }
    for quote in ['"', '\'', '`'] {
        for (around, inclusive) in [("a", true), ("i", false)] {
            objects.push((
                format!("{around}{quote}"),
                RangeType::Quote(quote),
                inclusive,
            ));
        }
    }

    objects
        .into_iter()
        .map(|(keys, range, inclusive)| {
            entry(
                &OBJECT_MODES,
                &keys,
                run(
                    Vec::new(),
                    vec![target(
                        Specifier::Contextual,
                        EditTarget::Range(range, inclusive, Count::Contextual),
                    )],
                    None,
                ),
            )
        })
        .collect()
}

/// # Returns
///
/// The entries that start a selection of a shape, or end the one that already has it.
fn selection_table() -> Vec<Entry> {
    [
        ("v", TargetShape::CharWise),
        ("V", TargetShape::LineWise),
        ("<C-V>", TargetShape::BlockWise),
    ]
    .into_iter()
    .map(|(keys, shape)| entry(&SELECTING_MODES, keys, Step::Visual(shape)))
    .collect()
}

/// # Returns
///
/// The entries read only in normal mode: the operators, the ways into an inserting mode, and the
/// edits that name what they take for themselves.
fn normal_table() -> Vec<Entry> {
    let mut entries = Vec::new();
    for (keys, operation) in [
        ("d", EditAction::Delete),
        ("y", EditAction::Yank),
        (
            ">",
            EditAction::Indent(IndentChange::Increase(Count::Exact(1))),
        ),
        (
            "<",
            EditAction::Indent(IndentChange::Decrease(Count::Exact(1))),
        ),
        ("=", EditAction::Indent(IndentChange::Auto)),
        ("{prefix}u", EditAction::ChangeCase(Case::Lower)),
        ("{prefix}U", EditAction::ChangeCase(Case::Upper)),
        ("{prefix}~", EditAction::ChangeCase(Case::Toggle)),
    ] {
        entries.push(entry(
            &NORMAL_MODES,
            keys,
            Step::Operator {
                operation: operation.clone(),
                mode: VimMode::Normal,
                insert: None,
                doubled: Box::new(lines(operation)),
            },
        ));
    }
    entries.push(entry(
        &NORMAL_MODES,
        "c",
        Step::Operator {
            operation: EditAction::Delete,
            mode: VimMode::Insert,
            insert: Some(InsertStyle::Insert),
            doubled: Box::new(run(
                Vec::new(),
                vec![target(
                    Specifier::Contextual,
                    EditTarget::Range(RangeType::Line, false, Count::Contextual),
                )],
                None,
            )),
        },
    ));
    for (keys, style, placed) in [
        ("i", InsertStyle::Insert, None),
        ("R", InsertStyle::Replace, None),
        (
            "a",
            InsertStyle::Insert,
            Some((MoveType::Column(MoveDir1D::Next, false), Count::Exact(1))),
        ),
        (
            "A",
            InsertStyle::Insert,
            Some((MoveType::LinePos(MovePosition::End), Count::Exact(0))),
        ),
        (
            "I",
            InsertStyle::Insert,
            Some((MoveType::FirstWord(MoveDir1D::Next), Count::Exact(0))),
        ),
        (
            "{prefix}I",
            InsertStyle::Insert,
            Some((MoveType::LinePos(MovePosition::Beginning), Count::Exact(0))),
        ),
    ] {
        entries.push(entry(&NORMAL_MODES, keys, inserting(style, placed)));
    }
    for (keys, direction) in [("o", MoveDir1D::Next), ("O", MoveDir1D::Previous)] {
        entries.push(entry(
            &NORMAL_MODES,
            keys,
            run(
                vec![Change::Insert(InsertStyle::Insert)],
                vec![
                    Emit::Always(EditorAction::Cursor(CursorAction::Split(Count::MinusOne)).into()),
                    Emit::Always(
                        EditorAction::InsertText(InsertTextAction::OpenLine(
                            TargetShape::LineWise,
                            direction,
                            Count::Exact(1),
                        ))
                        .into(),
                    ),
                ],
                Some(VimMode::Insert),
            ),
        ));
    }
    for (keys, edit_target) in [
        (
            "C",
            EditTarget::Motion(MoveType::LinePos(MovePosition::End), Count::MinusOne),
        ),
        (
            "s",
            EditTarget::Motion(MoveType::Column(MoveDir1D::Next, false), Count::Contextual),
        ),
        (
            "S",
            EditTarget::Range(RangeType::Line, false, Count::Contextual),
        ),
    ] {
        entries.push(entry(&NORMAL_MODES, keys, changing(edit_target)));
    }
    for (keys, operation, edit_target) in [
        (
            "x",
            EditAction::Delete,
            EditTarget::Motion(MoveType::Column(MoveDir1D::Next, false), Count::Contextual),
        ),
        (
            "X",
            EditAction::Delete,
            EditTarget::Motion(
                MoveType::Column(MoveDir1D::Previous, false),
                Count::Contextual,
            ),
        ),
        (
            "D",
            EditAction::Delete,
            EditTarget::Motion(MoveType::LinePos(MovePosition::End), Count::MinusOne),
        ),
        (
            "Y",
            EditAction::Yank,
            EditTarget::Range(RangeType::Line, true, Count::Contextual),
        ),
        (
            "J",
            EditAction::Join(JoinStyle::OneSpace),
            EditTarget::Range(RangeType::Line, true, Count::Contextual),
        ),
        (
            "{prefix}J",
            EditAction::Join(JoinStyle::NoChange),
            EditTarget::Range(RangeType::Line, true, Count::Contextual),
        ),
    ] {
        entries.push(entry(
            &NORMAL_MODES,
            keys,
            run(
                Vec::new(),
                vec![target(Specifier::Exact(operation), edit_target)],
                None,
            ),
        ));
    }
    entries.push(entry(
        &NORMAL_MODES,
        "~",
        run(
            vec![Change::CursorEnd(CursorEnd::End)],
            vec![target(
                Specifier::Exact(EditAction::ChangeCase(Case::Toggle)),
                EditTarget::Motion(MoveType::Column(MoveDir1D::Next, false), Count::Contextual),
            )],
            None,
        ),
    ));
    entries.push(entry(
        &NORMAL_MODES,
        "r{any}",
        replacing(EditTarget::Motion(
            MoveType::Column(MoveDir1D::Next, false),
            Count::Contextual,
        )),
    ));
    for (keys, direction) in [("p", MoveDir1D::Next), ("P", MoveDir1D::Previous)] {
        entries.push(entry(
            &NORMAL_MODES,
            keys,
            run(
                Vec::new(),
                vec![Emit::Always(
                    EditorAction::InsertText(InsertTextAction::Paste(
                        PasteStyle::Side(direction),
                        Count::Contextual,
                    ))
                    .into(),
                )],
                None,
            ),
        ));
    }
    for (keys, history) in [
        ("u", HistoryAction::Undo(Count::Contextual)),
        ("<C-R>", HistoryAction::Redo(Count::Contextual)),
    ] {
        entries.push(entry(
            &NORMAL_MODES,
            keys,
            run(
                Vec::new(),
                vec![Emit::Always(EditorAction::History(history).into())],
                Some(VimMode::Normal),
            ),
        ));
    }
    entries.push(entry(
        &NORMAL_MODES,
        "<Esc>",
        run(Vec::new(), Vec::new(), Some(VimMode::Normal)),
    ));

    entries
}

/// # Returns
///
/// The entries read only in visual mode, which are the ones that run over the selection rather
/// than waiting for a target.
fn visual_table() -> Vec<Entry> {
    let mut entries = Vec::new();
    for (keys, operation) in [
        ("d", EditAction::Delete),
        ("x", EditAction::Delete),
        ("y", EditAction::Yank),
        ("u", EditAction::ChangeCase(Case::Lower)),
        ("U", EditAction::ChangeCase(Case::Upper)),
        ("~", EditAction::ChangeCase(Case::Toggle)),
        ("{prefix}u", EditAction::ChangeCase(Case::Lower)),
        ("{prefix}U", EditAction::ChangeCase(Case::Upper)),
        ("{prefix}~", EditAction::ChangeCase(Case::Toggle)),
        ("J", EditAction::Join(JoinStyle::OneSpace)),
        ("{prefix}J", EditAction::Join(JoinStyle::NoChange)),
        (
            ">",
            EditAction::Indent(IndentChange::Increase(Count::Contextual)),
        ),
        (
            "<",
            EditAction::Indent(IndentChange::Decrease(Count::Contextual)),
        ),
        ("=", EditAction::Indent(IndentChange::Auto)),
    ] {
        entries.push(entry(
            &VISUAL_MODES,
            keys,
            run(
                Vec::new(),
                vec![target(Specifier::Exact(operation), EditTarget::Selection)],
                Some(VimMode::Normal),
            ),
        ));
    }
    entries.push(entry(
        &VISUAL_MODES,
        "Y",
        run(
            vec![Change::Shape(
                TargetShapeFilter::CHAR,
                TargetShape::LineWise,
            )],
            vec![target(
                Specifier::Exact(EditAction::Yank),
                EditTarget::Selection,
            )],
            Some(VimMode::Normal),
        ),
    ));
    for (keys, edit_target) in [
        (
            "D",
            EditTarget::Motion(MoveType::LinePos(MovePosition::End), Count::Exact(0)),
        ),
        ("X", EditTarget::Selection),
    ] {
        entries.push(entry(&VISUAL_MODES, keys, deleting_lines(edit_target)));
    }
    entries.push(entry(
        &VISUAL_MODES,
        "c",
        run(
            vec![Change::Insert(InsertStyle::Insert)],
            vec![
                Emit::Always(split(TargetShapeFilter::BLOCK)),
                Emit::Always(cursor_set(SelectionCursorChange::Beginning)),
                target(Specifier::Exact(EditAction::Delete), EditTarget::Selection),
                Emit::Always(EditorAction::Cursor(CursorAction::Split(Count::MinusOne)).into()),
            ],
            Some(VimMode::Insert),
        ),
    ));
    entries.push(entry(
        &VISUAL_MODES,
        "C",
        run(
            vec![
                Change::Shape(TargetShapeFilter::CHAR, TargetShape::LineWise),
                Change::Insert(InsertStyle::Insert),
            ],
            vec![
                Emit::Always(split(TargetShapeFilter::ALL)),
                Emit::Always(cursor_set(SelectionCursorChange::Beginning)),
                target(
                    Specifier::Exact(EditAction::Delete),
                    EditTarget::Motion(MoveType::LinePos(MovePosition::End), Count::Exact(0)),
                ),
                Emit::Always(EditorAction::Cursor(CursorAction::Split(Count::MinusOne)).into()),
            ],
            Some(VimMode::Insert),
        ),
    ));
    for keys in ["S", "R"] {
        entries.push(entry(
            &VISUAL_MODES,
            keys,
            run(
                vec![
                    Change::Shape(TargetShapeFilter::ALL, TargetShape::LineWise),
                    Change::Insert(InsertStyle::Insert),
                ],
                vec![target(
                    Specifier::Exact(EditAction::Delete),
                    EditTarget::Selection,
                )],
                Some(VimMode::Insert),
            ),
        ));
    }
    entries.push(entry(&VISUAL_MODES, "I", inserting_visual(None)));
    entries.push(entry(
        &VISUAL_MODES,
        "A",
        inserting_visual(Some(MoveType::Column(MoveDir1D::Next, false))),
    ));
    for (keys, change) in [
        ("o", SelectionCursorChange::SwapAnchor),
        ("O", SelectionCursorChange::SwapSide),
    ] {
        entries.push(entry(
            &VISUAL_MODES,
            keys,
            run(Vec::new(), vec![Emit::Always(cursor_set(change))], None),
        ));
    }
    for keys in ["p", "P"] {
        entries.push(entry(
            &VISUAL_MODES,
            keys,
            run(
                Vec::new(),
                vec![Emit::Always(
                    EditorAction::InsertText(InsertTextAction::Paste(
                        PasteStyle::Replace,
                        Count::Contextual,
                    ))
                    .into(),
                )],
                Some(VimMode::Normal),
            ),
        ));
    }
    entries.push(entry(
        &VISUAL_MODES,
        "r{any}",
        replacing(EditTarget::Selection),
    ));
    entries.push(entry(
        &VISUAL_MODES,
        "<Esc>",
        run(Vec::new(), Vec::new(), Some(VimMode::Normal)),
    ));

    entries
}

/// # Returns
///
/// The entries read only in an inserting mode, which are the keys that are not text.
fn insert_table() -> Vec<Entry> {
    vec![
        entry(
            &INSERT_MODES,
            "<Esc>",
            run(Vec::new(), Vec::new(), Some(VimMode::Normal)),
        ),
        entry(
            &INSERT_MODES,
            "<Enter>",
            run(
                Vec::new(),
                vec![Emit::Always(
                    EditorAction::InsertText(InsertTextAction::Type(
                        Specifier::Exact(Char::Single('\n')),
                        MoveDir1D::Previous,
                        Count::Exact(1),
                    ))
                    .into(),
                )],
                None,
            ),
        ),
        entry(
            &INSERT_MODES,
            "<BS>",
            run(
                vec![Change::Register(Register::Blackhole)],
                vec![target(
                    Specifier::Exact(EditAction::Delete),
                    EditTarget::Motion(
                        MoveType::Column(MoveDir1D::Previous, true),
                        Count::Contextual,
                    ),
                )],
                None,
            ),
        ),
    ]
}

/// # Returns
///
/// One entry of a table.
fn entry(modes: &[VimMode], keys: &str, step: Step) -> Entry {
    (modes.to_vec(), keys.to_owned(), step)
}
/// # Returns
///
/// The operator a sequence belongs to, the keys it is bound by, and what it does, for every entry
/// of the operator-pending table only one operator reads its keys that way in.
fn operator_table() -> Vec<(String, String, Step)> {
    [
        (
            "c",
            "w",
            MoveType::WordEnd(WordStyle::Little, MoveDir1D::Next),
        ),
        ("c", "W", MoveType::WordEnd(WordStyle::Big, MoveDir1D::Next)),
    ]
    .into_iter()
    .map(|(operator, keys, move_type)| {
        (
            operator.to_owned(),
            keys.to_owned(),
            motion(move_type, Count::Contextual),
        )
    })
    .collect()
}

/// # Returns
///
/// A step that changes the context, produces the actions and leaves the machine in `mode`.
fn run(changes: Vec<Change>, emits: Vec<Emit>, mode: Option<VimMode>) -> Step {
    Step::Run {
        changes,
        emits,
        mode,
    }
}

/// # Returns
///
/// The action running the operator the context holds over a motion.
fn motion(move_type: MoveType, count: Count) -> Step {
    run(
        Vec::new(),
        vec![target(
            Specifier::Contextual,
            EditTarget::Motion(move_type, count),
        )],
        None,
    )
}

/// # Returns
///
/// A step running one motion where no count was typed and another where one was, as `G` and `gg`
/// do.
fn counted_motion(bare: MoveType, bare_count: Count, counted: MoveType, count: Count) -> Step {
    run(
        Vec::new(),
        vec![Emit::Counted(
            action(Specifier::Contextual, EditTarget::Motion(bare, bare_count)),
            action(Specifier::Contextual, EditTarget::Motion(counted, count)),
        )],
        None,
    )
}

/// # Returns
///
/// A step running `operation` over whole lines, which is what an operator typed twice does.
fn lines(operation: EditAction) -> Step {
    run(
        Vec::new(),
        vec![target(
            Specifier::Exact(operation),
            EditTarget::Range(RangeType::Line, true, Count::Contextual),
        )],
        Some(VimMode::Normal),
    )
}

/// # Returns
///
/// A step entering an inserting mode, having first moved the cursor where the keys asked.
fn inserting(style: InsertStyle, placed: Option<(MoveType, Count)>) -> Step {
    let mut emits = Vec::new();
    if let Some((move_type, count)) = placed {
        emits.push(target(
            Specifier::Exact(EditAction::Motion),
            EditTarget::Motion(move_type, count),
        ));
    }
    emits.push(Emit::Always(
        EditorAction::Cursor(CursorAction::Split(Count::MinusOne)).into(),
    ));

    run(vec![Change::Insert(style)], emits, Some(VimMode::Insert))
}

/// # Returns
///
/// A step writing the character typed after the keys over what a target covers, as `r` does.
fn replacing(edit_target: EditTarget) -> Step {
    run(
        vec![
            Change::Operation(EditAction::Replace(false)),
            Change::ReplaceChar,
        ],
        vec![target(Specifier::Contextual, edit_target)],
        Some(VimMode::Normal),
    )
}

/// # Returns
///
/// A step deleting what a target covers and inserting in its place, as `c`, `s` and `S` do.
fn changing(edit_target: EditTarget) -> Step {
    run(
        vec![Change::Insert(InsertStyle::Insert)],
        vec![target(Specifier::Exact(EditAction::Delete), edit_target)],
        Some(VimMode::Insert),
    )
}

/// # Returns
///
/// A step deleting a selection as whole lines where it covered characters, as `D` and `X` do in
/// visual mode.
fn deleting_lines(edit_target: EditTarget) -> Step {
    run(
        vec![Change::Shape(
            TargetShapeFilter::CHAR,
            TargetShape::LineWise,
        )],
        vec![
            Emit::Always(split(TargetShapeFilter::ALL)),
            Emit::Always(cursor_set(SelectionCursorChange::Beginning)),
            target(Specifier::Exact(EditAction::Delete), edit_target),
        ],
        Some(VimMode::Normal),
    )
}

/// # Returns
///
/// A step inserting at one end of every line a selection covers, as `I` and `A` do in visual mode.
fn inserting_visual(placed: Option<MoveType>) -> Step {
    let change = if placed.is_some() {
        SelectionCursorChange::End
    } else {
        SelectionCursorChange::Beginning
    };
    let mut emits = vec![
        Emit::Always(split(TargetShapeFilter::BLOCK)),
        Emit::Always(cursor_set(change)),
        Emit::Always(EditorAction::Cursor(CursorAction::Split(Count::MinusOne)).into()),
    ];
    if let Some(move_type) = placed {
        emits.push(target(
            Specifier::Exact(EditAction::Motion),
            EditTarget::Motion(move_type, Count::Exact(1)),
        ));
    }

    run(
        vec![Change::Insert(InsertStyle::Insert)],
        emits,
        Some(VimMode::Insert),
    )
}

/// # Returns
///
/// The action splitting a selection into the lines it covers.
fn split(filter: TargetShapeFilter) -> Action {
    EditorAction::Selection(SelectionAction::Split(SelectionSplitStyle::Lines, filter)).into()
}

/// # Returns
///
/// The action moving a selection's cursor to one of its ends.
fn cursor_set(change: SelectionCursorChange) -> Action {
    EditorAction::Selection(SelectionAction::CursorSet(change)).into()
}

/// # Returns
///
/// The target a character search names, which is the one `f`, `t` and `;` are all answered by.
fn char_search(modifier: MoveDirMod) -> EditTarget {
    EditTarget::Search(SearchType::Char(false), modifier, Count::Contextual)
}

/// # Returns
///
/// The action running an operator over a target, produced whether or not a count was typed.
fn target(operation: Specifier<EditAction>, edit_target: EditTarget) -> Emit {
    Emit::Always(action(operation, edit_target))
}

/// # Returns
///
/// The action running an operator over a target.
fn action(operation: Specifier<EditAction>, edit_target: EditTarget) -> Action {
    EditorAction::Edit(operation, edit_target).into()
}

/// # Returns
///
/// The keys `keys` names, with `{prefix}` standing for `prefix` and `{any}` for a key of any
/// kind. A `<` that opens no named key stands for itself, which is how `<<` and `i<` are written.
///
/// # Panics
///
/// Panics if `keys` names a key that cannot be parsed.
fn edges(keys: &str, prefix: char) -> Vec<Edge> {
    let mut edges = Vec::new();
    let mut rest = keys;
    while let Some(character) = rest.chars().next() {
        if let Some(closing) = closed(rest, '{', '}') {
            let named = &rest[1..closing];
            edges.push(match named {
                "prefix" => Edge::Key(key(prefix)),
                "any" => Edge::Any,
                named => panic!("`{named}` is not a class of keys the table names"),
            });
            rest = &rest[closing + 1..];
            continue;
        }
        if let Some(closing) = closed(rest, '<', '>') {
            if let Ok(named) = TerminalKey::from_str(&rest[..=closing]) {
                edges.push(Edge::Key(named));
                rest = &rest[closing + 1..];
                continue;
            }
        }
        edges.push(Edge::Key(key(character)));
        rest = &rest[character.len_utf8()..];
    }

    edges
}

/// # Returns
///
/// The index of the character closing a group `text` opens with `open`, and [`None`] where the
/// text neither opens one nor closes it.
fn closed(text: &str, open: char, close: char) -> Option<usize> {
    if !text.starts_with(open) {
        return None;
    }

    text.find(close)
}

/// # Returns
///
/// The key typed when `character` is typed with no modifier held.
///
/// # Panics
///
/// Panics if `character` names no key, which no character does.
fn key(character: char) -> TerminalKey {
    TerminalKey::from_str(&character.to_string())
        .unwrap_or_else(|_| panic!("`{character}` names a key"))
}

/// # Returns
///
/// The keys `edges` names, with a key of any kind standing for itself.
///
/// # Panics
///
/// Panics if `edges` holds a key of any kind, which an operator's own keys never do.
fn keyed(edges: &[Edge]) -> Vec<TerminalKey> {
    edges
        .iter()
        .map(|edge| match edge {
            Edge::Key(key) => *key,
            Edge::Any => panic!("an operator is not typed by a key of any kind"),
        })
        .collect()
}

/// # Returns
///
/// Whether `keys` complete `edges`, whether they are a prefix of them, and [`None`] where they
/// are neither.
fn matched(edges: &[Edge], keys: &[TerminalKey]) -> Option<bool> {
    if keys.len() > edges.len() {
        return None;
    }
    for (edge, key) in edges.iter().zip(keys) {
        if let Edge::Key(bound) = edge {
            if bound != key {
                return None;
            }
        }
    }

    Some(edges.len() == keys.len())
}

/// # Returns
///
/// The character typed where `edges` asked for a key of any kind, and [`None`] where they asked
/// for none.
fn any_of(edges: &[Edge], keys: &[TerminalKey]) -> Option<char> {
    edges
        .iter()
        .zip(keys)
        .find(|(edge, _)| Edge::Any == **edge)
        .and_then(|(_, key)| key.get_char())
}

/// # Returns
///
/// The digit `typed` names, and [`None`] where it names none.
fn digit_of(typed: TerminalKey) -> Option<usize> {
    typed
        .get_char()
        .and_then(|character| character.to_digit(10))
        .map(|digit| digit as usize)
}

/// # Returns
///
/// The register `typed` names and whether it is appended to rather than replaced, and [`None`]
/// where it names none.
fn register_of(typed: TerminalKey) -> Option<(Register, bool)> {
    let character = typed.get_char()?;
    let register = match character {
        '0' => Register::LastYanked,
        '1'..='9' => Register::RecentlyDeleted(character as usize - '1' as usize),
        'a'..='z' => Register::Named(character),
        'A'..='Z' => return Some((Register::Named(character.to_ascii_lowercase()), true)),
        '"' => Register::Unnamed,
        '-' => Register::SmallDelete,
        '_' => Register::Blackhole,
        '.' => Register::LastInserted,
        '*' => Register::SelectionPrimary,
        '+' => Register::SelectionClipboard,
        _ => return None,
    };

    Some((register, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// # Returns
    ///
    /// What the editor's own table produces for the characters of `keys`, each typed on its own.
    fn produced(keys: &str) -> (Vec<(Action, EditContext)>, VimMode) {
        let mut machine = Keys::vim();
        let mut produced = Vec::new();
        for character in keys.chars() {
            machine.input_key(key(character));
            while let Some(pair) = machine.pop() {
                produced.push(pair);
            }
        }

        (produced, machine.mode())
    }

    #[test]
    fn a_sequence_names_the_prefix_and_a_key_of_any_kind_apart_from_the_keys_it_spells() {
        assert_eq!(
            vec![Edge::Key(key('z')), Edge::Key(key('j'))],
            edges("{prefix}j", 'z')
        );
        assert_eq!(vec![Edge::Key(key('f')), Edge::Any], edges("f{any}", 'g'));
        assert_eq!(
            vec![Edge::Key(key('<')), Edge::Key(key('<'))],
            edges("<<", 'g')
        );
    }

    #[test]
    fn a_count_in_front_of_an_operator_multiplies_the_one_in_front_of_its_motion() {
        let (produced, _mode) = produced("2d3w");

        assert_eq!(Some(6), produced[0].1.get_count());
    }

    #[test]
    fn an_operator_whose_sequence_died_leaves_the_machine_where_it_stood() {
        let (produced, mode) = produced("cQw");

        assert_eq!(VimMode::Normal, mode);
        assert_eq!(None, produced[0].1.get_insert_style());
    }
}
