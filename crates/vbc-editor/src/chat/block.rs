//! The blocks a transcript is a sequence of.
//!
//! A block is one thing that was said: a message, a fenced code block, a call to a tool, what the
//! tool answered, a thinking block, or a diff. It holds the source it was built from and the spans
//! styling it, and the spans name byte ranges of that source, so nothing a block draws is
//! addressed in any coordinate but the source's.
//!
//! Rendering is a projection. A rendered row carries the byte offset of the source it starts at,
//! which is what lets the source behind a run of rows be recovered exactly -- separators included,
//! which no row's own text holds -- and is what a selection over rendered rows will be turned into
//! a range of the source with.

use std::ops::Range;

use vbc_layout::anchor::Wrapping;
use vbc_layout::line;

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

    /// The lines an edit changed, computed from the text either side of it.
    Diff,
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
    /// A [`Kind::Diff`] block of the lines between the text `old` an edit replaced and the text
    /// `new` it wrote.
    #[must_use]
    pub fn diff(old: &str, new: &str) -> Self {
        Self {
            kind: Kind::Diff,
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

    /// Lays the block out and applies its spans to the rows it is drawn in.
    ///
    /// The whole block is rendered rather than the part of it a window shows, so a caller drawing
    /// a screenful renders the blocks it reaches and no others.
    ///
    /// # Returns
    ///
    /// The rows the block is drawn in, top to bottom, which are at least one per logical line of
    /// the source.
    #[must_use]
    pub fn render(&self, wrapping: &Wrapping) -> Rendered {
        let mut rows = Vec::new();
        for (index, (start, text)) in self.body.lines().enumerate() {
            let laid_out = line::lay_out(
                index,
                text,
                wrapping.width(),
                wrapping.metrics(),
                wrapping.options(),
            );

            let mut offset = start;
            for styled in self.body.style_rows(start, &laid_out) {
                let length = styled.row().text().len();
                rows.push(RenderedRow {
                    start: offset,
                    styled,
                });
                offset += length;
            }
        }

        Rendered { rows }
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

/// A block as it is drawn: the rows it is laid out into, top to bottom.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rendered {
    rows: Vec<RenderedRow>,
}

impl Rendered {
    #[must_use]
    pub fn rows(&self) -> &[RenderedRow] {
        &self.rows
    }

    /// # Returns
    ///
    /// The byte range of the block's source the rows were drawn from, which is the whole of it and
    /// which includes the separators no row draws.
    ///
    /// # Panics
    ///
    /// Panics if the block was drawn in no rows, which no rendered block is.
    #[must_use]
    pub fn source(&self) -> Range<usize> {
        let first = self
            .rows
            .first()
            .expect("a block is drawn in at least one row");
        let last = self
            .rows
            .last()
            .expect("a block is drawn in at least one row");

        first.start..last.source().end
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::ops::Range;

    use ratatui::style::{Color, Modifier, Style};
    use vbc_layout::anchor::Wrapping;
    use vbc_layout::line::Options;
    use vbc_layout::width::Metrics;

    use crate::style::{Span, StyledSegment};

    use super::{Block, Kind, Rendered, RenderedRow, Role};

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
            let rendered = block.render(&wrapping(WIDTH));
            assert_eq!(
                block.source(),
                block
                    .slice(rendered.source())
                    .expect("a render names a range of the source"),
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
        let rendered = block.render(&wrapping(3));

        assert_eq!(vec![0..3, 3..6, 6..8], sources(&rendered));
        assert_eq!(0..8, rendered.source());
    }

    #[test]
    fn a_row_names_the_bytes_it_draws_and_the_separators_name_the_rest() {
        let block = Block::new(Kind::Message(Role::User), "ab\n\ncd".to_owned());
        let rendered = block.render(&wrapping(UNWRAPPED));

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
            "ab\n\ncd",
            block
                .slice(rendered.source())
                .expect("a render names a range of the source")
        );
    }

    #[test]
    fn a_span_names_the_same_source_once_it_has_been_rendered() {
        let source = "the middle of it wraps".to_owned();
        let span = Span::new(4..10, red());
        let block = Block::with_spans(Kind::Message(Role::Assistant), source, vec![span.clone()]);
        let rendered = block.render(&wrapping(WIDTH));

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
        let rendered = block.render(&wrapping(UNWRAPPED));

        assert_eq!("error: nope\ndone", block.source());
        assert_eq!(
            &[Span::new(0..5, red().add_modifier(Modifier::BOLD))],
            block.spans()
        );

        let recovered = block
            .slice(rendered.source())
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
        let block = Block::diff("fn main() {}\n", "fn main() {\n    todo!();\n}\n");
        let rendered = block.render(&wrapping(UNWRAPPED));

        assert_eq!(
            "-fn main() {}\n+fn main() {\n+    todo!();\n+}",
            block.source()
        );
        assert_eq!(
            block.source(),
            block
                .slice(rendered.source())
                .expect("a render names a range of the source")
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
    fn a_source_of_wide_and_joined_clusters_round_trips() {
        let block = Block::new(Kind::Message(Role::Assistant), CLUSTERS.to_owned());
        let rendered = block.render(&wrapping(4));

        assert!(
            2 < rendered.rows().len(),
            "the fixture did not wrap: {rendered:?}"
        );
        assert_eq!(
            block.source(),
            block
                .slice(rendered.source())
                .expect("a render names a range of the source")
        );

        let drawn: String = rendered
            .rows()
            .iter()
            .map(|row| row.styled().row().text())
            .collect();
        assert_eq!(block.source().replace('\n', ""), drawn);
    }

    #[test]
    fn an_empty_block_is_drawn_in_one_empty_row() {
        let block = Block::new(Kind::Message(Role::User), String::new());
        let rendered = block.render(&wrapping(WIDTH));

        assert_eq!(vec![0..0], sources(&rendered));
        assert_eq!(
            "",
            block
                .slice(rendered.source())
                .expect("a render names a range of the source")
        );
    }

    #[test]
    fn a_source_ending_at_a_separator_keeps_it_and_one_ending_without_one_adds_none() {
        let ended = Block::new(Kind::Message(Role::User), "tail\n".to_owned());
        let bare = Block::new(Kind::Message(Role::User), "tail".to_owned());
        let wrapping = wrapping(UNWRAPPED);

        let rendered = ended.render(&wrapping);
        assert_eq!(vec![0..4, 5..5], sources(&rendered));
        assert_eq!(
            "tail\n",
            ended
                .slice(rendered.source())
                .expect("a render names a range of the source")
        );

        let rendered = bare.render(&wrapping);
        assert_eq!(vec![0..4], sources(&rendered));
        assert_eq!(
            "tail",
            bare.slice(rendered.source())
                .expect("a render names a range of the source")
        );
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
            Block::diff("fn main() {}\n", "fn main() {\n    todo!();\n}\n"),
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
    /// The style the fixtures are drawn in where they are drawn in one.
    fn red() -> Style {
        Style::new().fg(Color::Red)
    }
}
