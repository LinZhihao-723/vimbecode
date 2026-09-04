//! Selecting the text of a block, in the coordinates the block holds it in.
//!
//! Soft wrap is a property of the viewport rather than of the text, and a selection must not see
//! it. Claude writes prose as one long logical line, so a paragraph is drawn in forty rows in a
//! narrow panel and in four in a wide one, and a selection of five lines is the same five logical
//! lines in both. So a selection is held as byte offsets of the block's source, every motion moves
//! through that source, and every count a selection reports counts logical lines.
//!
//! Blockwise is the one place columns come into it, and the columns are the virtual columns of the
//! logical lines, measured as though nothing wrapped. That is what vim takes, and it is why a
//! block over a wrapped paragraph is a block over the unwrapped text of it: a column past the
//! width of the panel is a column no row is wide enough to hold, and it is still the column the
//! selection is taken at.
//!
//! The highlight runs source to screen and only that way. A drawn row names the bytes it shows, so
//! the columns to paint are found by intersecting the rows a caller was handed with the range the
//! selection already covers. Nothing here reads a selection back out of the rows it happens to be
//! drawn in, which is the door the wrapping would come through.
//!
//! What any of this costs is what the selection covers rather than what the block holds: a motion
//! walks the logical lines it crosses and nothing else, and the text, the counts and the segments
//! are read off the range covered. The highlight costs less again, because a panel derives one
//! every frame while a yank happens once: it costs the rows it was handed rather than the
//! selection over them, so selecting the whole of what `cargo` wrote and scrolling through it
//! costs a screenful a frame.

use std::ops::Range;

use vbc_layout::buffer::LINE_SEPARATOR;
use vbc_layout::width::{grapheme_indices, graphemes, Metrics};

use crate::chat::block::{Rendered, RenderedRow};

/// What a selection takes from the source it is over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// `v`: the bytes between the two ends, the grapheme under the moving end included.
    Charwise,

    /// `V`: the whole of every logical line between the two ends.
    Linewise,

    /// `C-v`: the virtual columns between the two ends, taken from every logical line between
    /// them.
    Blockwise,
}

/// How the moving end of a selection is moved.
///
/// The vertical motions cross logical lines, which is what makes `V4j` five lines rather than five
/// rows, and the horizontal ones stay inside the logical line they start in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Motion {
    /// `j`: down the given number of logical lines, however many rows each is drawn in.
    Down(usize),

    /// `k`: up the given number of logical lines.
    Up(usize),

    /// `h`: back the given number of graphemes, no further than the start of the logical line.
    Left(usize),

    /// `l`: forward the given number of graphemes, no further than the last grapheme of the
    /// logical line.
    Right(usize),

    /// `0`: to the start of the logical line.
    LineStart,

    /// `$`: to the last grapheme of the logical line, which is not the last grapheme of the row it
    /// happens to be drawn in.
    LineEnd,
}

/// The source a selection is over, together with the metrics its virtual columns are measured
/// under.
///
/// The width the source is drawn in is deliberately not here. A selection is a range of the source
/// and nothing it answers may depend on where the rows break.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Source<'block> {
    text: &'block str,
    metrics: Metrics,
}

impl<'block> Source<'block> {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A source over `text`, whose virtual columns are measured under `metrics`.
    #[must_use]
    pub fn new(text: &'block str, metrics: Metrics) -> Self {
        Self { text, metrics }
    }

    #[must_use]
    pub fn text(&self) -> &'block str {
        self.text
    }

    #[must_use]
    pub fn metrics(&self) -> Metrics {
        self.metrics
    }
}

/// A selection of a block's source: what it takes, where it was started, and where its moving end
/// has been carried to.
///
/// Both ends are byte offsets of the source, each on a grapheme boundary and never past the last
/// grapheme of the logical line it sits in. The grapheme under the moving end is part of the
/// selection, the way vim's visual selection holds the one under the cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    mode: Mode,
    origin: usize,
    cursor: usize,
    wanted: usize,
}

