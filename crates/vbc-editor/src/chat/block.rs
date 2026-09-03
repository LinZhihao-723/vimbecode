//! The blocks a transcript is a sequence of.
//!
//! A block is one thing that was said: a message, a fenced code block, a call to a tool, what the
//! tool answered, a thinking block, or a diff. It holds the source it was built from and the spans
//! styling it, and the spans name byte ranges of that source, so nothing a block draws is
//! addressed in any coordinate but the source's.
//!
//! Rendering is a projection over a window of that source. A block is asked for a [`RowWindow`] --
//! a display row to start at and a number of rows to draw -- and lays out the logical lines from
//! its own start down to the bottom of that window, the way [`vbc_layout::anchor`]'s mapping walks
//! out from an anchor: a line at a time, each thrown away once its rows have been counted, nothing
//! remembered between calls and nothing below the window touched at all. What a block holds past
//! the window therefore costs nothing, which is what lets a transcript hold the whole of what
//! `cargo` wrote and still draw a frame in the time a frame has.
//!
//! What that costs is the rows down to the bottom of the window and nothing else, which is the
//! same walk an anchored mapping pays for and is why the length of the block does not enter into
//! it. Measured in release at eighty columns, twenty rows off the top of a block cost 37 µs and
//! 24,017 bytes at a hundred lines, at a thousand, at ten thousand and at a hundred thousand
//! alike. Twenty rows further down cost the rows above them as well: 0.9 ms at row 1,000, 89 ms at
//! row 100,000, in a block of either length. A caller drawing a panel therefore asks each block
//! for the rows it shows of that block rather than for a window into the middle of a long one.
//!
//! A rendered row carries the byte offset of the source it starts at, which is what lets the
//! source behind a run of rows be recovered exactly -- separators included, which no row's own
//! text holds -- and is what a selection over rendered rows will be turned into a range of the
//! source with.

use std::ops::Range;

use vbc_layout::anchor::Wrapping;
use vbc_layout::buffer::LINE_SEPARATOR;
use vbc_layout::line::{self, DisplayRow};

use crate::chat::{ansi, diff};
use crate::style::{self, Span, StyledRow};

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

/// The display rows of a block a caller is asking to be drawn: the row of the block to start at,
/// and how many rows to draw from there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RowWindow {
    start: usize,
    rows: usize,
}

impl RowWindow {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A window of `rows` display rows starting at the block's row `start`.
    #[must_use]
    pub fn new(start: usize, rows: usize) -> Self {
        Self { start, rows }
    }

    /// # Returns
    ///
    /// The row of the block the window starts at.
    #[must_use]
    pub fn start(&self) -> usize {
        self.start
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    kind: Kind,
    body: style::Block,
}

impl Block {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// An unstyled block of `source`.
    #[must_use]
    pub fn new(kind: Kind, source: String) -> Self {
        Self {
            kind,
            body: style::Block::new(source),
        }
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A block of `source` styled by `spans`, in the order they are given.
    #[must_use]
    pub fn with_spans(kind: Kind, source: String, spans: Vec<Span>) -> Self {
        Self {
            kind,
            body: style::Block::with_spans(source, spans),
        }
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
        Self {
            kind,
            body: ansi::parse(raw),
        }
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A [`Kind::Diff`] block of the lines between the text `old` an edit to `path` replaced and
    /// the text `new` it wrote.
    #[must_use]
    pub fn diff(path: String, old: &str, new: &str) -> Self {
        Self {
            kind: Kind::Diff { path },
            body: diff::compute(old, new),
        }
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
    /// The walk starts at the block's first row and stops at the window's last, so what it costs
    /// is the rows down to the bottom of the window and never the block: twenty rows off the top
    /// of a hundred-line block and off the top of a hundred-thousand-line one both cost 37 µs and
    /// 24,017 bytes, which `chat_cost.rs` measures rather than asserts.
    ///
    /// # Returns
    ///
    /// The rows of the window, top to bottom, which are fewer than were asked for where the block
    /// ends inside the window and none at all where it ends above it.
    #[must_use]
    pub fn render(&self, window: RowWindow, wrapping: &Wrapping) -> Rendered {
        let source = self.body.source();
        let end = source.len();
        let wanted = window.rows;
        let mut rows: Vec<RenderedRow> = Vec::new();
        let mut above = window.start;
        let mut drawn = 0;
        let mut offset = 0;
        let mut index = 0;

        while offset <= end && drawn < wanted {
            let text = line_at(source, offset);
            let laid_out = line::lay_out(
                index,
                text,
                wrapping.width(),
                wrapping.metrics(),
                wrapping.options(),
            );

            if above < laid_out.len() {
                let taken = (laid_out.len() - above).min(wanted - drawn);
                let mut at = offset + bytes_of(&laid_out[..above]);
                for styled in self.body.style_rows(at, &laid_out[above..above + taken]) {
                    let length = styled.row().text().len();
                    rows.push(RenderedRow { start: at, styled });
                    at += length;
                }
                drawn += taken;
                above = 0;
            } else {
                above -= laid_out.len();
            }

            offset += text.len() + LINE_SEPARATOR.len_utf8();
            index += 1;
        }

        Rendered {
            start: window.start,
            rows,
        }
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

/// A window of a block as it is drawn: the rows it was asked for, top to bottom, and the row of
/// the block the window began at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rendered {
    start: usize,
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
    /// wherever any were drawn at all.
    #[must_use]
    pub fn start(&self) -> usize {
        self.start
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

    use super::{Block, Kind, Rendered, RenderedRow, Role, RowWindow};

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
        assert_eq!(1, rendered.start());
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
        ]
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
