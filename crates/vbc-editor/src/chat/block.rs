//! The blocks a transcript is a sequence of.
//!
//! A block is one thing that was said: a message, a fenced code block, a call to a tool, what the
//! tool answered, a thinking block, or a diff. It holds the source it was built from and the spans
//! styling it, and the spans name byte ranges of that source, so nothing a block draws is
//! addressed in any coordinate but the source's.
//!
//! Rendering is a projection over a window of that source. A block is asked for a [`RowWindow`],
//! which names where to start and how many rows to draw, and draws out from there the way
//! [`vbc_layout::anchor`]'s mapping walks out from an anchor: a logical line at a time, nothing
//! remembered between calls and nothing below the window touched at all. What it keeps is the
//! window and only the window.
//!
//! A window names its start as a position rather than as an ordinal. A [`RowAnchor`] says which
//! logical line the window begins inside and which of that line's rows it begins at, so the block
//! above that line is not read at all, let alone laid out, and what a window costs is the rows it
//! draws whatever the content is written from and whatever the wrapping options ask for. A reader
//! scrolling by a row moves the anchor [`Rendered::next`] hands back rather than counting rows
//! again, so a keystroke costs a screenful however far down a block the screenful sits.
//!
//! What goes unread is the block above the anchor's own logical line, not that line above the
//! anchor. Where every line is shorter than a screenful those are the same thing; where one of
//! them is not, they are not, which is what the paragraph on long lines below is about.
//!
//! A window may still name the row it starts at, which is what a caller holding an ordinal has,
//! and that window is walked down to before it is drawn. Where the lines above it are plain text
//! wrapped at the column it runs out of the walk is cheap -- the rows such a line takes are its
//! length over the width, so stepping over it is reading where it ends -- and where they are not,
//! the walk lays out every line it steps over to count it. That walk is [`Block::anchor`], written
//! apart from the drawing so that what it costs is spent by a caller that asks for it rather than
//! by every frame.
//!
//! A line is not always shorter than the window drawn into it either: a minified document arrives
//! as one logical line of megabytes, and laying it out whole to draw twenty rows of it costs the
//! line rather than the rows. So a line is laid out only as far as the window reaches into it,
//! and what a window into such a line costs is where it reaches rather than how long the line is.
//!
//! Where it reaches is still counted from the line's own first row, because a line can only be
//! laid out from its start: a continuation row's decoration, the tab stops its text is measured
//! against and the word it may not be broken inside are all read from there. So a window anchored
//! deep inside one logical line lays out that line's rows above it, throws them away again, and
//! costs them. Bounding that as well needs a layout that can be resumed from a byte of a line
//! rather than only from the first of it, which [`vbc_layout::line`] does not offer; until it
//! does, what is bounded is the block above a window and the line below it, not the line above it.
//!
//! Measured in release at eighty columns, an anchored window of twenty rows of a
//! hundred-thousand-line block asks the allocator for the same bytes in the same number of calls
//! and takes the same time at row 0, at row 50,000 and at row 99,000: 121,800 bytes in 1,364 calls
//! and 42 µs of printable ASCII, 125,180 in 1,394 and 42 µs of tab-indented lines, 76,960 in 824
//! and 25 µs of CJK, 83,720 in 824 and 29 µs of emoji, and 131,040 in 1,264 and 41 µs of
//! box-drawing characters, and the same under each of the options that move where a line breaks.
//! The window numbered by row 99,000 of those same blocks costs the walk down to it instead: it
//! asks for 121,800 bytes and 499 µs where the lines above it are read off their lengths, and for
//! 467 MB in 1.3 million calls and 93 ms, 245 MB and 59 ms, 258 MB and 68 ms, and 466 MB and
//! 101 ms where they are laid out to be counted. Twenty rows off the top of a sixteen-megabyte
//! logical line ask for 196,816 bytes in 2,063 calls and take 65 µs, which is what the same rows
//! of a one-megabyte line cost, where laying either of those lines out whole asked for 2.6 GB and
//! 160 MB and took 2.0 s and 64 ms. Anchored at row 100, at row 1,000 and at row 3,000 of either
//! of those two lines, twenty rows ask for 1.1 MB, 9.0 MB and 20.7 MB and take 227 µs, 1.6 ms and
//! 4.5 ms: the rows of the line above the window, laid out to reach it and thrown away again. The
//! two lines ask for exactly the same at each of those rows, which is the length of the line
//! below the window not being read.
//!
//! `chat_cost.rs` measures all of this rather than taking it on trust, over content that is not
//! only printable ASCII and under wrapping options that are not only vim's defaults, because those
//! were the one configuration the earlier bound held in.
//!
//! A rendered row carries the byte offset of the source it starts at, which is what lets the
//! source behind a run of rows be recovered exactly -- separators included, which no row's own
//! text holds -- and is what a selection over rendered rows will be turned into a range of the
//! source with.

use std::ops::{Range, RangeInclusive};

use vbc_layout::anchor::Wrapping;
use vbc_layout::buffer::LINE_SEPARATOR;
use vbc_layout::line::{self, DisplayRow, Options};

use crate::chat::{ansi, diff};
use crate::style::{self, Span, StyledRow};

/// The bytes a line whose rows are its length over the width may be written from: the printable
/// ASCII, each of which is one column wide and a grapheme cluster of its own, and none of which is
/// a tab or a control character. Every one of them is greater than [`SEPARATOR`], so the first byte
/// outside this range either ends the line or rules the line out.
const PLAIN: RangeInclusive<u8> = 0x20..=0x7e;

/// The byte a logical line ends on, which is [`LINE_SEPARATOR`] as it is written in the source.
const SEPARATOR: u8 = LINE_SEPARATOR as u8;

/// The rows past the last one a window asks for that a prefix of a logical line has to be drawn in
/// before the rows above them are the rows the whole line is drawn in there. Wrapping places a row
/// from the row before it and from no more of the text than the row after it can hold, so a prefix
/// that reaches two rows further has settled every row the window keeps.
const PROBE_ROWS: usize = 2;

/// The bytes a column of a display row is guessed to be written in, which is what the first prefix
/// of a logical line is measured by. Text a column of which takes more bytes than that -- anything
/// but the printable ASCII -- leaves the prefix short of the rows it was meant to reach, and a
/// prefix that falls short is doubled rather than trusted, so the guess decides how many prefixes
/// are laid out rather than which rows come back.
const PROBE_BYTES_PER_COLUMN: usize = 1;