impl Selection {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A selection of `mode` with both of its ends at `at`, moved back to the nearest grapheme of
    /// the logical line `at` falls in where it names no grapheme of one.
    #[must_use]
    pub fn new(mode: Mode, source: Source<'_>, at: usize) -> Self {
        let at = place(source.text, at);

        Self {
            mode,
            origin: at,
            cursor: at,
            wanted: column_of(source, at),
        }
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A selection of `mode` anchored at `origin` with its moving end at `at`, each moved back to
    /// the nearest grapheme of the logical line it falls in where it names no grapheme of one.
    #[must_use]
    pub fn between(mode: Mode, source: Source<'_>, origin: usize, at: usize) -> Self {
        let mut selection = Self::new(mode, source, origin);
        selection.cursor = place(source.text, at);
        selection.wanted = column_of(source, selection.cursor);

        selection
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    #[must_use]
    pub fn origin(&self) -> usize {
        self.origin
    }

    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Takes the selection as `mode` instead, leaving both of its ends where they are.
    pub fn switch(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// Carries the moving end of the selection along `motion`.
    ///
    /// A motion that would leave the source stops at the last logical line it can reach rather
    /// than failing, which is what a panel over a transcript wants of one.
    pub fn extend(&mut self, source: Source<'_>, motion: Motion) {
        let text = source.text;
        match motion {
            Motion::Down(count) => {
                let mut start = line_start(text, self.cursor);
                for _ in 0..count {
                    let end = line_end(text, start);
                    if text.len() <= end {
                        break;
                    }
                    start = end + LINE_SEPARATOR.len_utf8();
                }
                self.cursor = column_at(source, start..line_end(text, start), self.wanted);
            }
            Motion::Up(count) => {
                let mut start = line_start(text, self.cursor);
                for _ in 0..count {
                    if 0 == start {
                        break;
                    }
                    start = line_start(text, start - LINE_SEPARATOR.len_utf8());
                }
                self.cursor = column_at(source, start..line_end(text, start), self.wanted);
            }
            Motion::Left(count) => {
                let start = line_start(text, self.cursor);
                let offsets: Vec<usize> = grapheme_indices(&text[start..self.cursor])
                    .map(|(offset, _)| start + offset)
                    .collect();
                self.cursor = match offsets.len().checked_sub(count) {
                    Some(index) => offsets.get(index).copied().unwrap_or(self.cursor),
                    None => start,
                };
                self.wanted = column_of(source, self.cursor);
            }
            Motion::Right(count) => {
                let end = line_end(text, self.cursor);
                let mut moved = self.cursor;
                for (steps, (offset, _)) in grapheme_indices(&text[self.cursor..end]).enumerate() {
                    if count < steps {
                        break;
                    }
                    moved = self.cursor + offset;
                }
                self.cursor = moved;
                self.wanted = column_of(source, self.cursor);
            }
            Motion::LineStart => {
                self.cursor = line_start(text, self.cursor);
                self.wanted = 0;
            }
            Motion::LineEnd => {
                self.cursor = place(text, line_end(text, self.cursor));
                self.wanted = column_of(source, self.cursor);
            }
        }
    }

    /// # Returns
    ///
    /// The byte ranges of the source the selection takes: one for a charwise selection, and one
    /// for each logical line it covers for a linewise or a blockwise one, top to bottom. A
    /// blockwise range is empty on a logical line that is not drawn as far as the columns taken.
    #[must_use]
    pub fn segments(&self, source: Source<'_>) -> Vec<Range<usize>> {
        let text = source.text;
        let (first, last) = self.ends();
        match self.mode {
            Mode::Charwise => {
                let taken = first..past(text, last);
                vec![taken]
            }
            Mode::Linewise => covered_lines(text, first, last),
            Mode::Blockwise => {
                let window = self.window(source);
                covered_lines(text, first, last)
                    .into_iter()
                    .map(|line| cut(source, line, &window))
                    .collect()
            }
        }
    }

    /// # Returns
    ///
    /// The byte range of the source from the start of the first of [`Selection::segments`] to the
    /// end of the last, which for a blockwise selection spans the logical lines it covers rather
    /// than naming what it takes from them.
    ///
    /// # Panics
    ///
    /// Panics if the selection covers no logical line, which no selection does.
    #[must_use]
    pub fn range(&self, source: Source<'_>) -> Range<usize> {
        let segments = self.segments(source);
        let first = segments.first().expect("a selection covers a logical line");
        let last = segments.last().expect("a selection covers a logical line");

        first.start..last.end
    }

    /// # Returns
    ///
    /// The text the selection takes, its segments separated by [`LINE_SEPARATOR`].
    ///
    /// # Panics
    ///
    /// Panics if a segment is not a range of the source, which no segment is.
    #[must_use]
    pub fn text(&self, source: Source<'_>) -> String {
        let mut text = String::new();
        for (index, segment) in self.segments(source).into_iter().enumerate() {
            if 0 < index {
                text.push(LINE_SEPARATOR);
            }
            text.push_str(
                source
                    .text
                    .get(segment)
                    .expect("a segment names a range of the source"),
            );
        }

        text
    }

    /// # Returns
    ///
    /// The number of logical lines the selection covers, which is what every count it reports
    /// counts however many rows those lines are drawn in.
    ///
    /// # Panics
    ///
    /// Panics if an end of the selection is not a byte offset of the source, which neither is.
    #[must_use]
    pub fn lines(&self, source: Source<'_>) -> usize {
        let (first, last) = self.ends();
        let crossed = source
            .text
            .get(first..last)
            .expect("an end of a selection is an offset of the source");

        1 + crossed.matches(LINE_SEPARATOR).count()
    }

    /// Derives the highlight of the selection over the rows a block was drawn in.
    ///
    /// The rows are read for the bytes they name and for the columns they drew them at, and what
    /// is painted is the part of each row the selection already covers. Nothing here decides what
    /// is selected, which is why wrapping a line differently changes the picture and never the
    /// text.
    ///
    /// What this costs is the rows it is handed rather than the selection over them: a row is
    /// answered from the logical line it shows, and a run of rows continuing one line reads that
    /// line once. A selection of a hundred thousand lines is therefore painted into a screenful
    /// for what a selection of one costs, which `chat_selection_cost.rs` measures rather than
    /// argues.
    ///
    /// # Returns
    ///
    /// The columns to paint, one entry for each row of `rendered` the selection reaches, in the
    /// order the rows are drawn in. A row the selection reaches and takes nothing from is absent.
    ///
    /// # Panics
    ///
    /// Panics if a row carries fewer columns than the graphemes it shows, which no laid-out row
    /// does.
    #[must_use]
    pub fn highlight(&self, source: Source<'_>, rendered: &Rendered) -> Vec<RowHighlight> {
        self.painted(source, rendered.rows())
    }

    /// Derives the highlight of the selection over rows a caller drew itself.
    ///
    /// This is [`Selection::highlight`] over rows that reached the caller one at a time rather
    /// than as the window a block was rendered into, which is what a panel drawing several blocks
    /// down one screen has: the rows of one block arrive interleaved with summaries and with the
    /// rows of its neighbours, and only the caller knows which of them belong to the block the
    /// selection is over.
    ///
    /// # Type Parameters
    ///
    /// * `RowsType` - The rows of the block the selection is over, in the order they are drawn.
    ///
    /// # Returns
    ///
    /// The columns to paint, one entry for each row of `rows` the selection reaches, numbered by
    /// that row's place among the rows given. A row the selection reaches and takes nothing from
    /// is absent.
    ///
    /// # Panics
    ///
    /// Panics if a row carries fewer columns than the graphemes it shows, which no laid-out row
    /// does.
    #[must_use]
    pub fn painted<'row, RowsType>(&self, source: Source<'_>, rows: RowsType) -> Vec<RowHighlight>
    where
        RowsType: IntoIterator<Item = &'row RenderedRow>,
    {
        let text = source.text;
        let (first, last) = self.ends();
        let covered = line_start(text, first)..line_start(text, last);
        let window = (Mode::Blockwise == self.mode).then(|| self.window(source));

        let mut highlights = Vec::new();
        let mut held: Option<(Range<usize>, Range<usize>)> = None;
        for (index, row) in rows.into_iter().enumerate() {
            let drawn = row.source();
            let continues = |(line, _): &(Range<usize>, Range<usize>)| {
                line.start <= drawn.start && drawn.end <= line.end
            };
            if !held.as_ref().is_some_and(continues) {
                let line = line_start(text, drawn.start)..line_end(text, drawn.start);
                let taken = self.taken(source, line.clone(), window.as_ref());
                held = Some((line, taken));
            }

            let (line, taken) = held.as_ref().expect("a drawn row shows a logical line");
            if line.start < covered.start || covered.end < line.start {
                continue;
            }

            let start = drawn.start.max(taken.start);
            let end = drawn.end.min(taken.end);
            let bare = drawn.start == drawn.end && taken.start == taken.end;
            if start < end || (bare && drawn.start == taken.start) {
                highlights.push(RowHighlight {
                    row: index,
                    source: start..end,
                    columns: columns_of(row, start - drawn.start..end - drawn.start),
                });
            }
        }

        highlights
    }

    /// # Returns
    ///
    /// The byte range of the logical line `line` the selection takes, which is what
    /// [`Selection::segments`] would name for that line, `window` being the virtual columns a
    /// blockwise selection takes.
    ///
    /// # Panics
    ///
    /// Panics if a blockwise selection is asked without the columns it takes, which no caller
    /// does.
    fn taken(
        &self,
        source: Source<'_>,
        line: Range<usize>,
        window: Option<&Range<usize>>,
    ) -> Range<usize> {
        let (first, last) = self.ends();
        match self.mode {
            Mode::Charwise => first..past(source.text, last),
            Mode::Linewise => line,
            Mode::Blockwise => cut(
                source,
                line,
                window.expect("a blockwise selection is asked with the columns it takes"),
            ),
        }
    }

    /// # Returns
    ///
    /// The two ends of the selection, the earlier of them first.
    fn ends(&self) -> (usize, usize) {
        (self.origin.min(self.cursor), self.origin.max(self.cursor))
    }

    /// # Returns
    ///
    /// The virtual columns a blockwise selection takes, measured on the logical lines its ends sit
    /// in rather than on the rows they are drawn in.
    fn window(&self, source: Source<'_>) -> Range<usize> {
        let origin = span(source, self.origin);
        let cursor = span(source, self.cursor);

        origin.start.min(cursor.start)..origin.end.max(cursor.end)
    }
}

/// The columns of one drawn row a selection paints, together with the bytes of the source they
/// were derived from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RowHighlight {
    row: usize,
    source: Range<usize>,
    columns: Range<usize>,
}

impl RowHighlight {
    /// # Returns
    ///
    /// The index, among the rows the window was drawn in, of the row this paints, which is the row
    /// of the block itself only where the window began at the top of it.
    #[must_use]
    pub fn row(&self) -> usize {
        self.row
    }

