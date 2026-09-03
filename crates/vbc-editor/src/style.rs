//! Styling the display rows a block of text is rendered in.
//!
//! A [`Block`] holds the text it was built from and the [`Span`]s drawn over it, each of which
//! carries a byte range back into that text. Styling is a projection and the source is the truth:
//! a span says only how a part of the text is drawn, and every rendered segment names the bytes it
//! came from, so the exact text behind anything on the screen is a slice of the block.
//!
//! Applying styles never moves a character. A [`StyledRow`] draws the cells its display row draws
//! and no others; the spans decide only where one run of cells ends and the next begins. A span
//! reaching across a wrap boundary therefore styles its part of each row it reaches, and the rows
//! themselves are the rows the layout produced.
//!
//! Two rules settle what a span means where a caller left it ambiguous:
//!
//! * A span is widened to the grapheme cluster boundaries around it, so a range landing inside a
//!   cluster styles the whole cluster rather than splitting it. A span left empty by that, or
//!   lying past the end of the source, styles nothing.
//! * Where spans overlap, the later span of the block's list wins over the earlier one, so a block
//!   is painted in the order its spans were given.
//!
//! The decoration a continuation row carries is not text of the block, so it is drawn unstyled.

use std::collections::BTreeSet;
use std::ops::Range;

use vbc_layout::buffer::LINE_SEPARATOR;
use vbc_layout::line::DisplayRow;
use vbc_layout::width::grapheme_indices;

/// How a run of cells is drawn, which is the type the cells of a drawn terminal buffer carry.
pub type Style = ratatui::style::Style;

/// A styled region of a block's source, held as the byte range it covers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span {
    range: Range<usize>,
    style: Style,
}

impl Span {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A span styling `range` of a block's source.
    #[must_use]
    pub fn new(range: Range<usize>, style: Style) -> Self {
        Self { range, style }
    }

    #[must_use]
    pub fn range(&self) -> &Range<usize> {
        &self.range
    }

    #[must_use]
    pub fn style(&self) -> Style {
        self.style
    }
}

/// A block of text together with the spans styling it.
///
/// The spans a block reports are the spans it draws: each is widened to the cluster boundaries
/// around it when the block is built, and one that covers no cluster is dropped. The disjoint runs
/// those spans paint the source in are resolved when the block is built as well, so styling a row
/// costs that row and a search of the runs rather than costing every span of the block once for
/// every row drawn.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Block {
    source: String,
    spans: Vec<Span>,
    runs: Vec<Run>,
}

impl Block {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// An unstyled block of `source`.
    #[must_use]
    pub fn new(source: String) -> Self {
        Self {
            source,
            spans: Vec::new(),
            runs: Vec::new(),
        }
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A block of `source` styled by `spans`, in the order they are given.
    #[must_use]
    pub fn with_spans(source: String, spans: Vec<Span>) -> Self {
        let spans = widen(&source, spans);
        let runs = resolve(&spans);

        Self {
            source,
            spans,
            runs,
        }
    }

    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// # Returns
    ///
    /// The source behind `range`, or `None` if `range` is not a range of it.
    #[must_use]
    pub fn slice(&self, range: Range<usize>) -> Option<&str> {
        self.source.get(range)
    }

    /// # Returns
    ///
    /// Each logical line of the source, paired with the byte offset it starts at.
    pub fn lines(&self) -> impl Iterator<Item = (usize, &str)> {
        let mut start = 0;
        self.source.split(LINE_SEPARATOR).map(move |line| {
            let offset = start;
            start += line.len() + LINE_SEPARATOR.len_utf8();
            (offset, line)
        })
    }

    /// Applies the block's spans to the display rows one of its logical lines is laid out into.
    ///
    /// `line_start` is the byte offset within the source at which that logical line starts, which
    /// is what [`Block::lines`] hands out beside the line itself.
    ///
    /// # Returns
    ///
    /// The styled rows, one per row of `rows`.
    ///
    /// # Panics
    ///
    /// Panics if a row of `rows` carries fewer columns than the graphemes it shows, which no
    /// laid-out row does.
    #[must_use]
    pub fn style_rows(&self, line_start: usize, rows: &[DisplayRow]) -> Vec<StyledRow> {
        let runs = &self.runs;
        let mut styled = Vec::with_capacity(rows.len());
        let mut start = line_start;
        for row in rows {
            styled.push(style_row(row, start, runs));
            start += row.text().len();
        }

        styled
    }
}

/// One run of a display row drawn in a single style.
///
/// A segment carries the cells it is drawn in, in which a tab is spelled as the blanks it advances
/// by, and the byte range of the block's source those cells render.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyledSegment {
    source: Range<usize>,
    cells: String,
    column: usize,
    style: Style,
}

