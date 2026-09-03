//! Folding a transcript down to what a reader came to read.
//!
//! A transcript holds what a tool was asked, what it answered, what Claude was thinking and
//! whatever a subagent did on its own. All of it is worth keeping and none of it is worth drawing
//! in full while a reader looks for the one thing they came for, so the blocks holding it fold:
//! each is collapsed to a single summary row until it is opened, and `za`, `zo`, `zc`, `zR` and
//! `zM` are what opens and closes them.
//!
//! Subagent output arrives tagged with the id of the call it was produced under, at every depth,
//! so the folds nest. The call that started a subagent folds away the whole of what that subagent
//! did, and every call the subagent made folds away inside it. That nesting is read off the tags
//! rather than off the order the blocks arrived in, so a fold covers what was said beneath it
//! whatever else was said in between, and a fold at any depth opens and closes on its own.
//!
//! What folds is what is shown and never what is held. A fold names the blocks it covers, a
//! summary is computed from the source of those blocks, and nothing here writes to a transcript at
//! all, which is what makes a fold something a reader undoes by opening it rather than something
//! that costs them what it hid.
//!
//! The state a fold is in is held apart from the structure it folds and is named by the block the
//! fold heads, rather than by a row, a column or a width. A transcript grows only at its end, so
//! that name outlives what changes underneath it: the same folds are drawn after a resize, and a
//! tool result still being written stays folded and reports its new length the next time its
//! summary is asked for.
//!
//! A reader walking down the rows walks the folded transcript, where a closed fold is one row
//! however much it covers, and the rows a block is drawn in are the rows the block itself draws,
//! asked for a window at a time so that what is folded away below costs nothing to skip.

use std::collections::{BTreeMap, BTreeSet};

use vbc_layout::anchor::Wrapping;

use crate::chat::block::{Block, Kind, RenderedRow, RowWindow};
use crate::chat::transcript::Transcript;

/// The key a fold command is typed after in vim.
pub const PREFIX: char = 'z';

/// The key each fold command is bound to after [`PREFIX`].
const BINDINGS: [(char, Command); 5] = [
    ('M', Command::CloseAll),
    ('R', Command::OpenAll),
    ('a', Command::Toggle),
    ('c', Command::Close),
    ('o', Command::Open),
];

/// What is written before the summary of a fold, and the character repeated once per depth after
/// it, which is how vim draws a fold's own line.
const SUMMARY_MARK: char = '+';
const SUMMARY_DEPTH_MARK: char = '-';

/// The depth marks a fold at the outermost depth is drawn with.
const SUMMARY_DEPTH_MARKS: usize = 2;

/// What a summary calls a block that is not named by the tool it called.
const RESULT_LABEL: &str = "result";
const THINKING_LABEL: &str = "thinking";

/// What a summary counts what it folded away in, in the singular and in the plural.
const LINE_UNIT: &str = "line";
const LINES_UNIT: &str = "lines";

/// What a reader asks of the folds of a transcript.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    /// `za`: open the outermost closed fold at the cursor, or close the innermost open one where
    /// none is closed.
    Toggle,

    /// `zo`: open the outermost closed fold at the cursor.
    Open,

    /// `zc`: close the innermost open fold at the cursor.
    Close,

    /// `zR`: open every fold, at every depth.
    OpenAll,

    /// `zM`: close every fold, at every depth.
    CloseAll,
}

impl Command {
    /// Reads a sequence of typed keys as a fold command.
    ///
    /// # Returns
    ///
    /// What `keys` are: a fold command, the start of one, or neither.
    #[must_use]
    pub fn read(keys: &str) -> Reading {
        let mut typed = keys.chars();
        let Some(prefix) = typed.next() else {
            return Reading::Pending;
        };
        if PREFIX != prefix {
            return Reading::Unbound;
        }
        let Some(key) = typed.next() else {
            return Reading::Pending;
        };
        if typed.next().is_some() {
            return Reading::Unbound;
        }

        BINDINGS
            .iter()
            .find(|(bound, _)| key == *bound)
            .map_or(Reading::Unbound, |(_, command)| Reading::Bound(*command))
    }
}