    /// # Returns
    ///
    /// The byte range of the block's source the painted columns show.
    #[must_use]
    pub fn source(&self) -> &Range<usize> {
        &self.source
    }

    /// # Returns
    ///
    /// The columns of the row to paint, the row's own decoration accounted for.
    #[must_use]
    pub fn columns(&self) -> &Range<usize> {
        &self.columns
    }
}

/// # Returns
///
/// The byte offset the logical line holding `at` starts at.
fn line_start(text: &str, at: usize) -> usize {
    text.as_bytes()[..at]
        .iter()
        .rposition(|byte| LINE_SEPARATOR == char::from(*byte))
        .map_or(0, |offset| offset + LINE_SEPARATOR.len_utf8())
}

/// # Returns
///
/// The byte offset just past the logical line holding `at`, its separator excluded.
fn line_end(text: &str, at: usize) -> usize {
    text.as_bytes()[at..]
        .iter()
        .position(|byte| LINE_SEPARATOR == char::from(*byte))
        .map_or(text.len(), |offset| at + offset)
}

/// # Returns
///
/// The byte ranges of the logical lines from the one holding `first` to the one holding `last`,
/// their separators excluded, top to bottom.
fn covered_lines(text: &str, first: usize, last: usize) -> Vec<Range<usize>> {
    let stop = line_start(text, last);
    let mut lines = Vec::new();
    let mut start = line_start(text, first);
    loop {
        let end = line_end(text, start);
        lines.push(start..end);
        if stop <= start {
            break;
        }
        start = end + LINE_SEPARATOR.len_utf8();
    }

    lines
}

/// # Returns
///
/// `at` moved back to the grapheme of its logical line it falls in, to the last grapheme of that
/// line where it falls past one, or to the start of an empty line.
fn place(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    let start = line_start(text, at);
    let end = line_end(text, at);
    let mut placed = start;
    for (offset, _) in grapheme_indices(&text[start..end]) {
        if at < start + offset {
            break;
        }
        placed = start + offset;
    }

    placed
}

/// # Returns
///
/// The byte offset just past the grapheme at `at`, which is `at` itself at the end of a logical
/// line.
fn past(text: &str, at: usize) -> usize {
    let end = line_end(text, at);

    graphemes(&text[at..end])
        .next()
        .map_or(at, |grapheme| at + grapheme.len())
}

/// # Returns
///
/// The virtual column of its logical line that the grapheme at `at` starts in.
fn column_of(source: Source<'_>, at: usize) -> usize {
    let start = line_start(source.text, at);

    source.metrics.text_width(&source.text[start..at], 0)
}

/// # Returns
///
/// The virtual columns the grapheme at `at` occupies, which is empty at the end of a logical line.
fn span(source: Source<'_>, at: usize) -> Range<usize> {
    let column = column_of(source, at);
    let end = line_end(source.text, at);
    let width = graphemes(&source.text[at..end])
        .next()
        .map_or(0, |grapheme| {
            source.metrics.grapheme_width(grapheme, column)
        });

    column..column + width
}

/// # Returns
///
/// The byte offset within the logical line `line` of the grapheme drawn in virtual `column`, or of
/// its last grapheme where the line is not drawn that far.
fn column_at(source: Source<'_>, line: Range<usize>, column: usize) -> usize {
    let mut placed = line.start;
    let mut at = 0;
    for (offset, grapheme) in grapheme_indices(&source.text[line.clone()]) {
        let width = source.metrics.grapheme_width(grapheme, at);
        if column < at + width {
            return line.start + offset;
        }
        at += width;
        placed = line.start + offset;
    }

    placed
}

/// # Returns
///
/// The byte range of the logical line `line` drawn in the virtual columns `window`, which is empty
/// where the line does not reach them.
fn cut(source: Source<'_>, line: Range<usize>, window: &Range<usize>) -> Range<usize> {
    let mut taken: Option<Range<usize>> = None;
    let mut at = 0;
    for (offset, grapheme) in grapheme_indices(&source.text[line.clone()]) {
        let width = source.metrics.grapheme_width(grapheme, at);
        if at < window.end && window.start < at + width {
            let start = taken.map_or(line.start + offset, |range| range.start);
            taken = Some(start..line.start + offset + grapheme.len());
        }
        at += width;
    }

    taken.unwrap_or(line.end..line.end)
}

/// # Returns
///
/// The columns of `row` that the bytes `bytes` of its own text are drawn in.
///
/// # Panics
///
/// Panics if `row` carries fewer columns than the graphemes it shows, which no laid-out row does.
fn columns_of(row: &RenderedRow, bytes: Range<usize>) -> Range<usize> {
    let drawn = row.styled().row();
    let columns = drawn.columns();
    let column = |at: usize| {
        *columns
            .get(index_of(drawn.text(), at))
            .expect("a row carries a column for each of its graphemes and one past them")
    };

    column(bytes.start)..column(bytes.end)
}

/// # Returns
///
/// The number of graphemes of `text` that start before `at`, which is the index of the grapheme
/// starting at `at`.
fn index_of(text: &str, at: usize) -> usize {
    grapheme_indices(text)
        .take_while(|(offset, _)| *offset < at)
        .count()
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::ops::Range;

    use vbc_layout::anchor::Wrapping;
    use vbc_layout::line::{self, DisplayRow, Options};
    use vbc_layout::width::Metrics;

    use crate::chat::block::{Block, Kind, Rendered, Role, RowWindow};

    use super::{Mode, Motion, RowHighlight, Selection, Source};

    /// The columns the fixtures are drawn in, narrow enough that a paragraph of one is drawn in
    /// ten rows or more.
    const WIDTH: usize = 20;

    /// The rows the headline invariant asks a paragraph of the fixture to be drawn in at least.
    const WRAPPED: usize = 10;

    /// The rows a paragraph of the fixture is in fact drawn in at [`WIDTH`] columns, which the
    /// fixture is checked to be drawn in so that the rows a selection is painted into are an exact
    /// number rather than a floor a highlight painting the whole block would also clear.
    const ROWS: usize = 13;

    /// The logical lines `V4j` covers.
    const SELECTED: usize = 5;

    /// A column well inside a paragraph, and well past the width of the panel, so that no row of
    /// the panel is wide enough to hold it.
    const INSIDE: usize = 100;

    /// The marker a continuation row carries where a fixture is drawn with one, which puts the
    /// columns of a row out of step with the virtual columns of the line it continues.
    const SHOW_BREAK: &str = "++ ";

    /// The columns a marked panel paints the block taken at [`INSIDE`] in, which are nowhere near
    /// [`INSIDE`] itself: the first row of a line holds twenty columns of it and every row after
    /// that holds seventeen beside the marker, so the hundredth column of the line is reached five
    /// rows down and fifteen columns across.
    const PAINTED: Range<usize> = 15..WIDTH;

    /// The prose a paragraph of the fixture is padded out with, long enough that the paragraph
    /// wraps many times over.
    const PROSE: &str = "wrap me around the panel and back again ";

    /// The words the paragraphs of the fixture are told apart by, which are of different lengths
    /// so that the same column of two of them holds different text.
    const ORDINALS: [&str; 8] = [
        "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth",
    ];

    /// A line far too short to reach the column a selection above it was heading for.
    const SHORT: &str = "short";

    /// Lines of ASCII and of CJK, whose bytes, graphemes and columns all disagree.
    const WIDE: &str = "abcdefghij\n\u{4e2d}\u{6587}\u{4e2d}\u{6587}\u{4e2d}ghij\nklmnopqrst";

    /// A line of nothing but wide graphemes, so that an end of a selection carried onto it is
    /// drawn in two columns rather than in one.
    const WIDE_ONLY: &str = "\u{4e2d}\u{6587}\u{4e2d}";

    #[test]
    fn a_linewise_selection_four_lines_down_takes_five_logical_lines_however_they_wrap() {
        let text = transcript(8);
        let source = over(&text);
        const {
            assert!(
                WRAPPED <= ROWS,
                "a paragraph of the fixture is not drawn in ten rows or more"
            );
        }
        for index in 0..SELECTED {
            assert_eq!(
                ROWS,
                rows_of(&paragraph(index)).len(),
                "paragraph {index} is not drawn in {ROWS} rows, so the fixture does not wrap"
            );
        }

        let mut selection = Selection::new(Mode::Linewise, source, 0);
        selection.extend(source, Motion::Down(4));

        let taken = selection.text(source);
        let wanted: Vec<String> = (0..SELECTED).map(paragraph).collect();
        assert_eq!(wanted.join("\n"), taken);
        assert_eq!(SELECTED - 1, taken.matches('\n').count());
        assert_eq!(SELECTED, selection.lines(source));
        assert_eq!(SELECTED, selection.segments(source).len());

        let painted = selection.highlight(source, &drawn(&text, Options::new(), WIDTH));
        assert_eq!(
            SELECTED * ROWS,
            painted.len(),
            "{SELECTED} logical lines were painted in {} rows rather than in {}",
            painted.len(),
            SELECTED * ROWS
        );
    }

    #[test]
    fn a_charwise_selection_across_a_wrap_boundary_holds_no_separator() {
        let text = paragraph(0);
        let source = over(&text);
        let rows = rows_of(&text);
        assert!(
            text.is_ascii(),
            "a column of the fixture is not one of its bytes"
        );
        assert!(
            WRAPPED <= rows.len(),
            "the fixture is drawn in {} rows, so nothing of it wraps",
            rows.len()
        );

        let from = rows[1].start();
        let to = rows[4].start() + 2;
        let mut selection = Selection::new(Mode::Charwise, source, from);
        selection.extend(source, Motion::Right(to - from));

        assert_eq!(to, selection.cursor());
        let taken = selection.text(source);
        assert_eq!(
            0,
            taken.matches('\n').count(),
            "a selection inside one logical line held a separator: {taken:?}"
        );
        assert_eq!(1, selection.lines(source));
        assert_eq!(&text[from..=to], taken);

        let painted = selection.highlight(source, &drawn(&text, Options::new(), WIDTH));
        assert_eq!(
            vec![1, 2, 3, 4],
            painted
                .iter()
                .map(RowHighlight::row)
                .collect::<Vec<usize>>(),
            "the selection did not cross a wrap boundary"
        );
    }

    #[test]
    fn a_charwise_selection_crossing_a_separator_holds_exactly_one() {
        let text = transcript(3);
        let source = over(&text);
        let mut selection = Selection::new(Mode::Charwise, source, INSIDE);
        selection.extend(source, Motion::Down(1));

        let taken = selection.text(source);
        assert_eq!(
            1,
            taken.matches('\n').count(),
            "a selection across one separator held another: {taken:?}"
        );
        assert_eq!(2, selection.lines(source));

        let tail = &paragraph(0)[INSIDE..];
        let head = &paragraph(1)[..=INSIDE];
        assert_eq!(format!("{tail}\n{head}"), taken);

        let painted = selection.highlight(source, &drawn(&text, Options::new(), WIDTH));
        assert!(
            WRAPPED <= painted.len(),
            "two logical lines were painted in {} rows, which is not what the fixture draws",
            painted.len()
        );
    }

    #[test]
    fn a_blockwise_selection_takes_virtual_columns_of_logical_lines_rather_than_of_rows() {
        let text = transcript(3);
        let source = over(&text);
        let taken_columns = 5;
        const {
            assert!(
                WIDTH < INSIDE,
                "the column taken is one a row of the panel is wide enough to hold"
            );
        }

        let mut selection = Selection::new(Mode::Blockwise, source, INSIDE);
        selection.extend(source, Motion::Down(2));
        selection.extend(source, Motion::Right(taken_columns - 1));

        let wanted: Vec<String> = (0..3)
            .map(|index| paragraph(index)[INSIDE..INSIDE + taken_columns].to_owned())
            .collect();
        assert_eq!(wanted, slices(&text, &selection.segments(source)));
        assert_eq!(3, selection.lines(source));
        assert!(
            wanted[0] != wanted[1] && wanted[1] != wanted[2],
            "the fixture holds the same text at that column in every line: {wanted:?}"
        );

        let marked = Options::new().with_show_break(SHOW_BREAK.to_owned());
        let painted = selection.highlight(source, &drawn(&text, marked, WIDTH));
        assert_eq!(
            vec![PAINTED; 3],
            painted
                .iter()
                .map(|highlight| highlight.columns().clone())
                .collect::<Vec<Range<usize>>>()
        );
        assert_ne!(
            INSIDE..INSIDE + taken_columns,
            PAINTED,
            "the columns painted are the virtual columns, so the two agree and prove nothing"
        );
        assert_eq!(
            wanted,
            painted
                .iter()
                .map(|highlight| text
                    .get(highlight.source().clone())
                    .expect("a highlight names a range of the source")
                    .to_owned())
                .collect::<Vec<String>>()
        );
    }

    #[test]
    fn a_blockwise_selection_takes_the_columns_a_line_is_drawn_in_rather_than_the_bytes_it_holds() {
        let source = over(WIDE);
        let mut selection = Selection::new(Mode::Blockwise, source, 4);
        selection.extend(source, Motion::Down(2));
        selection.extend(source, Motion::Right(3));

        let segments = selection.segments(source);
        assert_eq!(
            vec![
                "efgh".to_owned(),
                "\u{4e2d}\u{6587}".to_owned(),
                "opqr".to_owned(),
            ],
            slices(WIDE, &segments)
        );
        let wide = WIDE
            .find('\u{4e2d}')
            .expect("the fixture holds a line of wide graphemes");
        assert_ne!(
            wide + 4..wide + 8,
            segments[1],
            "the block was taken at the bytes of the line rather than at its columns"
        );
        assert_eq!(3, selection.lines(source));
    }

    #[test]
    fn a_blockwise_selection_reaches_the_far_side_of_a_wide_grapheme_it_ends_on() {
        let text = format!("abcdefghij\n{WIDE_ONLY}");
        let source = over(&text);
        let wide = text
            .find('\u{6587}')
            .expect("the fixture holds a line of wide graphemes");

        let mut downward = Selection::new(Mode::Blockwise, source, 0);
        downward.extend(source, Motion::Down(1));
        downward.extend(source, Motion::Right(1));
        assert_eq!(
            vec!["abcd".to_owned(), "\u{4e2d}\u{6587}".to_owned()],
            slices(&text, &downward.segments(source)),
            "the block stopped inside the wide grapheme its moving end is drawn in"
        );

        let mut upward = Selection::new(Mode::Blockwise, source, wide);
        upward.extend(source, Motion::Up(1));
        assert_eq!(
            vec!["cd".to_owned(), "\u{6587}".to_owned()],
            slices(&text, &upward.segments(source)),
            "the block stopped inside the wide grapheme it was started in"
        );
    }

    #[test]
    fn a_blockwise_selection_takes_nothing_from_a_line_that_is_not_drawn_that_far() {
        let text = format!("{}\n{SHORT}\n{}", paragraph(0), paragraph(1));
        let source = over(&text);
        let mut selection = Selection::new(Mode::Blockwise, source, INSIDE);
        selection.extend(source, Motion::Down(2));

        let segments = selection.segments(source);
        assert_eq!(
            vec![
                paragraph(0)[INSIDE..=INSIDE].to_owned(),
                String::new(),
                paragraph(1)[INSIDE..=INSIDE].to_owned(),
            ],
            slices(&text, &segments)
        );

        let painted = selection.highlight(source, &drawn(&text, Options::new(), WIDTH));
        assert_eq!(2, painted.len(), "a line taking nothing was painted");
    }

    #[test]
    fn every_count_a_selection_reports_counts_logical_lines() {
        let text = transcript(8);
        let source = over(&text);
        let rendered = drawn(&text, Options::new(), WIDTH);

        for mode in [Mode::Charwise, Mode::Linewise, Mode::Blockwise] {
            let mut selection = Selection::new(mode, source, 0);
            selection.extend(source, Motion::Down(4));

            assert_eq!(SELECTED, selection.lines(source), "{mode:?} counted rows");
            if Mode::Charwise != mode {
                assert_eq!(SELECTED, selection.segments(source).len(), "{mode:?}");
            }

            let painted = selection.highlight(source, &rendered);
            let first = painted.first().expect("the selection paints a row");
            let last = painted.last().expect("the selection paints a row");
            let rows = last.row() - first.row() + 1;
            // Only a linewise selection takes the whole of the last line it covers; the other two
            // stop at the column the moving end was carried to, which is the first row of it.
            let wanted = match mode {
                Mode::Linewise => SELECTED * ROWS,
                _ => (SELECTED - 1) * ROWS + 1,
            };
            assert_eq!(
                0,
                first.row(),
                "{mode:?} did not start at the top of the block"
            );
            assert_eq!(
                wanted, rows,
                "{mode:?} covered {SELECTED} logical lines painted into {rows} rows rather than \
                 into {wanted}"
            );
        }
    }

    #[test]
    fn the_same_selection_takes_the_same_text_however_narrow_the_panel_is() {
        let text = transcript(6);
        let source = over(&text);
        let mut selection = Selection::new(Mode::Linewise, source, 0);
        selection.extend(source, Motion::Down(4));
        let taken = selection.text(source);

        let mut rows = Vec::new();
        for width in [WIDTH, 40, 200] {
            let rendered = drawn(&text, Options::new(), width);
            let painted = selection.highlight(source, &rendered);
            let shown: String = painted
                .iter()
                .map(|highlight| {
                    text.get(highlight.source().clone())
                        .expect("a highlight names a range of the source")
                })
                .collect();

            assert_eq!(taken.replace('\n', ""), shown, "at {width} columns");
            assert_eq!(SELECTED, selection.lines(source), "at {width} columns");
            rows.push(painted.len());
        }

        assert!(
            rows[2] < rows[1] && rows[1] < rows[0],
            "the three widths drew the selection in {rows:?} rows, so none of them wrapped it \
             differently"
        );
    }

    #[test]
    fn the_highlight_of_a_scrolled_window_names_the_rows_of_that_window_and_the_same_bytes() {
        let text = transcript(3);
        let source = over(&text);
        let mut selection = Selection::new(Mode::Linewise, source, 0);
        selection.extend(source, Motion::Down(2));

        let whole = drawn(&text, Options::new(), WIDTH);
        let scrolled = window(&text, WIDTH, WRAPPED, WRAPPED / 2);
        assert_eq!(Some(WRAPPED), scrolled.start(), "the window did not scroll");
        assert_eq!(
            WRAPPED / 2,
            scrolled.rows().len(),
            "the window did not fill"
        );

        let painted = selection.highlight(source, &whole);
        let below = selection.highlight(source, &scrolled);
        assert_eq!(
            (0..WRAPPED / 2).collect::<Vec<usize>>(),
            below.iter().map(RowHighlight::row).collect::<Vec<usize>>(),
            "a scrolled window was painted at the rows of the block rather than at its own"
        );
        assert_eq!(
            painted[WRAPPED..WRAPPED + WRAPPED / 2]
                .iter()
                .map(|highlight| (highlight.source().clone(), highlight.columns().clone()))
                .collect::<Vec<(Range<usize>, Range<usize>)>>(),
            below
                .iter()
                .map(|highlight| (highlight.source().clone(), highlight.columns().clone()))
                .collect::<Vec<(Range<usize>, Range<usize>)>>(),
            "scrolling the window changed the bytes or the columns the selection painted"
        );
    }

    #[test]
    fn the_highlight_paints_a_continuation_row_from_the_column_its_marker_leaves() {
        let text = paragraph(0);
        let source = over(&text);
        let selection = Selection::new(Mode::Linewise, source, 0);
        let marked = Options::new().with_show_break(SHOW_BREAK.to_owned());
        let painted = selection.highlight(source, &drawn(&text, marked, WIDTH));

        assert!(WRAPPED <= painted.len(), "the fixture does not wrap");
        assert_eq!(&(0..WIDTH), painted[0].columns());
        for highlight in &painted[1..] {
            assert_eq!(
                SHOW_BREAK.len(),
                highlight.columns().start,
                "a continuation row was painted from under its own marker"
            );
        }
    }

    #[test]
    fn a_motion_to_the_end_of_the_line_reaches_the_end_of_the_logical_line_rather_than_of_a_row() {
        let text = paragraph(0);
        let source = over(&text);
        assert!(WRAPPED <= rows_of(&text).len(), "the fixture does not wrap");

        let mut selection = Selection::new(Mode::Charwise, source, 0);
        selection.extend(source, Motion::LineEnd);

        assert_eq!(text.len() - 1, selection.cursor());
        assert_eq!(text, selection.text(source));
        assert_eq!(1, selection.lines(source));

        selection.extend(source, Motion::LineStart);
        assert_eq!(0, selection.cursor());
        assert_eq!(&text[..1], selection.text(source));
    }

    #[test]
    fn a_selection_moved_over_a_short_line_comes_back_to_the_column_it_wanted() {
        let text = format!("{}\n{SHORT}\n{}", paragraph(0), paragraph(1));
        let source = over(&text);
        let mut selection = Selection::new(Mode::Charwise, source, INSIDE);

        selection.extend(source, Motion::Down(1));
        assert_eq!(paragraph(0).len() + SHORT.len(), selection.cursor());

        selection.extend(source, Motion::Down(1));
        assert_eq!(
            paragraph(0).len() + SHORT.len() + 2 + INSIDE,
            selection.cursor()
        );

        selection.extend(source, Motion::Up(2));
        assert_eq!(INSIDE, selection.cursor());
    }

    #[test]
    fn a_motion_past_the_last_logical_line_stops_at_it() {
        let text = transcript(3);
        let source = over(&text);
        let mut selection = Selection::new(Mode::Linewise, source, 0);
        selection.extend(source, Motion::Down(40));

        assert_eq!(3, selection.lines(source));
        assert_eq!(text, selection.text(source));

        selection.extend(source, Motion::Up(40));
        assert_eq!(1, selection.lines(source));
        assert_eq!(paragraph(0), selection.text(source));
    }

    #[test]
    fn a_horizontal_motion_stops_at_the_ends_of_the_logical_line_it_starts_in() {
        let text = transcript(3);
        let source = over(&text);
        let start = paragraph(0).len() + 1;
        let mut selection = Selection::new(Mode::Charwise, source, start + INSIDE);

        selection.extend(source, Motion::Right(9_999));
        assert_eq!(start + paragraph(1).len() - 1, selection.cursor());

        selection.extend(source, Motion::Left(9_999));
        assert_eq!(start, selection.cursor());
        assert_eq!(1, selection.lines(source));
    }

    #[test]
    fn a_position_inside_a_cluster_is_taken_at_the_cluster_it_falls_in() {
        let source = over(WIDE);
        let inside = WIDE
            .find('\u{4e2d}')
            .expect("the fixture holds a wide line")
            + 4;
        let selection = Selection::new(Mode::Charwise, source, inside);

        assert_eq!(inside - 1, selection.cursor());
        assert_eq!("\u{6587}", selection.text(source));
    }

    #[test]
    fn the_ends_of_a_selection_survive_a_change_of_mode() {
        let text = transcript(3);
        let source = over(&text);
        let mut selection = Selection::new(Mode::Charwise, source, INSIDE);
        selection.extend(source, Motion::Down(1));
        let ends = (selection.origin(), selection.cursor());

        selection.switch(Mode::Linewise);
        assert_eq!(ends, (selection.origin(), selection.cursor()));
        assert_eq!(Mode::Linewise, selection.mode());
        assert_eq!(2, selection.lines(source));
        assert_eq!(
            format!("{}\n{}", paragraph(0), paragraph(1)),
            selection.text(source)
        );
    }

    #[test]
    fn an_empty_block_selects_nothing_and_still_covers_one_logical_line() {
        let source = over("");
        let mut selection = Selection::new(Mode::Linewise, source, 7);
        selection.extend(source, Motion::Down(3));

        assert_eq!(0, selection.cursor());
        assert_eq!(1, selection.lines(source));
        assert_eq!("", selection.text(source));
        assert_eq!(0..0, selection.range(source));

        let painted = selection.highlight(source, &drawn("", Options::new(), WIDTH));
        let bare = 0..0;
        assert_eq!(
            vec![bare],
            painted
                .iter()
                .map(|highlight| highlight.columns().clone())
                .collect::<Vec<Range<usize>>>()
        );
    }

    /// # Returns
    ///
    /// A source over `text`, measured under the defaults vim measures under.
    fn over(text: &str) -> Source<'_> {
        Source::new(text, Metrics::default())
    }