impl StyledSegment {
    /// # Returns
    ///
    /// The byte range of the block's source the segment renders.
    #[must_use]
    pub fn source(&self) -> &Range<usize> {
        &self.source
    }

    /// # Returns
    ///
    /// The cells the segment is drawn in, every tab spelled as the blanks it advances by.
    #[must_use]
    pub fn cells(&self) -> &str {
        &self.cells
    }

    /// # Returns
    ///
    /// The column of the row at which the segment starts, its decoration accounted for.
    #[must_use]
    pub fn column(&self) -> usize {
        self.column
    }

    #[must_use]
    pub fn style(&self) -> Style {
        self.style
    }
}

/// One display row with its styles applied: the row the layout produced, the decoration it
/// carries drawn unstyled, and the styled segments its text is drawn as.
///
/// The display row is kept rather than copied out of, so a styled row can be drawn without the
/// caller carrying the row it was built from alongside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyledRow {
    row: DisplayRow,
    segments: Vec<StyledSegment>,
}

impl StyledRow {
    /// # Returns
    ///
    /// The display row the layout produced, which is the row these styles were applied to.
    #[must_use]
    pub fn row(&self) -> &DisplayRow {
        &self.row
    }

    /// # Returns
    ///
    /// The decoration drawn in front of the row's segments, empty on the row that starts a line.
    #[must_use]
    pub fn prefix(&self) -> &str {
        self.row.prefix()
    }

    #[must_use]
    pub fn segments(&self) -> &[StyledSegment] {
        &self.segments
    }

    /// # Returns
    ///
    /// The cells the row is drawn in, its decoration included, which are the cells the display row
    /// it was built from is drawn in.
    #[must_use]
    pub fn cells(&self) -> String {
        let mut cells = self.prefix().to_owned();
        for segment in &self.segments {
            cells.push_str(&segment.cells);
        }

        cells
    }
}

/// One stretch of a block's source painted in a single style.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Run {
    range: Range<usize>,
    style: Style,
}

/// # Returns
///
/// The disjoint runs `spans` paint a block's source in, ascending and with the overlaps between
/// spans resolved in favour of the later span.
fn resolve(spans: &[Span]) -> Vec<Run> {
    let edges: BTreeSet<usize> = spans
        .iter()
        .flat_map(|span| [span.range.start, span.range.end])
        .collect();
    let edges: Vec<usize> = edges.into_iter().collect();
    let mut opening = ordered(spans, |span| span.range.start);
    let mut closing = ordered(spans, |span| span.range.end);

    let mut painting: BTreeSet<usize> = BTreeSet::new();
    let mut runs: Vec<Run> = Vec::new();
    for pair in edges.windows(2) {
        let &[start, end] = pair else {
            continue;
        };
        while closing.last().is_some_and(|(at, _)| *at <= start) {
            let (_, span) = closing
                .pop()
                .expect("the last of the closing spans is there");
            painting.remove(&span);
        }
        while opening.last().is_some_and(|(at, _)| *at <= start) {
            let (_, span) = opening
                .pop()
                .expect("the last of the opening spans is there");
            painting.insert(span);
        }
        let Some(painted) = painting.last().map(|span| &spans[*span]) else {
            continue;
        };

        match runs.last_mut() {
            Some(run) if run.style == painted.style && run.range.end == start => {
                run.range.end = end;
            }
            _ => runs.push(Run {
                range: start..end,
                style: painted.style,
            }),
        }
    }

    runs
}

