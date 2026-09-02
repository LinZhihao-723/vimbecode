//! The text vimbecode edits, and the index that addresses it.
//!
//! The text is kept as one string per logical line rather than as a rope: at the sizes a prompt
//! reaches -- a few kilobytes -- the flat form outruns a rope on every edit measured, a paste at
//! the front of the text included. Alongside the lines a buffer carries the byte offset each line
//! begins at, so the two ways of naming a spot in the text -- a line together with a grapheme
//! offset into it, and a byte offset into the whole text -- convert into each other without
//! walking every line.
//!
//! Positions are counted in grapheme clusters, the unit [`crate::width`] measures in, so an edit
//! can never land inside what a terminal draws as one glyph.

use std::cmp::Ordering;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::mem;
use std::ops::Range;

use crate::invariants::LogicalPosition;
use crate::width::{grapheme_indices, graphemes};

/// The character separating one logical line from the next in a buffer's text.
pub const LINE_SEPARATOR: char = '\n';

/// The ways a position or a range can fail to address a buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// A position named a line the buffer does not hold.
    LineOutOfBounds {
        /// The line that was named.
        line: usize,

        /// The number of lines the buffer holds.
        line_count: usize,
    },

    /// A position named a grapheme past the end of its line.
    GraphemeOutOfBounds {
        /// The position that was named.
        position: LogicalPosition,

        /// The number of graphemes the line holds.
        line_len: usize,
    },

    /// A byte offset ran past the end of the buffer's text.
    ByteOffsetOutOfBounds {
        /// The offset that was named.
        offset: usize,

        /// The number of bytes the buffer's text occupies.
        len: usize,
    },

    /// A byte offset fell inside a grapheme cluster, where no position can be.
    NotAGraphemeBoundary {
        /// The offset that was named.
        offset: usize,
    },

    /// A range ended before it started.
    RangeInverted {
        /// Where the range starts.
        start: LogicalPosition,

        /// Where the range ends.
        end: LogicalPosition,
    },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::LineOutOfBounds { line, line_count } => write!(
                formatter,
                "line {line} is past the end of a buffer holding {line_count} lines"
            ),
            Self::GraphemeOutOfBounds { position, line_len } => write!(
                formatter,
                "{position} is past the end of a line holding {line_len} graphemes"
            ),
            Self::ByteOffsetOutOfBounds { offset, len } => write!(
                formatter,
                "byte offset {offset} is past the end of a text {len} bytes long"
            ),
            Self::NotAGraphemeBoundary { offset } => write!(
                formatter,
                "byte offset {offset} falls inside a grapheme cluster"
            ),
            Self::RangeInverted { start, end } => {
                write!(
                    formatter,
                    "the range from {start} to {end} ends before it starts"
                )
            }
        }
    }
}

impl StdError for Error {}

/// The editable text, held as one string per logical line together with the byte offset each line
/// begins at.
///
/// A buffer always holds at least one line, so the empty text is one empty line and every buffer
/// has a position an edit can be made at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Buffer {
    lines: Vec<String>,
    line_starts: Vec<usize>,
}