/// What a sequence of typed keys turned out to be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reading {
    /// The keys are a fold command.
    Bound(Command),

    /// The keys are the start of a fold command and not yet one.
    Pending,

    /// The keys are no fold command.
    Unbound,
}

/// How a block arrived nested: the id a call to a tool is answered under, and the id of the call
/// the block itself arrived beneath.
///
/// A block that is neither is untagged, which is what the blocks of the conversation itself are.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Tag {
    id: Option<String>,
    parent: Option<String>,
}

impl Tag {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A tag naming the id `id` the block is answered under and the id `parent` it arrived
    /// beneath, either of which may be `None`.
    #[must_use]
    pub fn new(id: Option<String>, parent: Option<String>) -> Self {
        Self { id, parent }
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// The tag of a block that arrived under nothing and is answered under nothing.
    #[must_use]
    pub fn untagged() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    #[must_use]
    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }
}

/// One fold of a transcript: the block it heads, how deep it sits among the folds around it, and
/// the blocks it covers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fold {
    head: usize,
    depth: usize,
    covered: Vec<usize>,
}

impl Fold {
    /// # Returns
    ///
    /// The index of the block the fold heads, which is the block a summary is drawn from and the
    /// name the fold's own state is kept under.
    #[must_use]
    pub fn head(&self) -> usize {
        self.head
    }

    /// # Returns
    ///
    /// How many folds enclose this one.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// # Returns
    ///
    /// The index of every block the fold covers, the head included, in the order they were said.
    #[must_use]
    pub fn covered(&self) -> &[usize] {
        &self.covered
    }
}

/// The folds of a transcript and which of them are open.
///
/// Every fold is closed until it is opened. The structure is read off the transcript and the tags
/// its blocks arrived with; the state is a set of the blocks whose folds are open, which is why
/// reading the structure again over a transcript that has grown or changed leaves the state alone.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Folds {
    folds: Vec<Fold>,
    roots: Vec<usize>,
    children: Vec<Vec<usize>>,
    heads: BTreeMap<usize, usize>,
    open: BTreeSet<usize>,
}

impl Folds {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The folds of `transcript` under the tags `tags` its blocks arrived with, every one of them
    /// closed. A block the tags say nothing about is nested under nothing.
    #[must_use]
    pub fn of(transcript: &Transcript, tags: &[Tag]) -> Self {
        let mut folds = Self::default();
        folds.rebuild(transcript, tags);

        folds
    }

    /// Reads the folds of `transcript` again, keeping the state each of them is in.
    ///
    /// A fold is named by the block it heads, so a transcript that has grown at its end or whose
    /// blocks have been written further into is folded as it was before.
    pub fn rebuild(&mut self, transcript: &Transcript, tags: &[Tag]) {
        let blocks = transcript.len();
        let mut answered: BTreeMap<&str, usize> = BTreeMap::new();
        for index in 0..blocks {
            if let Some(id) = tags.get(index).and_then(Tag::id) {
                answered.entry(id).or_insert(index);
            }
        }

        self.roots = Vec::new();
        self.children = vec![Vec::new(); blocks];
        for index in 0..blocks {
            let beneath = tags
                .get(index)
                .and_then(Tag::parent)
                .and_then(|parent| answered.get(parent))
                .copied()
                .filter(|beneath| *beneath < index);
            match beneath {
                Some(parent) => self.children[parent].push(index),
                None => self.roots.push(index),
            }
        }

        self.folds = Vec::new();
        self.heads = BTreeMap::new();
        let mut pending: Vec<(usize, usize)> =
            self.roots.iter().rev().map(|block| (*block, 0)).collect();
        while let Some((block, depth)) = pending.pop() {
            let heads = transcript.block(block).is_some_and(folds_away);
            if heads {
                self.heads.insert(block, self.folds.len());
                self.folds.push(Fold {
                    head: block,
                    depth,
                    covered: covered_by(&self.children, block),
                });
            }

            let inner = if heads { depth + 1 } else { depth };
            pending.extend(
                self.children[block]
                    .iter()
                    .rev()
                    .map(|child| (*child, inner)),
            );
        }
    }