/// # Returns
///
/// Every span paired with the edge of it `edge` names, ordered so that the next edge to be reached
/// is the last of them.
fn ordered(spans: &[Span], edge: impl Fn(&Span) -> usize) -> Vec<(usize, usize)> {
    let mut edges: Vec<(usize, usize)> = spans
        .iter()
        .enumerate()
        .map(|(index, span)| (edge(span), index))
        .collect();
    edges.sort_unstable_by(|one, other| other.cmp(one));

    edges
}

/// Widens each span to the grapheme cluster boundaries of `source` around it, dropping those left
/// covering no cluster.
///
/// # Returns
///
/// The widened spans, in the order they were given.
fn widen(source: &str, spans: Vec<Span>) -> Vec<Span> {
    let boundaries: Vec<usize> = grapheme_indices(source)
        .map(|(offset, _)| offset)
        .chain(std::iter::once(source.len()))
        .collect();

    spans
        .into_iter()
        .filter_map(|span| {
            let start = span.range.start.min(source.len());
            let end = span.range.end.min(source.len());
            if end <= start {
                return None;
            }

            let below = boundaries.partition_point(|&boundary| boundary <= start);
            let above = boundaries.partition_point(|&boundary| boundary < end);

            Some(Span {
                range: boundaries[below - 1]..boundaries[above],
                style: span.style,
            })
        })
        .collect()
}

/// Applies the runs painting a block to one display row of it.
///
/// # Returns
///
/// The row's text drawn as the styled segments the runs paint it in, adjacent graphemes sharing a
/// style drawn as one segment.
///
/// # Panics
///
/// Panics if the row carries fewer columns than the graphemes it shows, which no laid-out row
/// does.
fn style_row(row: &DisplayRow, row_start: usize, runs: &[Run]) -> StyledRow {
    let columns = row.columns();
    let mut segments: Vec<StyledSegment> = Vec::new();
    for (index, (offset, grapheme)) in grapheme_indices(row.text()).enumerate() {
        let column = *columns
            .get(index)
            .expect("a row carries a column for each of its graphemes");
        let next = *columns
            .get(index + 1)
            .expect("a row's columns end past its last grapheme");
        let drawn = if "\t" == grapheme {
            " ".repeat(next - column)
        } else {
            grapheme.to_owned()
        };

        let source = row_start + offset..row_start + offset + grapheme.len();
        let style = style_at(runs, source.start);
        match segments.last_mut() {
            Some(segment) if segment.style == style => {
                segment.cells.push_str(&drawn);
                segment.source.end = source.end;
            }
            _ => segments.push(StyledSegment {
                source,
                cells: drawn,
                column,
                style,
            }),
        }
    }

    StyledRow {
        row: row.clone(),
        segments,
    }
}