/// The characters a logical line's indent is written from, which a prefix of the line reaches past
/// so that a continuation row of it carries the decoration the whole line gives it.
const BLANK: [char; 2] = [' ', '\t'];

/// Who a message was said by.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// The person at the keyboard.
    User,

    /// Claude.
    Assistant,
}

/// What a block of a transcript is.
///
/// The kind carries what the block is about rather than how it is drawn: a code block's language,
/// a tool call's tool. How each is styled is the drawing code's business, and what a motion may
/// treat as an object is the block itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Kind {
    /// Prose said by one side of the conversation.
    Message(Role),

    /// A fenced code block, in the language it was fenced with where it named one.
    Code {
        /// The language the fence named, or `None` where it named none.
        language: Option<String>,
    },

    /// A call to a tool, named by the tool it calls.
    ToolCall {
        /// The name of the tool called.
        name: String,
    },

    /// What a tool answered, which is where the ANSI escapes turn up.
    ToolResult,

    /// Claude's reasoning, which a reader may fold away.
    Thinking,

    /// The lines an edit changed, computed from the text either side of it, named by the file the
    /// edit was to.
    Diff {
        /// The path of the file the edit was to, which is what a patch written from the block
        /// names.
        path: String,
    },
}

/// Where a window of a block begins: the logical line it starts inside, named by the byte offset
/// that line starts at and the index it is numbered by, and the display row of that line the
/// window starts at.
///
/// An anchor is a position rather than an ordinal, which is what lets a window be reached without
/// counting the rows of the block above the line it names. The rows of that line above it are
/// counted all the same, because a line is laid out from its first row and from nowhere else.
///
/// Nothing here checks that the offset starts a logical line or that the index numbers it: an
/// anchor is the caller's own position, handed to it by whatever drew the rows around it, and a
/// caller naming a position the block does not hold draws from where it named rather than from
/// anywhere else.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RowAnchor {
    offset: usize,
    line: usize,
    row: usize,
}

impl RowAnchor {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The anchor of the display row `row` of the logical line starting at byte `offset`, which is
    /// the line numbered `line`.
    #[must_use]
    pub fn new(offset: usize, line: usize, row: usize) -> Self {
        Self { offset, line, row }
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// The anchor of the first row of a block.
    #[must_use]
    pub fn top() -> Self {
        Self::new(0, 0, 0)
    }

    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }

    #[must_use]
    pub fn row(&self) -> usize {
        self.row
    }
}

/// Where a window of a block starts.
///
/// A window names its start either as a position or as an ordinal, and the two cost different
/// things: a position is drawn from where it names, and an ordinal is a row the block has to be
/// walked down to before anything can be drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Start {
    /// The row of the block the window starts at, counted from its first.
    Row(usize),

    /// Where in the block's source the window starts.
    At(RowAnchor),
}

/// The display rows of a block a caller is asking to be drawn: where to start, and how many rows
/// to draw from there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RowWindow {
    start: Start,
    rows: usize,
}

impl RowWindow {
    /// Factory function.
    ///
    /// A window named by a row is reached by counting the rows above it, so drawing one costs what
    /// the block above it costs as well as the rows it draws. [`RowWindow::at`] names the same
    /// window as a position and costs the block above it nothing.
    ///
    /// # Returns
    ///
    /// A window of `rows` display rows starting at the block's row `start`.
    #[must_use]
    pub fn new(start: usize, rows: usize) -> Self {
        Self {
            start: Start::Row(start),
            rows,
        }
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A window of `rows` display rows starting where `anchor` names.
    #[must_use]
    pub fn at(anchor: RowAnchor, rows: usize) -> Self {
        Self {
            start: Start::At(anchor),
            rows,
        }
    }

    /// # Returns
    ///
    /// The row of the block the window starts at, or `None` where the window names a position
    /// rather than a row.
    #[must_use]
    pub fn start(&self) -> Option<usize> {
        match self.start {
            Start::Row(row) => Some(row),
            Start::At(_) => None,
        }
    }

    /// # Returns
    ///
    /// Where the window starts, or `None` where the window names a row rather than a position.
    #[must_use]
    pub fn anchor(&self) -> Option<RowAnchor> {
        match self.start {
            Start::Row(_) => None,
            Start::At(anchor) => Some(anchor),
        }
    }

    /// # Returns
    ///
    /// The number of rows the window draws.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }
}

/// One block of a transcript: what it is, the source it was built from, and the spans styling that
/// source.
///
/// Whether the source is written from plain bytes alone is read off it when the block is built,
/// beside the runs its spans are resolved into, because that is the last moment either can change:
/// a block is what was said, and what was said does not change afterwards.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    kind: Kind,
    body: style::Block,
    plain: bool,
}