    /// # Returns
    ///
    /// Every fold of the transcript, in the order the blocks they head are drawn in.
    #[must_use]
    pub fn folds(&self) -> &[Fold] {
        &self.folds
    }

    /// # Returns
    ///
    /// The fold headed by the block `block`, or `None` where that block heads none.
    #[must_use]
    pub fn at(&self, block: usize) -> Option<&Fold> {
        self.heads
            .get(&block)
            .and_then(|fold| self.folds.get(*fold))
    }

    /// # Returns
    ///
    /// Every fold covering the block `block`, the one it heads included.
    pub fn covering(&self, block: usize) -> impl Iterator<Item = &Fold> {
        self.folds
            .iter()
            .filter(move |fold| fold.covered.binary_search(&block).is_ok())
    }

    /// # Returns
    ///
    /// Whether the fold headed by the block `head` is open. A block heading no fold folds nothing
    /// away and is open.
    #[must_use]
    pub fn is_open(&self, head: usize) -> bool {
        self.open.contains(&head)
    }

    /// Applies `command` to the folds covering the block `at`, which is the block the cursor is
    /// in.
    ///
    /// Opening and closing move one depth at a time, the way vim's own do: what a `zo` opens is
    /// the outermost fold still closed over the cursor, and what a `zc` closes is the innermost
    /// still open, so a fold nested inside another is opened and closed on its own.
    pub fn apply(&mut self, command: Command, at: usize) {
        match command {
            Command::Toggle => {
                if self
                    .covering(at)
                    .any(|fold| !self.open.contains(&fold.head))
                {
                    self.open_one(at);
                } else {
                    self.close_one(at);
                }
            }
            Command::Open => self.open_one(at),
            Command::Close => self.close_one(at),
            Command::OpenAll => self.open = self.folds.iter().map(Fold::head).collect(),
            Command::CloseAll => self.open.clear(),
        }
    }

    /// Opens the outermost fold still closed over the block `at`, where there is one.
    fn open_one(&mut self, at: usize) {
        let head = self
            .covering(at)
            .filter(|fold| !self.open.contains(&fold.head))
            .min_by_key(|fold| fold.depth)
            .map(Fold::head);
        if let Some(head) = head {
            self.open.insert(head);
        }
    }

    /// Closes the innermost fold still open over the block `at`, where there is one.
    fn close_one(&mut self, at: usize) {
        let head = self
            .covering(at)
            .filter(|fold| self.open.contains(&fold.head))
            .max_by_key(|fold| fold.depth)
            .map(Fold::head);
        if let Some(head) = head {
            self.open.remove(&head);
        }
    }
}

/// The one row a closed fold is drawn in: which fold it stands for, how deep that fold sits, and
/// the text saying what was folded away and how much of it there is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Summary {
    head: usize,
    depth: usize,
    text: String,
}

impl Summary {
    /// # Returns
    ///
    /// The index of the block headed by the fold this summary stands for.
    #[must_use]
    pub fn head(&self) -> usize {
        self.head
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// One entry of a folded transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    /// A closed fold, drawn in the one row its summary is.
    Summary(Summary),

    /// A block, drawn in the rows of its own source.
    Body(usize),
}

impl Entry {
    /// # Returns
    ///
    /// The index of the block the entry draws, which is the block a closed fold heads where the
    /// entry is a summary.
    #[must_use]
    pub fn block(&self) -> usize {
        match self {
            Self::Summary(summary) => summary.head,
            Self::Body(block) => *block,
        }
    }
}

/// Where a reader is in a folded transcript: which entry, and which row of that entry.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Position {
    entry: usize,
    row: usize,
}

impl Position {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The position of the row `row` of the entry `entry`.
    #[must_use]
    pub fn new(entry: usize, row: usize) -> Self {
        Self { entry, row }
    }

    #[must_use]
    pub fn entry(&self) -> usize {
        self.entry
    }

    #[must_use]
    pub fn row(&self) -> usize {
        self.row
    }
}

/// One row of a folded transcript as it is drawn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Row<'view> {
    /// The row a closed fold is collapsed to, which is one row at every width: unlike a body row
    /// it is neither wrapped nor cut to the panel, so fitting it to the columns there are is the
    /// business of whatever draws it.
    Summary(&'view Summary),

    /// A row of a block, drawn from that block's own source.
    Body {
        /// The index of the block the row was drawn from.
        block: usize,

        /// The row itself, naming the bytes of that block it shows.
        row: RenderedRow,
    },
}