/// # Returns
///
/// The style `runs` paint the byte at `offset` in, or the default style if none of them reaches
/// it.
fn style_at(runs: &[Run], offset: usize) -> Style {
    let index = runs.partition_point(|run| run.range.end <= offset);

    runs.get(index)
        .filter(|run| run.range.contains(&offset))
        .map_or_else(Style::default, |run| run.style)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::ops::Range;

    use ratatui::style::Color;
    use vbc_layout::line::{self, Options};
    use vbc_layout::width::{AmbiWidth, Metrics};

    use super::{Block, Span, Style, StyledRow, StyledSegment};

    /// The width the fixtures wrap after, narrow enough that a six-grapheme line takes two rows.
    const WIDTH: usize = 3;

    /// The width no fixture wraps at.
    const UNWRAPPED: usize = 16;

    #[test]
    fn a_span_crossing_a_wrap_boundary_styles_both_rows() {
        let block = Block::with_spans("abcdef".to_owned(), vec![Span::new(2..4, red())]);
        let rows = rows(&block, WIDTH, &Options::new());
        assert_eq!(2, rows.len());

        assert_eq!(
            vec![(0..2, "ab", Style::default()), (2..3, "c", red())],
            drawn(&rows[0])
        );
        assert_eq!(
            vec![(3..4, "d", red()), (4..6, "ef", Style::default())],
            drawn(&rows[1])
        );

        let yanked: String = rows
            .iter()
            .flat_map(StyledRow::segments)
            .filter(|segment| red() == segment.style())
            .map(|segment| {
                block
                    .slice(segment.source().clone())
                    .expect("a segment names a range of the source")
            })
            .collect();
        assert_eq!("cd", yanked);
    }

    #[test]
    fn overlapping_spans_are_painted_in_the_order_they_were_given() {
        let later_wins = Block::with_spans(
            "abcdef".to_owned(),
            vec![Span::new(0..4, red()), Span::new(2..6, blue())],
        );
        assert_eq!(
            vec![(0..2, "ab", red()), (2..6, "cdef", blue())],
            drawn(&rows(&later_wins, UNWRAPPED, &Options::new())[0])
        );

        let reversed = Block::with_spans(
            "abcdef".to_owned(),
            vec![Span::new(2..6, blue()), Span::new(0..4, red())],
        );
        assert_eq!(
            vec![(0..4, "abcd", red()), (4..6, "ef", blue())],
            drawn(&rows(&reversed, UNWRAPPED, &Options::new())[0])
        );
    }

    #[test]
    fn a_span_covered_by_a_later_one_is_painted_over_entirely() {
        let block = Block::with_spans(
            "abcdef".to_owned(),
            vec![Span::new(2..4, red()), Span::new(0..6, blue())],
        );
        assert_eq!(
            vec![(0..6, "abcdef", blue())],
            drawn(&rows(&block, UNWRAPPED, &Options::new())[0])
        );
    }

    #[test]
    fn a_span_landing_inside_a_cluster_styles_the_whole_cluster() {
        let source = "e\u{0301}x".to_owned();
        for range in [0..1, 1..2, 1..3, 0..2] {
            let block = Block::with_spans(source.clone(), vec![Span::new(range.clone(), red())]);
            assert_eq!(
                &[Span::new(0..3, red())],
                block.spans(),
                "the span {range:?} was not widened to the cluster it lands in"
            );
            assert_eq!(
                vec![(0..3, "e\u{0301}", red()), (3..4, "x", Style::default())],
                drawn(&rows(&block, UNWRAPPED, &Options::new())[0])
            );
        }
    }

    #[test]
    fn a_span_already_on_cluster_boundaries_is_left_where_it_was() {
        let block = Block::with_spans("e\u{0301}x".to_owned(), vec![Span::new(3..4, red())]);
        assert_eq!(&[Span::new(3..4, red())], block.spans());
    }

    #[test]
    fn a_span_covering_no_cluster_styles_nothing() {
        let backwards = Range { start: 4, end: 2 };
        for range in [2..2, backwards, 8..12] {
            let block = Block::with_spans("abcdef".to_owned(), vec![Span::new(range, red())]);
            assert_eq!(&[] as &[Span], block.spans());
            assert_eq!(
                vec![(0..6, "abcdef", Style::default())],
                drawn(&rows(&block, UNWRAPPED, &Options::new())[0])
            );
        }
    }

    #[test]
    fn a_span_reaching_past_the_source_is_clamped_to_it() {
        let block = Block::with_spans("abcdef".to_owned(), vec![Span::new(4..99, red())]);
        assert_eq!(&[Span::new(4..6, red())], block.spans());
    }

    #[test]
    fn a_tab_is_styled_across_the_blanks_it_is_drawn_as() {
        let block = Block::with_spans("a\tb".to_owned(), vec![Span::new(1..2, red())]);
        let rows = rows(&block, UNWRAPPED, &Options::new());
        assert_eq!(
            vec![
                (0..1, "a", Style::default()),
                (1..2, "       ", red()),
                (2..3, "b", Style::default()),
            ],
            drawn(&rows[0])
        );
        assert_eq!(1, rows[0].segments()[1].column());
    }

    #[test]
    fn a_continuation_decoration_is_drawn_unstyled() {
        let options = Options::new().with_show_break("> ".to_owned());
        let block = Block::with_spans("abcdefgh".to_owned(), vec![Span::new(0..8, red())]);
        let rows = rows(&block, 4, &options);

        assert_eq!("", rows[0].prefix());
        assert_eq!("> ", rows[1].prefix());
        assert_eq!(2, rows[1].segments()[0].column());
        assert_eq!("> ef", rows[1].cells());
    }

    #[test]
    fn the_lines_of_a_block_are_the_offsets_its_rows_are_addressed_from() {
        let block = Block::new("ab\ncd\n".to_owned());
        assert_eq!(
            vec![(0, "ab"), (3, "cd"), (6, "")],
            block.lines().collect::<Vec<(usize, &str)>>()
        );
    }

    #[test]
    fn a_span_is_addressed_against_the_source_rather_than_the_line_it_falls_in() {
        let block = Block::with_spans("ab\ncd".to_owned(), vec![Span::new(3..5, red())]);
        let mut lines = block.lines();

        let (start, text) = lines.next().expect("the source holds a first line");
        assert_eq!(
            vec![(0..2, "ab", Style::default())],
            drawn(&styled(&block, 0, start, text, UNWRAPPED, &Options::new())[0])
        );

        let (start, text) = lines.next().expect("the source holds a second line");
        assert_eq!(
            vec![(3..5, "cd", red())],
            drawn(&styled(&block, 1, start, text, UNWRAPPED, &Options::new())[0])
        );
    }

    #[test]
    fn an_unstyled_block_draws_each_row_in_one_default_segment() {
        let block = Block::new("abcdef".to_owned());
        let rows = rows(&block, WIDTH, &Options::new());
        assert_eq!(vec![(0..3, "abc", Style::default())], drawn(&rows[0]));
        assert_eq!(vec![(3..6, "def", Style::default())], drawn(&rows[1]));
    }

    #[test]
    fn an_empty_line_is_drawn_as_a_row_of_no_segments() {
        let block = Block::with_spans(String::new(), vec![Span::new(0..1, red())]);
        let rows = rows(&block, WIDTH, &Options::new());
        assert_eq!(1, rows.len());
        assert_eq!(&[] as &[StyledSegment], rows[0].segments());
        assert_eq!("", rows[0].cells());
    }

    /// # Returns
    ///
    /// The styled rows the first logical line of `block` is drawn as.
    fn rows(block: &Block, width: usize, options: &Options) -> Vec<StyledRow> {
        let (start, text) = block.lines().next().expect("a block holds a first line");

        styled(block, 0, start, text, width, options)
    }

    /// # Returns
    ///
    /// The styled rows the logical line `line` of `block`, whose text is `text` and which starts
    /// at byte `start` of the source, is drawn as.
    fn styled(
        block: &Block,
        line: usize,
        start: usize,
        text: &str,
        width: usize,
        options: &Options,
    ) -> Vec<StyledRow> {
        let rows = line::lay_out(
            line,
            text,
            NonZeroUsize::new(width).expect("a fixture is drawn in at least one column"),
            Metrics::new(
                AmbiWidth::Single,
                NonZeroUsize::new(8).expect("a fixture advances tabs by eight columns"),
            ),
            options,
        );

        block.style_rows(start, &rows)
    }

    /// # Returns
    ///
    /// What each segment of `row` renders: the source it names, the cells it is drawn in, and the
    /// style it is drawn under.
    fn drawn(row: &StyledRow) -> Vec<(Range<usize>, &str, Style)> {
        row.segments()
            .iter()
            .map(|segment| (segment.source().clone(), segment.cells(), segment.style()))
            .collect()
    }

    /// # Returns
    ///
    /// One of the two styles the fixtures tell apart.
    fn red() -> Style {
        Style::new().fg(Color::Red)
    }

    /// # Returns
    ///
    /// The other of the two styles the fixtures tell apart.
    fn blue() -> Style {
        Style::new().bg(Color::Blue)
    }
}