impl Block {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// An unstyled block of `source`.
    #[must_use]
    pub fn new(kind: Kind, source: String) -> Self {
        Self::of(kind, style::Block::new(source))
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A block of `source` styled by `spans`, in the order they are given.
    #[must_use]
    pub fn with_spans(kind: Kind, source: String, spans: Vec<Span>) -> Self {
        Self::of(kind, style::Block::with_spans(source, spans))
    }

    /// Factory function.
    ///
    /// Reads the ANSI escapes of `raw` as the styles they name, so that the block's source is the
    /// text a reader sees rather than the bytes a terminal was sent.
    ///
    /// # Returns
    ///
    /// A block of the text of `raw`, styled by the renditions its escapes selected.
    #[must_use]
    pub fn from_ansi(kind: Kind, raw: &str) -> Self {
        Self::of(kind, ansi::parse(raw))
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A [`Kind::Diff`] block of the lines between the text `old` an edit to `path` replaced and
    /// the text `new` it wrote.
    #[must_use]
    pub fn diff(path: String, old: &str, new: &str) -> Self {
        Self::of(Kind::Diff { path }, diff::compute(old, new))
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A block of `kind` drawn from `body`.
    fn of(kind: Kind, body: style::Block) -> Self {
        let plain = is_plain(body.source());

        Self { kind, body, plain }
    }

    #[must_use]
    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    #[must_use]
    pub fn source(&self) -> &str {
        self.body.source()
    }

    #[must_use]
    pub fn spans(&self) -> &[Span] {
        self.body.spans()
    }

    /// # Returns
    ///
    /// The source behind `range`, or `None` if `range` is not a range of it.
    #[must_use]
    pub fn slice(&self, range: Range<usize>) -> Option<&str> {
        self.body.slice(range)
    }

    /// Lays out the window `window` asks for and applies the block's spans to the rows in it.
    ///
    /// Only the lines the window draws are kept, and only as much of each of them as the window
    /// reaches, so a window holds the rows it was asked for however far down a block it sits and
    /// however long the line it sits inside runs on below it.
    ///
    /// What it costs is the rows it draws, together with the rows of its own logical line above
    /// it, once the window names where it starts, and the rows of the whole block above it as well
    /// once the window names which row it starts at: an anchored window is drawn from the first
    /// row of the line it names, and a numbered one is walked down to first.
    ///
    /// # Returns
    ///
    /// The rows of the window, top to bottom, which are fewer than were asked for where the block
    /// ends inside the window and none at all where it ends above it.
    #[must_use]
    pub fn render(&self, window: RowWindow, wrapping: &Wrapping) -> Rendered {
        let anchor = match window.start {
            Start::Row(row) => self.anchor(row, wrapping),
            Start::At(anchor) => anchor,
        };

        self.drawn(window, anchor, wrapping)
    }

    /// Walks down to the block's row `row`, counting the rows above it rather than drawing them.
    ///
    /// This is the cost an anchor exists to be spent once rather than every frame: a caller that
    /// keeps the anchor it was handed and moves it with [`Rendered::next`] never walks again,
    /// while one that names a row walks the block above that row each time it draws.
    ///
    /// # Returns
    ///
    /// Where the block's row `row` begins, which is past the end of the source where the block is
    /// drawn in fewer rows than that.
    #[must_use]
    pub fn anchor(&self, row: usize, wrapping: &Wrapping) -> RowAnchor {
        let end = self.body.source().len();
        let mut above = row;
        let mut at = RowAnchor::top();

        while at.offset <= end {
            let (text, counted) = self.counted_line(at.offset, at.line, wrapping);
            if above < counted {
                return RowAnchor::new(at.offset, at.line, above);
            }

            above -= counted;
            at = RowAnchor::new(
                at.offset + text.len() + LINE_SEPARATOR.len_utf8(),
                at.line + 1,
                0,
            );
        }

        RowAnchor::new(at.offset, at.line, above)
    }

    /// Steps one display row down from `anchor`.
    ///
    /// What that costs is the logical line the anchor names rather than the block around it, so a
    /// reader walking downward pays a line a row wherever in the block they stand.
    ///
    /// # Returns
    ///
    /// Where the row below `anchor` begins, or `None` where `anchor` names the block's last row or
    /// a row past its end.
    #[must_use]
    pub fn below(&self, anchor: RowAnchor, wrapping: &Wrapping) -> Option<RowAnchor> {
        let end = self.body.source().len();
        if end < anchor.offset {
            return None;
        }

        let (text, counted) = self.counted_line(anchor.offset, anchor.line, wrapping);
        if anchor.row + 1 < counted {
            return Some(RowAnchor::new(anchor.offset, anchor.line, anchor.row + 1));
        }

        let following = anchor.offset + text.len() + LINE_SEPARATOR.len_utf8();

        (following <= end).then(|| RowAnchor::new(following, anchor.line + 1, 0))
    }

    /// Steps one display row up from `anchor`.
    ///
    /// What that costs is the logical line above the one the anchor names rather than the block
    /// around it, so a reader walking upward pays a line a row wherever in the block they stand.
    ///
    /// # Returns
    ///
    /// Where the row above `anchor` begins, or `None` where `anchor` names the block's first row.
    #[must_use]
    pub fn above(&self, anchor: RowAnchor, wrapping: &Wrapping) -> Option<RowAnchor> {
        if 0 < anchor.row {
            return Some(RowAnchor::new(anchor.offset, anchor.line, anchor.row - 1));
        }

        let ended = anchor.offset.checked_sub(LINE_SEPARATOR.len_utf8())?;
        let line = anchor.line.checked_sub(1)?;
        let start = self.body.source()[..ended]
            .rfind(LINE_SEPARATOR)
            .map_or(0, |at| at + LINE_SEPARATOR.len_utf8());
        let (_, counted) = self.counted_line(start, line, wrapping);

        Some(RowAnchor::new(start, line, counted - 1))
    }

    /// Reaches the block's last display row without walking the rows above it.
    ///
    /// What that costs is the block's last logical line, together with a read of the source to
    /// number it, rather than the layout of everything above that line.
    ///
    /// # Returns
    ///
    /// Where the block's last display row begins, which is its first row where the block is drawn
    /// in one row.
    #[must_use]
    pub fn bottom(&self, wrapping: &Wrapping) -> RowAnchor {
        let source = self.body.source();
        let start = source
            .rfind(LINE_SEPARATOR)
            .map_or(0, |at| at + LINE_SEPARATOR.len_utf8());
        let line = source[..start].matches(LINE_SEPARATOR).count();
        let (_, counted) = self.counted_line(start, line, wrapping);

        RowAnchor::new(start, line, counted - 1)
    }

    /// Draws the rows `window` asks for from `anchor` downward.
    ///
    /// # Returns
    ///
    /// The window as it is drawn.
    fn drawn(&self, window: RowWindow, anchor: RowAnchor, wrapping: &Wrapping) -> Rendered {
        let end = self.body.source().len();
        let wanted = window.rows;
        let mut rows: Vec<RenderedRow> = Vec::new();
        let mut at = anchor;
        let mut next = None;

        while at.offset <= end && rows.len() < wanted {
            let below = wanted - rows.len();
            let rest = &self.body.source()[at.offset..];
            let reached = at.row.saturating_add(below);
            let (laid_out, length) = laid_out_to(rest, at.line, wrapping, reached);
            let following = length.map(|length| {
                RowAnchor::new(
                    at.offset + length + LINE_SEPARATOR.len_utf8(),
                    at.line + 1,
                    0,
                )
            });

            if laid_out.len() <= at.row {
                let ended = following.expect("a line drawn in fewer rows than were asked for ends");
                at = RowAnchor::new(ended.offset, ended.line, at.row - laid_out.len());
                continue;
            }

            let taken = (laid_out.len() - at.row).min(below);
            let mut start = at.offset + bytes_of(&laid_out[..at.row]);
            for styled in self
                .body
                .style_rows(start, &laid_out[at.row..at.row + taken])
            {
                let length = styled.row().text().len();
                rows.push(RenderedRow { start, styled });
                start += length;
            }

            let past = at.row + taken;
            next = Some(match following {
                Some(following) if laid_out.len() <= past => following,
                _ => RowAnchor::new(at.offset, at.line, past),
            });

            let Some(following) = following else {
                break;
            };
            at = following;
        }

        Rendered {
            start: window.start(),
            anchor,
            next: next.filter(|below| below.offset <= end),
            rows,
        }
    }

    /// Counts the display rows the whole of the block is drawn in.
    ///
    /// Counting is not drawing: a line whose rows can be read off its length is never laid out at
    /// all, and one that has to be laid out is thrown away again as soon as its rows have been
    /// counted, so the count holds no row of the block whatever it is written from. In release at
    /// eighty columns, counting a hundred-thousand-line block of plain lines asks the allocator for
    /// nothing at all and takes 1.0 ms, and counting one of tab-indented lines asks for 942 MB in
    /// 2.7 million calls and takes 200 ms while holding a line of it at a time; drawing either to
    /// count its rows asked for 1.2 GB in 13.6 million calls, held every row of it at once, and
    /// took 507 ms.
    ///
    /// # Returns
    ///
    /// The number of display rows the block is drawn in, which is at least one because its first
    /// logical line is drawn even where the block is empty.
    #[must_use]
    pub fn row_count(&self, wrapping: &Wrapping) -> usize {
        let end = self.body.source().len();
        let mut counted = 0;
        let mut offset = 0;
        let mut index = 0;

        while offset <= end {
            let (text, rows) = self.counted_line(offset, index, wrapping);
            counted += rows;
            offset += text.len() + LINE_SEPARATOR.len_utf8();
            index += 1;
        }

        counted
    }

    /// Reads a logical line of the block and the number of display rows it is drawn in, laying it
    /// out only where it has to be.
    ///
    /// A line of [`PLAIN`] bytes wrapped at the column it runs out of breaks there and nowhere
    /// else, and its bytes are its columns, so the rows it takes are its length over the width and
    /// there is nothing to lay out. A block written from those bytes throughout says so, which is
    /// what leaves a walk over such a block reading nothing but the ends of its lines; a block that
    /// carries anything else is asked line by line, so one tab in a hundred thousand lines slows
    /// the ninety-nine thousand plain ones rather than laying them out. Every line that is not
    /// plain -- one carrying a tab, a control character or a cluster of more than one byte -- and
    /// every line at all under `'linebreak'`, `'showbreak'` or `'breakindent'` is laid out and the
    /// layout thrown away again.
    ///
    /// # Returns
    ///
    /// The logical line starting at `offset`, its separator excluded, and the number of display
    /// rows it is drawn in, which is one for an empty line.
    ///
    /// # Panics
    ///
    /// Panics if `offset` is not a byte offset of the block's source, or falls inside one of its
    /// characters.
    fn counted_line(&self, offset: usize, index: usize, wrapping: &Wrapping) -> (&str, usize) {
        let text = line_at(self.body.source(), offset);
        if breaks_at_the_column(wrapping.options()) && (self.plain || is_plain(text)) {
            return (text, text.len().div_ceil(wrapping.width().get()).max(1));
        }

        (text, laid_out(text, index, wrapping).len())
    }
}

/// One display row a block is drawn in, together with the byte offset of the source it starts at.
///
/// The offset is the row's own: a row's text names the bytes it shows, and the offset says where
/// among the block's source those bytes were taken from, which the row alone cannot say because
/// the separators between logical lines are drawn by no row at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedRow {
    start: usize,
    styled: StyledRow,
}

impl RenderedRow {
    /// # Returns
    ///
    /// The byte offset of the block's source at which the row's text starts.
    #[must_use]
    pub fn start(&self) -> usize {
        self.start
    }

    /// # Returns
    ///
    /// The styled row the block is drawn in.
    #[must_use]
    pub fn styled(&self) -> &StyledRow {
        &self.styled
    }

    /// # Returns
    ///
    /// The byte range of the block's source the row's text was taken from.
    #[must_use]
    pub fn source(&self) -> Range<usize> {
        self.start..self.start + self.styled.row().text().len()
    }
}

/// A window of a block as it is drawn: the rows it was asked for, top to bottom, and where the
/// window began and ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rendered {
    start: Option<usize>,
    anchor: RowAnchor,
    next: Option<RowAnchor>,
    rows: Vec<RenderedRow>,
}

impl Rendered {
    #[must_use]
    pub fn rows(&self) -> &[RenderedRow] {
        &self.rows
    }