/// A transcript as its folds leave it: the entries a reader sees, in the order they are drawn.
///
/// A view is built from the state of the folds and from the transcript as it stands, and is what a
/// reader moves and draws through. It holds no rendered row and no laid-out block, so a view built
/// again after a resize, after a fold was opened, or after a tool wrote another line is the same
/// view of the same folds over whatever the transcript now holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct View<'transcript> {
    transcript: &'transcript Transcript,
    entries: Vec<Entry>,
}

impl<'transcript> View<'transcript> {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The view of `transcript` left by `folds`, in which a closed fold is one summary entry and
    /// every other block is an entry of its own, the blocks nested beneath a block following it.
    #[must_use]
    pub fn of(folds: &Folds, transcript: &'transcript Transcript) -> Self {
        let mut entries = Vec::new();
        let mut pending: Vec<usize> = folds.roots.iter().rev().copied().collect();
        while let Some(block) = pending.pop() {
            match folds.at(block) {
                Some(fold) if !folds.is_open(fold.head) => {
                    entries.push(Entry::Summary(summarize(transcript, fold)));
                }
                _ => {
                    entries.push(Entry::Body(block));
                    if let Some(children) = folds.children.get(block) {
                        pending.extend(children.iter().rev().copied());
                    }
                }
            }
        }

        Self {
            transcript,
            entries,
        }
    }

    /// # Returns
    ///
    /// The entries a reader sees, top to bottom.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// # Returns
    ///
    /// The position one row below `at`, which is the top of the next entry wherever `at` is the
    /// last row of its own, or `None` where `at` is the last row of the last entry.
    #[must_use]
    pub fn down(&self, at: Position, wrapping: &Wrapping) -> Option<Position> {
        let entry = self.entries.get(at.entry)?;
        if let Entry::Body(block) = entry {
            let below = RowWindow::new(at.row + 1, 1);
            let drawn = self
                .transcript
                .block(*block)
                .is_some_and(|block| !block.render(below, wrapping).rows().is_empty());
            if drawn {
                return Some(Position::new(at.entry, at.row + 1));
            }
        }

        (at.entry + 1 < self.entries.len()).then(|| Position::new(at.entry + 1, 0))
    }

    /// # Returns
    ///
    /// The position one row above `at`, which is the last row of the entry above wherever `at` is
    /// the first row of its own, or `None` where `at` is the first row of the first entry.
    #[must_use]
    pub fn up(&self, at: Position, wrapping: &Wrapping) -> Option<Position> {
        if 0 < at.row {
            return Some(Position::new(at.entry, at.row - 1));
        }

        let above = at.entry.checked_sub(1)?;

        Some(Position::new(
            above,
            self.rows(above, wrapping).saturating_sub(1),
        ))
    }

    /// Counts the rows the entry `entry` is drawn in.
    ///
    /// A closed fold is one row however much it covers. A block is as many rows as its own source
    /// is drawn in, which costs that block: that is what a reader walking upward pays at the
    /// boundary they cross, and is why walking downward asks for a row at a time instead.
    ///
    /// Measured in release at eighty columns, as the fastest of nine runs: one step upward across
    /// an entry boundary costs 57 µs above an open hundred-line block and 88 ms above an open
    /// hundred-thousand-line one, while the same step above a closed fold costs 33 ns at either
    /// length, and a step downward and a twenty-row render stay flat at both lengths. So a fold
    /// costs nothing to walk over however much it hides, and a block a reader has opened costs
    /// what it holds to arrive at from below.
    ///
    /// # Returns
    ///
    /// The number of rows the entry is drawn in, or zero where the view holds no such entry.
    #[must_use]
    pub fn rows(&self, entry: usize, wrapping: &Wrapping) -> usize {
        match self.entries.get(entry) {
            Some(Entry::Summary(_)) => 1,
            Some(Entry::Body(block)) => self.transcript.block(*block).map_or(0, |block| {
                block
                    .render(RowWindow::new(0, usize::MAX), wrapping)
                    .rows()
                    .len()
            }),
            None => 0,
        }
    }