    /// # Returns
    ///
    /// The paragraph told apart by the `index`th ordinal, long enough to wrap many times over in a
    /// panel of [`WIDTH`] columns.
    fn paragraph(index: usize) -> String {
        format!("{} paragraph: {}", ORDINALS[index], PROSE.repeat(6))
    }

    /// # Returns
    ///
    /// `count` paragraphs, one logical line each.
    fn transcript(count: usize) -> String {
        let mut text = String::new();
        for index in 0..count {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&paragraph(index));
        }

        text
    }

    /// # Returns
    ///
    /// The rows one logical line is drawn in at [`WIDTH`] columns under vim's own defaults.
    fn rows_of(line: &str) -> Vec<DisplayRow> {
        line::lay_out(
            0,
            line,
            NonZeroUsize::new(WIDTH).expect("a fixture is drawn in at least one column"),
            Metrics::default(),
            &Options::new(),
        )
    }

    /// # Returns
    ///
    /// The whole of `text` drawn as one block, in a panel `width` columns wide, under `options`.
    fn drawn(text: &str, options: Options, width: usize) -> Rendered {
        rendered(text, options, width, RowWindow::new(0, 2 * text.len() + 2))
    }

    /// # Returns
    ///
    /// `rows` rows of `text` drawn as one block from its row `start`, in a panel `width` columns
    /// wide, under vim's own defaults.
    fn window(text: &str, width: usize, start: usize, rows: usize) -> Rendered {
        rendered(text, Options::new(), width, RowWindow::new(start, rows))
    }

    /// # Returns
    ///
    /// The rows of `window` of `text` drawn as one block, in a panel `width` columns wide, under
    /// `options`.
    fn rendered(text: &str, options: Options, width: usize, window: RowWindow) -> Rendered {
        let block = Block::new(Kind::Message(Role::Assistant), text.to_owned());
        let wrapping = Wrapping::new(
            NonZeroUsize::new(width).expect("a fixture is drawn in at least one column"),
            Metrics::default(),
            options,
        );

        block.render(window, &wrapping)
    }

    /// # Returns
    ///
    /// The text of `text` each of `segments` names.
    fn slices(text: &str, segments: &[Range<usize>]) -> Vec<String> {
        segments
            .iter()
            .map(|segment| {
                text.get(segment.clone())
                    .expect("a segment names a range of the source")
                    .to_owned()
            })
            .collect()
    }
}