impl Buffer {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created buffer holding the empty text.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            line_starts: vec![0],
        }
    }

    /// Factory function.
    ///
    /// A trailing [`LINE_SEPARATOR`] opens an empty last line rather than being dropped, so
    /// [`Buffer::text`] returns `text` unchanged.
    ///
    /// # Returns
    ///
    /// A newly created buffer holding `text`.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let lines: Vec<String> = text.split(LINE_SEPARATOR).map(str::to_owned).collect();
        let mut buffer = Self {
            lines,
            line_starts: Vec::new(),
        };
        buffer.reindex_from(0);
        buffer
    }

    /// # Returns
    ///
    /// The buffer's lines, in order and with their separators excluded.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// # Returns
    ///
    /// The number of lines the buffer holds, which is never zero.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// # Returns
    ///
    /// The number of bytes the buffer's text occupies, separators included.
    #[must_use]
    pub fn len(&self) -> usize {
        let last = self.lines.len() - 1;
        self.line_starts[last] + self.lines[last].len()
    }

    /// # Returns
    ///
    /// Whether the buffer holds no text at all, which is one empty line.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        1 == self.lines.len() && self.lines[0].is_empty()
    }

    /// # Returns
    ///
    /// The buffer's text, with its lines joined by [`LINE_SEPARATOR`].
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::with_capacity(self.len());
        for (index, line) in self.lines.iter().enumerate() {
            if 0 != index {
                text.push(LINE_SEPARATOR);
            }
            text.push_str(line);
        }
        text
    }

    /// # Returns
    ///
    /// The text of the line at `index`, or `None` if the buffer has no such line.
    #[must_use]
    pub fn line(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(String::as_str)
    }

    /// # Returns
    ///
    /// The byte offset at which the line at `index` begins, or `None` if the buffer has no such
    /// line.
    #[must_use]
    pub fn line_start(&self, index: usize) -> Option<usize> {
        self.line_starts.get(index).copied()
    }

    /// # Returns
    ///
    /// The number of graphemes on the line at `index`, or `None` if the buffer has no such line.
    #[must_use]
    pub fn line_len(&self, index: usize) -> Option<usize> {
        self.line(index).map(|line| graphemes(line).count())
    }

    /// Converts a position into a byte offset into the buffer's text.
    ///
    /// # Returns
    ///
    /// The byte offset `position` names on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::LineOutOfBounds`] if the buffer has no such line.
    /// * [`Error::GraphemeOutOfBounds`] if the line is shorter than the position's grapheme offset.
    pub fn byte_offset(&self, position: LogicalPosition) -> Result<usize, Error> {
        let within = self.offset_in_line(position)?;
        Ok(self.line_starts[position.line] + within)
    }

    /// Converts a byte offset into the buffer's text into a position.
    ///
    /// # Returns
    ///
    /// The position `offset` names on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::ByteOffsetOutOfBounds`] if the offset runs past the end of the text.
    /// * [`Error::NotAGraphemeBoundary`] if the offset falls inside a grapheme cluster.
    pub fn position(&self, offset: usize) -> Result<LogicalPosition, Error> {
        let len = self.len();
        if len < offset {
            return Err(Error::ByteOffsetOutOfBounds { offset, len });
        }

        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        let text = &self.lines[line];
        let within = offset - self.line_starts[line];
        let mut grapheme = 0;
        for (start, _) in grapheme_indices(text) {
            match start.cmp(&within) {
                Ordering::Equal => return Ok(LogicalPosition { line, grapheme }),
                Ordering::Greater => return Err(Error::NotAGraphemeBoundary { offset }),
                Ordering::Less => grapheme += 1,
            }
        }
        if within == text.len() {
            Ok(LogicalPosition { line, grapheme })
        } else {
            Err(Error::NotAGraphemeBoundary { offset })
        }
    }

    /// Inserts text at a position, opening a new line wherever the inserted text holds a
    /// [`LINE_SEPARATOR`].
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Buffer::replace`]'s return values on failure.
    pub fn insert(&mut self, at: LogicalPosition, text: &str) -> Result<(), Error> {
        self.replace(at..at, text).map(|_| ())
    }

    /// Deletes the text a range covers, joining the lines it spans.
    ///
    /// # Returns
    ///
    /// The text that was removed on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Buffer::replace`]'s return values on failure.
    pub fn delete(&mut self, range: Range<LogicalPosition>) -> Result<String, Error> {
        self.replace(range, "")
    }

    /// Replaces the text a range covers with other text.
    ///
    /// An empty range inserts, and empty text deletes. The lines the range spans collapse into the
    /// lines the replacement text holds, so a replacement free of [`LINE_SEPARATOR`] joins them
    /// into one.
    ///
    /// # Returns
    ///
    /// The text that was removed on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::LineOutOfBounds`] if the buffer holds neither end of the range.
    /// * [`Error::GraphemeOutOfBounds`] if a line is shorter than the offset an end names.
    /// * [`Error::RangeInverted`] if the range ends before it starts.
    pub fn replace(&mut self, range: Range<LogicalPosition>, text: &str) -> Result<String, Error> {
        let start = range.start;
        let end = range.end;
        let start_offset = self.offset_in_line(start)?;
        let end_offset = self.offset_in_line(end)?;
        if (end.line, end_offset) < (start.line, start_offset) {
            return Err(Error::RangeInverted { start, end });
        }

        let removed = self.slice(start.line, start_offset, end.line, end_offset);
        let head = self.lines[start.line][..start_offset].to_owned();
        let tail = self.lines[end.line][end_offset..].to_owned();

        let mut pieces = text.split(LINE_SEPARATOR);
        let mut current = head;
        current.push_str(pieces.next().expect("a split yields at least one piece"));
        let mut replacement = Vec::new();
        for piece in pieces {
            replacement.push(mem::replace(&mut current, piece.to_owned()));
        }
        current.push_str(&tail);
        replacement.push(current);

        self.lines.splice(start.line..=end.line, replacement);
        self.reindex_from(start.line);
        Ok(removed)
    }

    /// # Returns
    ///
    /// The byte offset `position` names within its own line on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::LineOutOfBounds`] if the buffer has no such line.
    /// * [`Error::GraphemeOutOfBounds`] if the line is shorter than the position's grapheme offset.
    fn offset_in_line(&self, position: LogicalPosition) -> Result<usize, Error> {
        let line = self.line(position.line).ok_or(Error::LineOutOfBounds {
            line: position.line,
            line_count: self.lines.len(),
        })?;

        let mut offset = 0;
        let mut count = 0;
        for grapheme in graphemes(line) {
            if count == position.grapheme {
                return Ok(offset);
            }
            offset += grapheme.len();
            count += 1;
        }
        if count == position.grapheme {
            return Ok(offset);
        }
        Err(Error::GraphemeOutOfBounds {
            position,
            line_len: count,
        })
    }

    /// # Returns
    ///
    /// The text between two byte offsets, each taken within the line it is paired with.
    fn slice(
        &self,
        start_line: usize,
        start_offset: usize,
        end_line: usize,
        end_offset: usize,
    ) -> String {
        if start_line == end_line {
            return self.lines[start_line][start_offset..end_offset].to_owned();
        }

        let mut slice = self.lines[start_line][start_offset..].to_owned();
        for line in &self.lines[start_line + 1..end_line] {
            slice.push(LINE_SEPARATOR);
            slice.push_str(line);
        }
        slice.push(LINE_SEPARATOR);
        slice.push_str(&self.lines[end_line][..end_offset]);
        slice
    }

    /// Rebuilds the line index from `line` onwards, the lines before it being unchanged.
    fn reindex_from(&mut self, line: usize) {
        self.line_starts.resize(self.lines.len(), 0);
        let mut start = if 0 == line {
            0
        } else {
            self.line_starts[line - 1] + self.lines[line - 1].len() + LINE_SEPARATOR.len_utf8()
        };
        for (index, text) in self.lines.iter().enumerate().skip(line) {
            self.line_starts[index] = start;
            start += text.len() + LINE_SEPARATOR.len_utf8();
        }
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// The ZWJ sequence for the family of a man, a woman, a girl, and a boy: seven code points and
    /// one glyph.
    const ZWJ_FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";

    /// The flag of Japan, a pair of regional indicators.
    const FLAG: &str = "\u{1F1EF}\u{1F1F5}";

    /// Text whose lines hold, in turn: ASCII and CJK; a ZWJ sequence and a flag; letters carrying
    /// combining marks and one more CJK character; nothing; and ASCII again.
    const MIXED_TEXT: &str = concat!(
        "ab\u{4E2D}\u{6587}\n",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}x\u{1F1EF}\u{1F1F5}\n",
        "e\u{301}\u{323}a\u{301}\u{4E2D}\n",
        "\n",
        "tail"
    );

    /// The graphemes an edit's text is drawn from.
    ///
    /// Every entry begins with a character that never joins the one before it, so concatenating
    /// entries yields exactly as many clusters as entries.
    const ALPHABET: [&str; 8] = [
        "a", "b", " ", "\n", "\u{4E2D}", "\u{6587}", ZWJ_FAMILY, FLAG,
    ];

    /// # Returns
    ///
    /// The byte offset `position` names in `text`, computed without consulting a [`Buffer`].
    ///
    /// # Panics
    ///
    /// Panics if `text` has no such position.
    fn model_byte_offset(text: &str, position: LogicalPosition) -> usize {
        let mut offset = 0;
        for (index, line) in text.split(LINE_SEPARATOR).enumerate() {
            if index == position.line {
                let within: usize = graphemes(line)
                    .take(position.grapheme)
                    .map(str::len)
                    .sum::<usize>();
                return offset + within;
            }
            offset += line.len() + 1;
        }
        panic!("the model text holds no line {}", position.line);
    }

    /// # Returns
    ///
    /// The byte offset every line of `text` begins at, computed without consulting a [`Buffer`].
    fn model_line_starts(text: &str) -> Vec<usize> {
        let mut starts = Vec::new();
        let mut offset = 0;
        for line in text.split(LINE_SEPARATOR) {
            starts.push(offset);
            offset += line.len() + 1;
        }
        starts
    }

    /// Asserts that a buffer's lines and line index agree with the text it should hold.
    fn assert_consistent(buffer: &Buffer, text: &str) {
        assert_eq!(text, buffer.text());
        assert_eq!(text.len(), buffer.len());
        assert_eq!(
            text.split(LINE_SEPARATOR).collect::<Vec<_>>(),
            buffer.lines()
        );
        assert_eq!(model_line_starts(text), buffer.line_starts);
        assert_eq!(buffer.lines().len(), buffer.line_starts.len());
    }

    /// # Returns
    ///
    /// A strategy drawing text of up to `max_graphemes` graphemes from [`ALPHABET`].
    fn text_strategy(max_graphemes: usize) -> impl Strategy<Value = String> {
        proptest::collection::vec(
            proptest::sample::select(ALPHABET.as_slice()),
            0..=max_graphemes,
        )
        .prop_map(|graphemes| graphemes.concat())
    }

    /// # Returns
    ///
    /// A strategy drawing an edit as its kind, the two raw positions it spans, and the text it
    /// writes. The raw positions are folded onto the buffer being edited when the edit is applied.
    fn edit_strategy() -> impl Strategy<Value = (u8, (usize, usize), (usize, usize), String)> {
        (
            0u8..3,
            (0usize..16, 0usize..16),
            (0usize..16, 0usize..16),
            text_strategy(6),
        )
    }

    /// # Returns
    ///
    /// A position of `buffer` derived from a raw line and grapheme offset by folding each onto
    /// what the buffer holds.
    fn fold_position(buffer: &Buffer, raw: (usize, usize)) -> LogicalPosition {
        let line = raw.0 % buffer.line_count();
        let len = buffer.line_len(line).expect("the folded line exists");
        LogicalPosition {
            line,
            grapheme: raw.1 % (len + 1),
        }
    }

    #[test]
    fn line_indexing_holds_for_an_empty_buffer() {
        let buffer = Buffer::new();

        assert_eq!(Buffer::from_text(""), buffer);
        assert_eq!(1, buffer.line_count());
        assert_eq!(Some(""), buffer.line(0));
        assert_eq!(Some(0), buffer.line_start(0));
        assert_eq!(Some(0), buffer.line_len(0));
        assert_eq!(None, buffer.line(1));
        assert_eq!(None, buffer.line_start(1));
        assert_eq!(None, buffer.line_len(1));
        assert_eq!(0, buffer.len());
        assert!(buffer.is_empty());
        assert!(!Buffer::from_text("a").is_empty());
        assert_eq!("", buffer.text());
        assert_eq!(
            Ok(LogicalPosition {
                line: 0,
                grapheme: 0
            }),
            buffer.position(0)
        );
        assert_eq!(
            Err(Error::ByteOffsetOutOfBounds { offset: 1, len: 0 }),
            buffer.position(1)
        );
    }

    #[test]
    fn line_indexing_holds_without_a_trailing_newline() {
        let buffer = Buffer::from_text("ab\ncde");

        assert_eq!(2, buffer.line_count());
        assert_eq!(vec!["ab", "cde"], buffer.lines());
        assert_eq!(Some(0), buffer.line_start(0));
        assert_eq!(Some(3), buffer.line_start(1));
        assert_eq!(None, buffer.line_start(2));
        assert_eq!(6, buffer.len());
        assert!(!buffer.is_empty());
        assert_eq!("ab\ncde", buffer.text());
        assert_eq!(
            Ok(LogicalPosition {
                line: 1,
                grapheme: 0
            }),
            buffer.position(3)
        );
        assert_eq!(
            Ok(LogicalPosition {
                line: 1,
                grapheme: 3
            }),
            buffer.position(6)
        );
    }

    #[test]
    fn line_indexing_holds_with_a_trailing_newline() {
        let buffer = Buffer::from_text("ab\ncde\n");

        assert_eq!(3, buffer.line_count());
        assert_eq!(vec!["ab", "cde", ""], buffer.lines());
        assert_eq!(Some(0), buffer.line_start(0));
        assert_eq!(Some(3), buffer.line_start(1));
        assert_eq!(Some(7), buffer.line_start(2));
        assert_eq!(Some(0), buffer.line_len(2));
        assert_eq!(7, buffer.len());
        assert_eq!("ab\ncde\n", buffer.text());
        assert_eq!(
            Ok(LogicalPosition {
                line: 2,
                grapheme: 0
            }),
            buffer.position(7)
        );
    }

    #[test]
    fn byte_offsets_round_trip_for_every_position() {
        let buffer = Buffer::from_text(MIXED_TEXT);

        let mut positions = Vec::new();
        let mut offsets = Vec::new();
        for line in 0..buffer.line_count() {
            let len = buffer.line_len(line).expect("the line exists");
            for grapheme in 0..=len {
                let position = LogicalPosition { line, grapheme };
                let offset = buffer.byte_offset(position).expect("the position exists");
                assert_eq!(Ok(position), buffer.position(offset));
                positions.push(position);
                offsets.push(offset);
            }
        }

        let mut round_tripped = Vec::new();
        for offset in 0..=buffer.len() {
            if let Ok(position) = buffer.position(offset) {
                assert_eq!(Ok(offset), buffer.byte_offset(position));
                round_tripped.push(offset);
            }
        }
        assert_eq!(offsets, round_tripped);

        // The text must be multi-byte for the round trip to have said anything: were every
        // grapheme one byte wide, every offset would map and the two directions would be the same
        // function.
        assert!(round_tripped.len() < buffer.len());
        assert_eq!(
            positions.len(),
            MIXED_TEXT
                .split(LINE_SEPARATOR)
                .map(|line| graphemes(line).count() + 1)
                .sum::<usize>()
        );
    }

    #[test]
    fn positions_reject_offsets_inside_a_grapheme_cluster() {
        let buffer = Buffer::from_text(MIXED_TEXT);

        let mut boundaries = Vec::new();
        for line in 0..buffer.line_count() {
            let start = buffer.line_start(line).expect("the line exists");
            let text = buffer.line(line).expect("the line exists");
            boundaries.extend(grapheme_indices(text).map(|(offset, _)| start + offset));
            boundaries.push(start + text.len());
        }

        let mut rejected = 0;
        for offset in 0..=buffer.len() {
            if boundaries.contains(&offset) {
                assert!(buffer.position(offset).is_ok());
            } else {
                assert_eq!(
                    Err(Error::NotAGraphemeBoundary { offset }),
                    buffer.position(offset)
                );
                rejected += 1;
            }
        }
        assert!(0 < rejected);
    }

    #[test]
    fn insert_opens_a_line_at_every_separator() {
        let mut buffer = Buffer::from_text("ab\ncd");

        buffer
            .insert(
                LogicalPosition {
                    line: 0,
                    grapheme: 1,
                },
                "x\ny\nz",
            )
            .expect("the position exists");

        assert_consistent(&buffer, "ax\ny\nzb\ncd");
    }

    #[test]
    fn delete_across_lines_joins_them() {
        let mut buffer = Buffer::from_text("ab\ncd\nef");

        let removed = buffer
            .delete(
                LogicalPosition {
                    line: 0,
                    grapheme: 1,
                }..LogicalPosition {
                    line: 2,
                    grapheme: 1,
                },
            )
            .expect("the range exists");

        assert_eq!("b\ncd\ne", removed);
        assert_consistent(&buffer, "af");
    }

    #[test]
    fn replace_returns_the_text_it_removed() {
        let mut buffer = Buffer::from_text("\u{4E2D}\u{6587}\nab");

        let removed = buffer
            .replace(
                LogicalPosition {
                    line: 0,
                    grapheme: 1,
                }..LogicalPosition {
                    line: 1,
                    grapheme: 1,
                },
                "\u{1F1EF}\u{1F1F5}",
            )
            .expect("the range exists");

        assert_eq!("\u{6587}\na", removed);
        assert_consistent(&buffer, "\u{4E2D}\u{1F1EF}\u{1F1F5}b");
    }

    #[test]
    fn conversions_reject_positions_the_buffer_does_not_hold() {
        let buffer = Buffer::from_text("ab\ncd");
        let past_line = LogicalPosition {
            line: 2,
            grapheme: 0,
        };
        let past_grapheme = LogicalPosition {
            line: 0,
            grapheme: 3,
        };

        assert_eq!(
            Err(Error::LineOutOfBounds {
                line: 2,
                line_count: 2
            }),
            buffer.byte_offset(past_line)
        );
        assert_eq!(
            Err(Error::GraphemeOutOfBounds {
                position: past_grapheme,
                line_len: 2
            }),
            buffer.byte_offset(past_grapheme)
        );
    }

    #[test]
    fn edits_reject_positions_the_buffer_does_not_hold() {
        let mut buffer = Buffer::from_text("ab\ncd");
        let past_line = LogicalPosition {
            line: 2,
            grapheme: 0,
        };
        let past_grapheme = LogicalPosition {
            line: 0,
            grapheme: 3,
        };
        let start = LogicalPosition {
            line: 1,
            grapheme: 1,
        };
        let end = LogicalPosition {
            line: 0,
            grapheme: 1,
        };

        assert_eq!(
            Err(Error::LineOutOfBounds {
                line: 2,
                line_count: 2
            }),
            buffer.insert(past_line, "x")
        );
        assert_eq!(
            Err(Error::GraphemeOutOfBounds {
                position: past_grapheme,
                line_len: 2
            }),
            buffer.insert(past_grapheme, "x")
        );
        assert_eq!(
            Err(Error::RangeInverted { start, end }),
            buffer.delete(start..end)
        );
        assert_consistent(&buffer, "ab\ncd");
    }

    proptest! {
        #[test]
        fn random_edits_keep_the_buffer_and_its_index_consistent(
            initial in text_strategy(24),
            edits in proptest::collection::vec(edit_strategy(), 1..12),
        ) {
            let mut buffer = Buffer::from_text(&initial);
            let mut model = initial.clone();
            assert_consistent(&buffer, &model);

            for (kind, raw_start, raw_end, text) in edits {
                let mut start = fold_position(&buffer, raw_start);
                let mut end = fold_position(&buffer, raw_end);
                if (end.line, end.grapheme) < (start.line, start.grapheme) {
                    mem::swap(&mut start, &mut end);
                }
                if 0 == kind {
                    end = start;
                }
                let written = if 1 == kind { "" } else { text.as_str() };

                let start_offset = model_byte_offset(&model, start);
                let end_offset = model_byte_offset(&model, end);
                let expected_removal = model[start_offset..end_offset].to_owned();
                model.replace_range(start_offset..end_offset, written);

                let removed = if 0 == kind {
                    buffer.insert(start, written).map(|()| String::new())
                } else if 1 == kind {
                    buffer.delete(start..end)
                } else {
                    buffer.replace(start..end, written)
                };
                let removed = removed.expect("a folded range is one the buffer holds");
                if 0 != kind {
                    prop_assert_eq!(&expected_removal, &removed);
                }
                assert_consistent(&buffer, &model);
            }
        }

        #[test]
        fn edits_never_split_a_grapheme_cluster(
            initial in text_strategy(24),
            raw_start in (0usize..16, 0usize..16),
            raw_end in (0usize..16, 0usize..16),
            written in text_strategy(6),
        ) {
            let mut buffer = Buffer::from_text(&initial);
            let mut start = fold_position(&buffer, raw_start);
            let mut end = fold_position(&buffer, raw_end);
            if (end.line, end.grapheme) < (start.line, start.grapheme) {
                mem::swap(&mut start, &mut end);
            }

            // No entry of `ALPHABET` joins the entry before it, and neither does a separator, so
            // the text's clusters are exactly the lines' clusters interleaved with separators. The
            // edit may therefore neither split a cluster nor merge two, and the clusters outside
            // the range must survive it untouched.
            let text = buffer.text();
            let clusters: Vec<&str> = graphemes(&text).collect();
            let start_index = global_grapheme_index(&buffer, start);
            let end_index = global_grapheme_index(&buffer, end);
            let written_clusters: Vec<&str> = graphemes(&written).collect();
            let expected: Vec<&str> = clusters[..start_index]
                .iter()
                .chain(written_clusters.iter())
                .chain(clusters[end_index..].iter())
                .copied()
                .collect();

            let removed = buffer
                .replace(start..end, &written)
                .expect("a folded range is one the buffer holds");

            prop_assert_eq!(
                clusters[start_index..end_index].to_vec(),
                graphemes(&removed).collect::<Vec<_>>()
            );
            let edited = buffer.text();
            prop_assert_eq!(expected, graphemes(&edited).collect::<Vec<_>>());

            // Whatever the edit did, every position of the result still lands on a cluster
            // boundary of its line.
            for line in 0..buffer.line_count() {
                let line_text = buffer.line(line).expect("the line exists");
                let boundaries: Vec<usize> = grapheme_indices(line_text)
                    .map(|(offset, _)| offset)
                    .chain([line_text.len()])
                    .collect();
                let line_start = buffer.line_start(line).expect("the line exists");
                for (grapheme, boundary) in boundaries.iter().enumerate() {
                    let offset = buffer
                        .byte_offset(LogicalPosition { line, grapheme })
                        .expect("the position exists");
                    prop_assert_eq!(*boundary, offset - line_start);
                }
            }
        }
    }

    /// # Returns
    ///
    /// The number of graphemes before `position` in the buffer's whole text, counting each line
    /// separator as one.
    fn global_grapheme_index(buffer: &Buffer, position: LogicalPosition) -> usize {
        let preceding: usize = (0..position.line)
            .map(|line| buffer.line_len(line).expect("the line exists") + 1)
            .sum();
        preceding + position.grapheme
    }
}