    /// Draws `rows` rows of the folded transcript from `from` downward.
    ///
    /// Each block is asked for the window of it that falls in those rows and for nothing below,
    /// and a closed fold is drawn as the one row of its summary, so what is folded away and what
    /// lies past the bottom of the panel both cost nothing to walk over.
    ///
    /// # Returns
    ///
    /// The rows drawn, top to bottom, which are fewer than were asked for where the transcript
    /// ends inside them.
    #[must_use]
    pub fn render(&self, from: Position, rows: usize, wrapping: &Wrapping) -> Vec<Row<'_>> {
        let mut drawn: Vec<Row<'_>> = Vec::new();
        let mut at = from;
        while drawn.len() < rows {
            let Some(entry) = self.entries.get(at.entry) else {
                break;
            };

            match entry {
                Entry::Summary(summary) => {
                    if 0 == at.row {
                        drawn.push(Row::Summary(summary));
                    }
                }
                Entry::Body(block) => {
                    if let Some(source) = self.transcript.block(*block) {
                        let window = RowWindow::new(at.row, rows - drawn.len());
                        drawn.extend(source.render(window, wrapping).rows().iter().map(|row| {
                            Row::Body {
                                block: *block,
                                row: row.clone(),
                            }
                        }));
                    }
                }
            }

            at = Position::new(at.entry + 1, 0);
        }

        drawn
    }
}

/// # Returns
///
/// Whether `block` is one of the kinds that fold: a call to a tool, what a tool answered, or what
/// Claude was thinking. What a subagent did folds away under the call that started it rather than
/// as a kind of its own.
fn folds_away(block: &Block) -> bool {
    matches!(
        block.kind(),
        Kind::ToolCall { .. } | Kind::ToolResult | Kind::Thinking
    )
}

/// # Returns
///
/// The index of every block beneath `head` in the nesting `children` describes, `head` included,
/// in the order they were said.
fn covered_by(children: &[Vec<usize>], head: usize) -> Vec<usize> {
    let mut covered = Vec::new();
    let mut pending = vec![head];
    while let Some(block) = pending.pop() {
        covered.push(block);
        if let Some(beneath) = children.get(block) {
            pending.extend(beneath.iter().copied());
        }
    }
    covered.sort_unstable();

    covered
}

/// # Returns
///
/// The summary the closed fold `fold` of `transcript` is drawn as: how deep it sits, how many
/// lines it covers between every block it holds, and what the block it heads was.
fn summarize(transcript: &Transcript, fold: &Fold) -> Summary {
    let lines: usize = fold
        .covered
        .iter()
        .filter_map(|block| transcript.block(*block))
        .map(|block| lines_of(block.source()))
        .sum();
    let label = transcript.block(fold.head).map_or(String::new(), label_of);
    let marks: String =
        std::iter::repeat_n(SUMMARY_DEPTH_MARK, SUMMARY_DEPTH_MARKS + fold.depth).collect();
    let unit = if 1 == lines { LINE_UNIT } else { LINES_UNIT };
    let text = format!("{SUMMARY_MARK}{marks} {lines} {unit}: {label}")
        .trim_end()
        .to_owned();

    Summary {
        head: fold.head,
        depth: fold.depth,
        text,
    }
}

/// # Returns
///
/// The number of logical lines of `source`, which is one for a source holding no separator at all.
fn lines_of(source: &str) -> usize {
    1 + source.matches('\n').count()
}