    /// # Returns
    ///
    /// The row of the block the window began at, which is the row the first of these rows is
    /// wherever any were drawn at all, or `None` where the window named a position rather than a
    /// row.
    #[must_use]
    pub fn start(&self) -> Option<usize> {
        self.start
    }

    /// # Returns
    ///
    /// Where the window began, which is where it was anchored and is where the first of these
    /// rows is wherever any were drawn at all.
    #[must_use]
    pub fn anchor(&self) -> RowAnchor {
        self.anchor
    }

    /// # Returns
    ///
    /// Where the row below the last of these rows begins, which is what a reader scrolling by a
    /// row anchors on next, or `None` where the window drew nothing or the block ends with it.
    #[must_use]
    pub fn next(&self) -> Option<RowAnchor> {
        self.next
    }

    /// # Returns
    ///
    /// The byte range of the block's source the window was drawn from, which includes the
    /// separators no row draws, or `None` where the window drew nothing.
    #[must_use]
    pub fn source(&self) -> Option<Range<usize>> {
        let first = self.rows.first()?;
        let last = self.rows.last()?;

        Some(first.start..last.source().end)
    }
}

/// # Returns
///
/// The logical line of `source` starting at `offset`, its separator excluded.
///
/// # Panics
///
/// Panics if `offset` is not a byte offset of `source`, or falls inside one of its characters.
fn line_at(source: &str, offset: usize) -> &str {
    let rest = &source[offset..];

    rest.find(LINE_SEPARATOR).map_or(rest, |at| &rest[..at])
}

/// # Returns
///
/// The number of bytes of a logical line that `rows` show between them.
fn bytes_of(rows: &[DisplayRow]) -> usize {
    rows.iter().map(|row| row.text().len()).sum()
}

/// # Returns
///
/// The display rows the logical line `text`, which is the line numbered `index`, is drawn in.
fn laid_out(text: &str, index: usize, wrapping: &Wrapping) -> Vec<DisplayRow> {
    line::lay_out(
        index,
        text,
        wrapping.width(),
        wrapping.metrics(),
        wrapping.options(),
    )
}

/// Lays out as much of the logical line starting `rest` as it takes to reach its row `wanted`.
///
/// A line is not always shorter than the window drawn into it: what a tool answers is whatever it
/// wrote, and a minified document arrives as one line of several megabytes. Laying such a line out
/// whole to draw twenty rows of it costs the line rather than the rows, and so does looking for
/// where it ends, so a prefix of `rest` is read and laid out instead and doubled until it either
/// reaches the end of the line or is drawn in more rows than were asked for. Wrapping is greedy
/// and reads no further ahead than the row it is placing, so every row of a prefix but its last
/// two is the row the whole line is drawn in there, and the last two are thrown away.
///
/// # Returns
///
/// The display rows the line is drawn in, which are `wanted` of them where the line is drawn in
/// more than that, paired with the length of the line where the whole of it was read and `None`
/// where only a prefix was.
fn laid_out_to(
    rest: &str,
    index: usize,
    wrapping: &Wrapping,
    wanted: usize,
) -> (Vec<DisplayRow>, Option<usize>) {
    let indent = if wrapping.options().break_indent() {
        rest.len() - rest.trim_start_matches(BLANK).len()
    } else {
        0
    };
    let mut probe = wanted
        .saturating_add(PROBE_ROWS)
        .saturating_mul(wrapping.width().get())
        .saturating_mul(PROBE_BYTES_PER_COLUMN)
        .max(indent.saturating_add(1));

    loop {
        let head = &rest[..boundary(rest, probe)];
        if let Some(at) = head.find(LINE_SEPARATOR) {
            return (laid_out(&head[..at], index, wrapping), Some(at));
        }
        if head.len() == rest.len() {
            return (laid_out(rest, index, wrapping), Some(rest.len()));
        }

        let mut rows = laid_out(head, index, wrapping);
        if wanted.saturating_add(PROBE_ROWS) <= rows.len() {
            rows.truncate(wanted);
            return (rows, None);
        }

        probe = probe.saturating_mul(2);
    }
}

/// # Returns
///
/// The greatest byte offset of `text` no further than `at` that starts one of its characters.
fn boundary(text: &str, at: usize) -> usize {
    let mut cut = at.min(text.len());
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }

    cut
}

/// # Returns
///
/// Whether every byte of `text` is one a line of [`PLAIN`] bytes may be written from, the
/// separator between its logical lines included.
fn is_plain(text: &str) -> bool {
    text.bytes()
        .all(|byte| PLAIN.contains(&byte) || SEPARATOR == byte)
}

/// # Returns
///
/// Whether `options` wrap a line at the column it runs out of and nowhere else, which is what
/// leaves a plain line's rows to be read off its length.
fn breaks_at_the_column(options: &Options) -> bool {
    !options.break_indent() && !options.line_break() && options.show_break().is_empty()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::num::NonZeroUsize;
    use std::ops::Range;

    use ratatui::style::{Color, Modifier, Style};
    use vbc_layout::anchor::Wrapping;
    use vbc_layout::line::Options;
    use vbc_layout::width::{grapheme_indices, Metrics};

    use crate::style::{Span, StyledSegment};

    use super::{Block, Kind, Rendered, RenderedRow, Role, RowAnchor, RowWindow};

    /// The width the fixtures wrap at, narrow enough that most of them take several rows.
    const WIDTH: usize = 5;

    /// The width no fixture wraps at.
    const UNWRAPPED: usize = 64;

    /// The escape every ANSI sequence starts with, which no rendered source may hold.
    const ESCAPE: char = '\u{1b}';

    /// Tool output of the shape `cargo` writes: a bold red word, then plain text.
    const COLOURED: &str = "\u{1b}[1;31merror\u{1b}[0m: nope\n\u{1b}[2Kdone";

    /// A source of clusters that are neither one code point nor one column: CJK, a joined emoji,
    /// and a letter carrying a combining mark.
    const CLUSTERS: &str = "\u{4e2d}\u{6587} e\u{301} \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} \
                            end\n\u{884c}\u{884c}";

    /// A source of the bytes whose columns are not their length: a tab, a carriage return, a bell
    /// and a delete, each of which vim draws in more columns than it is written in.
    const CONTROLS: &str = "a\tb\rc\u{7}d\u{7f}e\n\tindented";

    /// The markers a continuation row is decorated with where the options ask for one.
    const SHOW_BREAK: &str = "> ";

    #[test]
    fn every_kind_of_block_round_trips_through_a_render() {
        for block in fixtures() {
            let rendered = block.render(whole(&block), &wrapping(WIDTH));
            assert_eq!(
                Some(block.source()),
                rendered.source().and_then(|range| block.slice(range)),
                "a block of {:?} did not round trip",
                block.kind()
            );

            let drawn: String = rendered
                .rows()
                .iter()
                .map(|row| row.styled().row().text())
                .collect();
            assert_eq!(
                block.source().replace('\n', ""),
                drawn,
                "a block of {:?} was not drawn from its whole source",
                block.kind()
            );
        }
    }

    #[test]
    fn a_window_anywhere_in_a_block_draws_the_rows_the_whole_of_it_draws_there() {
        for block in fixtures() {
            for wrapping in wrappings() {
                let whole = block.render(whole(&block), &wrapping);
                let all = whole.rows();
                for start in 0..2 + all.len() {
                    for wanted in [0, 1, 2, 3, 1 + all.len()] {
                        let drawn = block.render(RowWindow::new(start, wanted), &wrapping);
                        let end = (start + wanted).min(all.len());
                        let wanted_rows = all.get(start.min(all.len())..end).unwrap_or_default();

                        assert_eq!(
                            wanted_rows,
                            drawn.rows(),
                            "a window of {wanted} rows at row {start} of a block of {:?} drew \
                             something other than those rows of the whole of it",
                            block.kind()
                        );
                        assert_eq!(Some(start), drawn.start());
                    }
                }
            }
        }
    }

    #[test]
    fn a_window_anchored_where_a_row_is_draws_what_the_window_numbered_by_that_row_draws() {
        for block in fixtures() {
            for wrapping in wrappings() {
                let all = block.render(whole(&block), &wrapping).rows().len();
                for start in 0..2 + all {
                    for wanted in [0, 1, 2, 3, 1 + all] {
                        let numbered = block.render(RowWindow::new(start, wanted), &wrapping);
                        let anchor = block.anchor(start, &wrapping);
                        let anchored = block.render(RowWindow::at(anchor, wanted), &wrapping);

                        assert_eq!(
                            numbered.rows(),
                            anchored.rows(),
                            "a window of {wanted} rows anchored at {anchor:?} of a block of {:?} \
                             drew something other than the window numbered by row {start}",
                            block.kind()
                        );
                        assert_eq!(None, anchored.start());
                        assert_eq!(anchor, anchored.anchor());
                    }
                }
            }
        }
    }

    #[test]
    fn stepping_by_the_anchor_below_a_window_walks_the_rows_the_whole_block_draws() {
        for block in fixtures() {
            for wrapping in wrappings() {
                let all = block.render(whole(&block), &wrapping);
                let mut walked = Vec::new();
                let mut at = Some(RowAnchor::top());
                while let Some(anchor) = at {
                    let drawn = block.render(RowWindow::at(anchor, 1), &wrapping);
                    walked.extend(drawn.rows().iter().cloned());
                    at = drawn.next();
                }

                assert_eq!(
                    all.rows(),
                    walked,
                    "stepping a row at a time through a block of {:?} walked rows the whole of it \
                     does not draw",
                    block.kind()
                );
            }
        }
    }

    #[test]
    fn stepping_a_row_at_a_time_either_way_reaches_the_anchor_of_every_row_of_a_block() {
        for block in fixtures() {
            for wrapping in wrappings() {
                let all = block.render(whole(&block), &wrapping).rows().len();
                let anchors: Vec<RowAnchor> =
                    (0..all).map(|row| block.anchor(row, &wrapping)).collect();

                let mut walked = vec![RowAnchor::top()];
                while let Some(below) =
                    block.below(*walked.last().expect("the walk began somewhere"), &wrapping)
                {
                    walked.push(below);
                }

                assert_eq!(
                    anchors,
                    walked,
                    "stepping down a block of {:?} a row at a time reached anchors other than the \
                     rows it is drawn in",
                    block.kind()
                );
                assert_eq!(
                    anchors.last().copied(),
                    Some(block.bottom(&wrapping)),
                    "the last row of a block of {:?} is not the row reached by walking down it",
                    block.kind()
                );

                let mut back = vec![block.bottom(&wrapping)];
                while let Some(above) = block.above(
                    *back.last().expect("the walk back began somewhere"),
                    &wrapping,
                ) {
                    back.push(above);
                }
                back.reverse();

                assert_eq!(
                    anchors,
                    back,
                    "stepping up a block of {:?} a row at a time reached anchors other than the \
                     rows it is drawn in",
                    block.kind()
                );
            }
        }
    }

    #[test]
    fn a_window_into_a_line_longer_than_the_window_draws_the_rows_the_whole_line_draws() {
        let long: String = std::iter::repeat_n(CLUSTERS.replace('\n', " "), 64).collect();
        for source in [format!("{long}\n{long}"), format!(" \t {long}")] {
            let block = Block::new(Kind::ToolResult, source);
            for wrapping in wrappings() {
                let all = block.render(whole(&block), &wrapping);
                let rows = all.rows();
                for start in [0, 1, 2, 3, rows.len() / 2, rows.len().saturating_sub(1)] {
                    let anchor = block.anchor(start, &wrapping);
                    let drawn = block.render(RowWindow::at(anchor, 3), &wrapping);
                    let end = (start + 3).min(rows.len());

                    assert_eq!(
                        rows.get(start.min(rows.len())..end).unwrap_or_default(),
                        drawn.rows(),
                        "a window at row {start} of a block of one long line drew something other \
                         than those rows of the whole of it"
                    );
                }
            }
        }
    }

    #[test]
    fn counting_the_rows_of_a_block_agrees_with_laying_the_whole_of_it_out() {
        for block in fixtures() {
            for wrapping in wrappings() {
                assert_eq!(
                    block.render(whole(&block), &wrapping).rows().len(),
                    block.row_count(&wrapping),
                    "a block of {:?} was counted as a different number of rows than it drew",
                    block.kind()
                );
            }
        }
    }

    #[test]
    fn a_line_wider_than_the_columns_is_drawn_in_several_rows_naming_their_own_bytes() {
        let block = Block::new(Kind::Message(Role::User), "abcdefgh".to_owned());
        let rendered = block.render(whole(&block), &wrapping(3));

        assert_eq!(vec![0..3, 3..6, 6..8], sources(&rendered));
        assert_eq!(Some(0..8), rendered.source());
    }

    #[test]
    fn a_row_names_the_bytes_it_draws_and_the_separators_name_the_rest() {
        let block = Block::new(Kind::Message(Role::User), "ab\n\ncd".to_owned());
        let rendered = block.render(whole(&block), &wrapping(UNWRAPPED));

        assert_eq!(vec![0..2, 3..3, 4..6], sources(&rendered));
        for row in rendered.rows() {
            assert_eq!(
                block
                    .slice(row.source())
                    .expect("a row names a range of the source"),
                row.styled().row().text()
            );
        }
        assert_eq!(
            Some("ab\n\ncd"),
            rendered.source().and_then(|range| block.slice(range))
        );
    }

    #[test]
    fn a_span_names_the_same_source_once_it_has_been_rendered() {
        let source = "the middle of it wraps".to_owned();
        let span = Span::new(4..10, red());
        let block = Block::with_spans(Kind::Message(Role::Assistant), source, vec![span.clone()]);
        let rendered = block.render(whole(&block), &wrapping(WIDTH));

        let inside: Vec<&StyledSegment> = segments(&rendered)
            .filter(|segment| {
                span.range().start <= segment.source().start
                    && segment.source().end <= span.range().end
            })
            .collect();
        assert!(
            1 < inside.len(),
            "the fixture did not wrap the span across rows: {inside:?}"
        );

        let drawn: String = inside
            .iter()
            .map(|segment| {
                block
                    .slice(segment.source().clone())
                    .expect("a segment names a range of the source")
            })
            .collect();
        assert_eq!("middle", drawn);
        for segment in inside {
            assert_eq!(span.style(), segment.style());
        }
    }

    #[test]
    fn ansi_escapes_are_read_as_styles_and_are_absent_from_what_is_rendered() {
        let block = Block::from_ansi(Kind::ToolResult, COLOURED);
        let rendered = block.render(whole(&block), &wrapping(UNWRAPPED));

        assert_eq!("error: nope\ndone", block.source());
        assert_eq!(
            &[Span::new(0..5, red().add_modifier(Modifier::BOLD))],
            block.spans()
        );

        let recovered = rendered
            .source()
            .and_then(|range| block.slice(range))
            .expect("a render names a range of the source");
        assert_eq!(block.source(), recovered);
        assert!(
            !recovered.contains(ESCAPE),
            "an escape survived into the source: {recovered:?}"
        );

        let cells: String = rendered
            .rows()
            .iter()
            .map(|row| row.styled().cells())
            .collect();
        assert_eq!("error: nopedone", cells);
    }

    #[test]
    fn a_diff_round_trips_and_marks_the_lines_it_changed() {
        let block = Block::diff(
            "src/main.rs".to_owned(),
            "fn main() {}\n",
            "fn main() {\n    todo!();\n}\n",
        );
        let rendered = block.render(whole(&block), &wrapping(UNWRAPPED));

        assert_eq!(
            "-fn main() {}\n+fn main() {\n+    todo!();\n+}",
            block.source()
        );
        assert_eq!(
            Some(block.source()),
            rendered.source().and_then(|range| block.slice(range))
        );
        assert_eq!(4, rendered.rows().len());

        let marked: Vec<&str> = block
            .spans()
            .iter()
            .map(|span| {
                block
                    .slice(span.range().clone())
                    .expect("a span names a range of the source")
            })
            .collect();
        assert_eq!(
            vec!["-fn main() {}", "+fn main() {", "+    todo!();", "+}"],
            marked
        );
    }

    #[test]
    fn no_row_or_segment_of_a_wide_and_joined_source_splits_a_cluster() {
        let block = Block::new(Kind::Message(Role::Assistant), CLUSTERS.to_owned());
        let rendered = block.render(whole(&block), &wrapping(4));
        let boundaries = clusters(block.source());

        assert!(
            2 < rendered.rows().len(),
            "the fixture did not wrap: {rendered:?}"
        );
        assert!(
            rendered
                .rows()
                .iter()
                .any(|row| 1 < row.source().end - row.source().start
                    && !block
                        .slice(row.source())
                        .expect("a row names a range of the source")
                        .is_ascii()),
            "no row of the fixture drew a cluster of more than one byte"
        );

        let cut: Vec<usize> = rendered
            .rows()
            .iter()
            .flat_map(|row| [row.source().start, row.source().end])
            .chain(
                segments(&rendered)
                    .flat_map(|segment| [segment.source().start, segment.source().end]),
            )
            .filter(|offset| !boundaries.contains(offset))
            .collect();
        assert_eq!(
            Vec::<usize>::new(),
            cut,
            "a row or a segment was cut inside a cluster of {:?}",
            block.source()
        );

        assert_eq!(
            Some(block.source()),
            rendered.source().and_then(|range| block.slice(range))
        );
        let drawn: String = rendered
            .rows()
            .iter()
            .map(|row| row.styled().row().text())
            .collect();
        assert_eq!(block.source().replace('\n', ""), drawn);
    }

    #[test]
    fn a_span_landing_inside_a_cluster_styles_the_whole_cluster() {
        let joined = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        let source = format!("a{joined}b");
        let inside = 1 + "\u{1f468}".len();
        let block = Block::with_spans(
            Kind::Message(Role::Assistant),
            source.clone(),
            vec![Span::new(inside..inside + 1, red())],
        );

        assert_eq!(
            &[Span::new(1..1 + joined.len(), red())],
            block.spans(),
            "the span did not widen to the cluster it landed inside"
        );

        let rendered = block.render(whole(&block), &wrapping(UNWRAPPED));
        let painted: Vec<&str> = segments(&rendered)
            .filter(|segment| red() == segment.style())
            .map(|segment| {
                block
                    .slice(segment.source().clone())
                    .expect("a segment names a range of the source")
            })
            .collect();

        assert_eq!(vec![joined], painted);
    }

    #[test]
    fn an_empty_block_is_drawn_in_one_empty_row() {
        let block = Block::new(Kind::Message(Role::User), String::new());
        let rendered = block.render(whole(&block), &wrapping(WIDTH));

        assert_eq!(vec![0..0], sources(&rendered));
        assert_eq!(
            Some(""),
            rendered.source().and_then(|range| block.slice(range))
        );
    }

    #[test]
    fn a_source_ending_at_a_separator_keeps_it_and_one_ending_without_one_adds_none() {
        let ended = Block::new(Kind::Message(Role::User), "tail\n".to_owned());
        let bare = Block::new(Kind::Message(Role::User), "tail".to_owned());
        let wrapping = wrapping(UNWRAPPED);

        let rendered = ended.render(whole(&ended), &wrapping);
        assert_eq!(vec![0..4, 5..5], sources(&rendered));
        assert_eq!(
            Some("tail\n"),
            rendered.source().and_then(|range| ended.slice(range))
        );

        let rendered = bare.render(whole(&bare), &wrapping);
        assert_eq!(vec![0..4], sources(&rendered));
        assert_eq!(
            Some("tail"),
            rendered.source().and_then(|range| bare.slice(range))
        );
    }

    #[test]
    fn a_window_draws_the_rows_it_asks_for_and_says_which_they_are() {
        let block = Block::new(
            Kind::Message(Role::User),
            "one\ntwo\nthree\nfour\nfive".to_owned(),
        );
        let wrapping = wrapping(UNWRAPPED);

        let rendered = block.render(RowWindow::new(1, 3), &wrapping);
        assert_eq!(Some(1), rendered.start());
        assert_eq!(vec![4..7, 8..13, 14..18], sources(&rendered));
        assert_eq!(
            Some("two\nthree\nfour"),
            rendered.source().and_then(|range| block.slice(range))
        );

        let drawn: Vec<&str> = rendered
            .rows()
            .iter()
            .map(|row| row.styled().row().text())
            .collect();
        assert_eq!(vec!["two", "three", "four"], drawn);
    }

    #[test]
    fn a_window_starting_inside_a_wrapped_line_starts_at_that_rows_own_bytes() {
        let block = Block::new(Kind::Message(Role::User), "abcdefghij\nnext".to_owned());
        let rendered = block.render(RowWindow::new(1, 2), &wrapping(4));

        assert_eq!(vec![4..8, 8..10], sources(&rendered));
        assert_eq!(
            Some("efghij"),
            rendered.source().and_then(|range| block.slice(range))
        );
    }

    #[test]
    fn a_window_past_the_end_of_a_block_draws_nothing() {
        let block = Block::new(Kind::Message(Role::User), "one\ntwo".to_owned());
        let rendered = block.render(RowWindow::new(9, 4), &wrapping(UNWRAPPED));

        assert_eq!(&[] as &[RenderedRow], rendered.rows());
        assert_eq!(None, rendered.source());
    }

    #[test]
    fn a_window_wider_than_what_is_left_stops_at_the_end_of_the_block() {
        let block = Block::new(Kind::Message(Role::User), "one\ntwo\nthree".to_owned());
        let rendered = block.render(RowWindow::new(2, 8), &wrapping(UNWRAPPED));

        assert_eq!(vec![8..13], sources(&rendered));
    }

    #[test]
    fn a_window_of_no_rows_draws_nothing() {
        let block = Block::new(Kind::Message(Role::User), "one\ntwo".to_owned());
        let rendered = block.render(RowWindow::new(0, 0), &wrapping(UNWRAPPED));

        assert_eq!(&[] as &[RenderedRow], rendered.rows());
    }

    #[test]
    fn a_window_draws_the_rows_the_whole_block_would_have_drawn_there() {
        let block = Block::from_ansi(
            Kind::ToolResult,
            "\u{1b}[31mone two three\u{1b}[0m\nfour five six\nseven eight",
        );
        let wrapping = wrapping(WIDTH);
        let all = block.render(whole(&block), &wrapping);

        for start in 0..all.rows().len() {
            let window = block.render(RowWindow::new(start, 2), &wrapping);
            let wanted = &all.rows()[start..(start + 2).min(all.rows().len())];

            assert_eq!(wanted, window.rows(), "the window at row {start} differs");
        }
    }

    /// # Returns
    ///
    /// One block of each kind, together with the sources a round trip is most likely to lose.
    fn fixtures() -> Vec<Block> {
        vec![
            Block::new(
                Kind::Message(Role::User),
                "wrap the chat panel\nplease".to_owned(),
            ),
            Block::with_spans(
                Kind::Message(Role::Assistant),
                "here is what I did".to_owned(),
                vec![Span::new(8..12, red())],
            ),
            Block::new(
                Kind::Code {
                    language: Some("rust".to_owned()),
                },
                "fn main() {\n\tprintln!(\"hi\");\n}".to_owned(),
            ),
            Block::new(
                Kind::Code { language: None },
                "cargo test -p vbc-editor".to_owned(),
            ),
            Block::new(
                Kind::ToolCall {
                    name: "Bash".to_owned(),
                },
                "cargo clippy --all-targets".to_owned(),
            ),
            Block::from_ansi(Kind::ToolResult, COLOURED),
            Block::new(Kind::Thinking, "the block model comes first".to_owned()),
            Block::diff(
                "src/main.rs".to_owned(),
                "fn main() {}\n",
                "fn main() {\n    todo!();\n}\n",
            ),
            Block::new(Kind::Message(Role::User), String::new()),
            Block::new(
                Kind::Message(Role::User),
                "no separator at the end".to_owned(),
            ),
            Block::new(
                Kind::Message(Role::User),
                "a separator at the end\n".to_owned(),
            ),
            Block::new(Kind::Message(Role::Assistant), CLUSTERS.to_owned()),
            Block::new(Kind::ToolResult, CONTROLS.to_owned()),
        ]
    }

    /// # Returns
    ///
    /// The wrappings the fixtures are drawn under, which vary the width a row holds and every
    /// option that moves where a line breaks, because a count read off a line's length is only
    /// right where the line breaks at the column it runs out of.
    fn wrappings() -> Vec<Wrapping> {
        let options = vec![
            Options::new(),
            Options::new().with_line_break(true),
            Options::new().with_show_break(SHOW_BREAK.to_owned()),
            Options::new()
                .with_break_indent(true)
                .with_break_indent_min(1),
            Options::new()
                .with_line_break(true)
                .with_show_break(SHOW_BREAK.to_owned())
                .with_break_indent(true)
                .with_break_indent_min(1),
        ];

        let mut wrappings = Vec::new();
        for width in [1, 2, 3, WIDTH, 9, UNWRAPPED] {
            for options in &options {
                wrappings.push(Wrapping::new(
                    NonZeroUsize::new(width).expect("a fixture is drawn in at least one column"),
                    Metrics::default(),
                    options.clone(),
                ));
            }
        }

        wrappings
    }

    /// # Returns
    ///
    /// A window wide enough to draw the whole of `block`, which no caller drawing a screen asks
    /// for and which a round trip needs: a row shows at least one byte of the source or is the one
    /// row an empty line is drawn in, so there are never more rows than bytes and lines together.
    fn whole(block: &Block) -> RowWindow {
        RowWindow::new(0, 2 * block.source().len() + 2)
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
    /// The byte range of the source each row of `rendered` was drawn from.
    fn sources(rendered: &Rendered) -> Vec<Range<usize>> {
        rendered.rows().iter().map(RenderedRow::source).collect()
    }

    /// # Returns
    ///
    /// Every styled segment of `rendered`, top to bottom.
    fn segments(rendered: &Rendered) -> impl Iterator<Item = &StyledSegment> {
        rendered
            .rows()
            .iter()
            .flat_map(|row| row.styled().segments())
    }

    /// # Returns
    ///
    /// Every byte offset of `source` a grapheme cluster begins at, the end of the source included,
    /// which are the only offsets a row or a segment may be cut at.
    fn clusters(source: &str) -> BTreeSet<usize> {
        grapheme_indices(source)
            .map(|(offset, _)| offset)
            .chain(std::iter::once(source.len()))
            .collect()
    }

    /// # Returns
    ///
    /// The style the fixtures are drawn in where they are drawn in one.
    fn red() -> Style {
        Style::new().fg(Color::Red)
    }
}