/// # Returns
///
/// What a summary calls `block`: what it was, and the first line of what it said.
fn label_of(block: &Block) -> String {
    let said = block.source().lines().next().unwrap_or_default().trim();
    match block.kind() {
        Kind::ToolCall { name } => format!("{name} {said}"),
        Kind::ToolResult => format!("{RESULT_LABEL} {said}"),
        Kind::Thinking => format!("{THINKING_LABEL} {said}"),
        _ => said.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use vbc_layout::anchor::Wrapping;
    use vbc_layout::line::Options;
    use vbc_layout::width::Metrics;

    use crate::chat::block::{Block, Kind, Role};
    use crate::chat::transcript::Transcript;

    use super::{Command, Entry, Fold, Folds, Position, Reading, Row, Tag, View};

    /// The width the fixtures are drawn at, at which none of them wraps.
    const UNWRAPPED: usize = 64;

    /// The width at which the wrapped fixture takes three rows.
    const NARROW: usize = 4;

    /// The ids the fixture's calls are answered under.
    const OUTER: &str = "toolu_outer";
    const INNER: &str = "toolu_inner";

    #[test]
    fn every_fold_command_is_read_from_the_keys_vim_binds_it_to() {
        for (keys, command) in [
            ("za", Command::Toggle),
            ("zo", Command::Open),
            ("zc", Command::Close),
            ("zR", Command::OpenAll),
            ("zM", Command::CloseAll),
        ] {
            assert_eq!(Reading::Bound(command), Command::read(keys));
        }

        for keys in ["", "z"] {
            assert_eq!(Reading::Pending, Command::read(keys));
        }

        for keys in ["zx", "zA", "zr", "zm", "a", "gg", "zaa", "az"] {
            assert_eq!(
                Reading::Unbound,
                Command::read(keys),
                "{keys:?} was read as a fold command"
            );
        }
    }

    #[test]
    fn a_tool_call_a_tool_result_and_a_thought_fold_and_the_conversation_itself_does_not() {
        let transcript: Transcript = vec![
            Block::new(Kind::Message(Role::User), "make it hold".to_owned()),
            Block::new(
                Kind::ToolCall {
                    name: "Bash".to_owned(),
                },
                "cargo test".to_owned(),
            ),
            Block::from_ansi(Kind::ToolResult, "ok"),
            Block::new(Kind::Thinking, "it holds".to_owned()),
            Block::new(
                Kind::Code {
                    language: Some("rust".to_owned()),
                },
                "fn main() {}".to_owned(),
            ),
            Block::diff("one\n", "two\n"),
            Block::new(Kind::Message(Role::Assistant), "it holds".to_owned()),
        ]
        .into_iter()
        .collect();
        let folds = Folds::of(&transcript, &[]);

        assert_eq!(
            vec![1, 2, 3],
            folds.folds().iter().map(Fold::head).collect::<Vec<usize>>()
        );
        for fold in folds.folds() {
            assert_eq!(0, fold.depth());
            assert_eq!(&[fold.head()], fold.covered());
        }
    }

    #[test]
    fn subagent_output_nests_under_the_call_it_arrived_beneath() {
        let (transcript, tags) = nested();
        let folds = Folds::of(&transcript, &tags);

        assert_eq!(
            vec![
                (0, 0, vec![0, 1, 2, 3, 4]),
                (2, 1, vec![2, 3]),
                (3, 2, vec![3]),
                (4, 1, vec![4]),
            ],
            folds
                .folds()
                .iter()
                .map(|fold| (fold.head(), fold.depth(), fold.covered().to_vec()))
                .collect::<Vec<(usize, usize, Vec<usize>)>>()
        );
        assert_eq!(
            vec![0, 2, 3],
            folds.covering(3).map(Fold::head).collect::<Vec<usize>>()
        );
        assert_eq!(
            vec![0],
            folds.covering(1).map(Fold::head).collect::<Vec<usize>>()
        );
        assert_eq!(None, folds.at(1));
    }

    #[test]
    fn a_tag_naming_no_earlier_call_leaves_its_block_where_it_was_said() {
        let transcript: Transcript = vec![
            Block::new(Kind::Thinking, "first".to_owned()),
            Block::new(Kind::Message(Role::Assistant), "orphaned".to_owned()),
            Block::new(Kind::Message(Role::Assistant), "circular".to_owned()),
        ]
        .into_iter()
        .collect();
        let tags = vec![
            Tag::new(Some(OUTER.to_owned()), Some(INNER.to_owned())),
            Tag::new(None, Some("toolu_nobody".to_owned())),
            Tag::new(Some(INNER.to_owned()), Some(INNER.to_owned())),
        ];
        let folds = Folds::of(&transcript, &tags);

        assert_eq!(
            vec![vec![0]],
            folds
                .folds()
                .iter()
                .map(|fold| fold.covered().to_vec())
                .collect::<Vec<Vec<usize>>>(),
            "a tag naming a call that was not made moved a block under it"
        );
        let view = View::of(&folds, &transcript);
        let entries = view.entries();
        let Some(Entry::Summary(summary)) = entries.first() else {
            panic!("the thought was not folded away: {entries:?}");
        };
        assert_eq!(0, summary.head());
        assert_eq!(&[Entry::Body(1), Entry::Body(2)], &entries[1..]);
    }

    #[test]
    fn a_closed_fold_is_one_summary_and_an_open_one_is_the_blocks_it_covers() {
        let (transcript, tags) = nested();
        let mut folds = Folds::of(&transcript, &tags);

        let closed = View::of(&folds, &transcript);
        let entries = closed.entries();
        let Some(Entry::Summary(summary)) = entries.first() else {
            panic!("the call to the subagent was not folded away: {entries:?}");
        };
        assert_eq!(0, summary.head());
        assert_eq!(
            &[Entry::Body(5)],
            &entries[1..],
            "the block said after the fold was folded away with it"
        );

        folds.apply(Command::OpenAll, 0);
        assert_eq!(
            vec![
                Entry::Body(0),
                Entry::Body(1),
                Entry::Body(2),
                Entry::Body(3),
                Entry::Body(4),
                Entry::Body(5),
            ],
            View::of(&folds, &transcript).entries()
        );
    }

    #[test]
    fn a_summary_says_what_it_folded_away_and_how_much_of_it_there_is() {
        let (transcript, tags) = nested();
        let mut folds = Folds::of(&transcript, &tags);
        folds.apply(Command::Open, 0);
        folds.apply(Command::Open, 2);

        assert_eq!(
            vec![
                "reporting on the anchor",
                "cargo test -p vbc-layout",
                "+---- 1 line: result ok",
                "+--- 1 line: thinking the anchor holds",
            ],
            drawn(&View::of(&folds, &transcript), &wrapping(UNWRAPPED))[1..5]
        );

        folds.apply(Command::CloseAll, 0);
        assert_eq!(
            vec!["+-- 5 lines: Task review the anchor", "afterwards"],
            drawn(&View::of(&folds, &transcript), &wrapping(UNWRAPPED))
        );
    }

    #[test]
    fn toggling_opens_what_is_closed_and_closes_what_is_open() {
        let (transcript, tags) = nested();
        let mut folds = Folds::of(&transcript, &tags);

        folds.apply(Command::Toggle, 3);
        assert!(folds.is_open(0) && !folds.is_open(2) && !folds.is_open(3));

        folds.apply(Command::Toggle, 3);
        assert!(folds.is_open(0) && folds.is_open(2) && !folds.is_open(3));

        folds.apply(Command::Toggle, 3);
        assert!(folds.is_open(3));

        folds.apply(Command::Toggle, 3);
        assert!(folds.is_open(0) && folds.is_open(2) && !folds.is_open(3));
    }

    #[test]
    fn walking_down_the_rows_of_a_block_and_back_up_returns_the_way_it_came() {
        let (transcript, tags) = nested();
        let mut folds = Folds::of(&transcript, &tags);
        folds.apply(Command::OpenAll, 0);
        let view = View::of(&folds, &transcript);
        let wrapping = wrapping(NARROW);

        let mut walked = vec![Position::new(0, 0)];
        while let Some(next) =
            view.down(*walked.last().expect("the walk began somewhere"), &wrapping)
        {
            walked.push(next);
        }

        let rows: usize = (0..view.entries().len())
            .map(|entry| view.rows(entry, &wrapping))
            .sum();
        assert_eq!(rows, walked.len());
        assert!(
            walked.iter().any(|at| 0 < at.row()),
            "no block of the fixture wrapped, so nothing walked within one"
        );

        let mut back = vec![*walked.last().expect("the walk ended somewhere")];
        while let Some(above) = view.up(
            *back.last().expect("the walk back began somewhere"),
            &wrapping,
        ) {
            back.push(above);
        }
        back.reverse();

        assert_eq!(walked, back);
    }

    #[test]
    fn a_render_draws_the_rows_it_was_asked_for_from_where_it_was_asked() {
        let (transcript, tags) = nested();
        let mut folds = Folds::of(&transcript, &tags);
        folds.apply(Command::Open, 0);
        let view = View::of(&folds, &transcript);
        let wrapping = wrapping(UNWRAPPED);

        let whole = drawn(&view, &wrapping);
        assert_eq!(
            vec![
                "review the anchor",
                "reporting on the anchor",
                "+--- 2 lines: Bash cargo test -p vbc-layout",
                "+--- 1 line: thinking the anchor holds",
                "afterwards",
            ],
            whole
        );

        let below = view.render(Position::new(1, 0), 2, &wrapping);
        assert_eq!(whole[1..3], texts(&below));
        assert_eq!(
            Vec::<String>::new(),
            texts(&view.render(Position::new(9, 0), 4, &wrapping)),
            "a render past the end of the view drew something"
        );
        assert_eq!(
            Vec::<String>::new(),
            texts(&view.render(Position::new(0, 0), 0, &wrapping))
        );
    }

    #[test]
    fn a_transcript_holding_nothing_folds_to_nothing() {
        let transcript = Transcript::new();
        let folds = Folds::of(&transcript, &[]);
        let view = View::of(&folds, &transcript);

        assert_eq!(&[] as &[Fold], folds.folds());
        assert_eq!(&[] as &[Entry], view.entries());
        assert_eq!(None, view.down(Position::new(0, 0), &wrapping(UNWRAPPED)));
        assert_eq!(None, view.up(Position::new(0, 0), &wrapping(UNWRAPPED)));
        assert_eq!(0, view.rows(0, &wrapping(UNWRAPPED)));
    }

    /// # Returns
    ///
    /// A transcript nesting a call inside a call inside a call, and the tags its blocks arrived
    /// with: a subagent was asked to review, said what it was doing, ran a command, and thought
    /// about the answer, and the conversation went on afterwards.
    fn nested() -> (Transcript, Vec<Tag>) {
        let transcript = vec![
            Block::new(
                Kind::ToolCall {
                    name: "Task".to_owned(),
                },
                "review the anchor".to_owned(),
            ),
            Block::new(
                Kind::Message(Role::Assistant),
                "reporting on the anchor".to_owned(),
            ),
            Block::new(
                Kind::ToolCall {
                    name: "Bash".to_owned(),
                },
                "cargo test -p vbc-layout".to_owned(),
            ),
            Block::from_ansi(Kind::ToolResult, "ok"),
            Block::new(Kind::Thinking, "the anchor holds".to_owned()),
            Block::new(Kind::Message(Role::Assistant), "afterwards".to_owned()),
        ]
        .into_iter()
        .collect();
        let tags = vec![
            Tag::new(Some(OUTER.to_owned()), None),
            Tag::new(None, Some(OUTER.to_owned())),
            Tag::new(Some(INNER.to_owned()), Some(OUTER.to_owned())),
            Tag::new(None, Some(INNER.to_owned())),
            Tag::new(None, Some(OUTER.to_owned())),
            Tag::untagged(),
        ];

        (transcript, tags)
    }

    /// # Returns
    ///
    /// A wrapping drawing rows `width` columns wide under vim's own defaults.
    fn wrapping(width: usize) -> Wrapping {
        Wrapping::new(
            NonZeroUsize::new(width).expect("a fixture is drawn in at least one column"),
            Metrics::default(),
            Options::new(),
        )
    }

    /// # Returns
    ///
    /// The text of every row `view` draws, top to bottom.
    fn drawn(view: &View<'_>, wrapping: &Wrapping) -> Vec<String> {
        let rows: usize = (0..view.entries().len())
            .map(|entry| view.rows(entry, wrapping))
            .sum();

        texts(&view.render(Position::new(0, 0), rows, wrapping))
    }

    /// # Returns
    ///
    /// The text of each of `rows`, top to bottom.
    fn texts(rows: &[Row<'_>]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Summary(summary) => summary.text().to_owned(),
                Row::Body { row, .. } => row.styled().row().text().to_owned(),
            })
            .collect()
    }
}
