//! Diffing the text an edit replaced against the text it wrote.
//!
//! An edit reaches a transcript as the text before it and the text after it, and reprinting both
//! whole says almost nothing: what a reader wants is the lines that changed. The diff is computed
//! here rather than read out of prose, and what it computes is a block like any other -- the
//! marked lines as its source, the marks' colours as its spans -- so a yank of a diff yields the
//! diff a reader saw rather than a rendering of one.
//!
//! The lines kept in common are the longest common subsequence of the two texts, found by the
//! obvious dynamic program: O(n * m) time and space for texts of n and m lines. A transcript diffs
//! one edit at a time and the texts either side of one are small, so nothing here trades clarity
//! for a tighter bound.
//!
//! Where a line was replaced, the line taken away is written before the line put in its place, as
//! a unified diff writes it.

use std::ops::Range;

use ratatui::style::{Color, Style};
use vbc_layout::buffer::LINE_SEPARATOR;

use crate::style::{Block, Span};

/// The mark a line both texts hold is written with.
pub const CONTEXT: char = ' ';

/// The mark a line only the replaced text holds is written with.
pub const REMOVED: char = '-';

/// The mark a line only the written text holds is written with.
pub const ADDED: char = '+';

/// Diffs the text an edit replaced against the text it wrote.
///
/// # Returns
///
/// A block of the marked lines of the diff, the lines taken away and the lines put in styled
/// apart from the lines the two texts share.
#[must_use]
pub fn compute(old: &str, new: &str) -> Block {
    let old = lines(old);
    let new = lines(new);
    let width = new.len() + 1;
    let common = table(&old, &new);

    let mut text = String::new();
    let mut spans = Vec::new();
    let (mut before, mut after) = (0, 0);
    while before < old.len() || after < new.len() {
        let kept = before < old.len() && after < new.len() && old[before] == new[after];
        let taken = !kept
            && before < old.len()
            && (after == new.len()
                || common[(before + 1) * width + after] >= common[before * width + after + 1]);

        if kept {
            push(&mut text, CONTEXT, old[before]);
            before += 1;
            after += 1;
        } else if taken {
            let range = push(&mut text, REMOVED, old[before]);
            spans.push(Span::new(range, removed()));
            before += 1;
        } else {
            let range = push(&mut text, ADDED, new[after]);
            spans.push(Span::new(range, added()));
            after += 1;
        }
    }

    Block::with_spans(text, spans)
}

/// # Returns
///
/// The lines of `text`, which are none for an empty text and which never include the empty line a
/// trailing separator would otherwise leave behind.
fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<&str> = text.split(LINE_SEPARATOR).collect();
    if Some(&"") == lines.last() {
        lines.pop();
    }

    lines
}

/// # Returns
///
/// The length of the longest common subsequence of each pair of suffixes of `old` and `new`, the
/// row for a suffix of `old` laid out beside the next.
fn table(old: &[&str], new: &[&str]) -> Vec<usize> {
    let width = new.len() + 1;
    let mut table = vec![0; (old.len() + 1) * width];
    for before in (0..old.len()).rev() {
        for after in (0..new.len()).rev() {
            table[before * width + after] = if old[before] == new[after] {
                table[(before + 1) * width + after + 1] + 1
            } else {
                table[(before + 1) * width + after].max(table[before * width + after + 1])
            };
        }
    }

    table
}

/// Writes `line` to `text` under the mark `marker`, separated from the line before it.
///
/// # Returns
///
/// The byte range of `text` the marked line occupies, its separator excluded.
fn push(text: &mut String, marker: char, line: &str) -> Range<usize> {
    if !text.is_empty() {
        text.push(LINE_SEPARATOR);
    }

    let start = text.len();
    text.push(marker);
    text.push_str(line);

    start..text.len()
}

/// # Returns
///
/// The style a line only the replaced text holds is drawn in.
fn removed() -> Style {
    Style::new().fg(Color::Red)
}

/// # Returns
///
/// The style a line only the written text holds is drawn in.
fn added() -> Style {
    Style::new().fg(Color::Green)
}

#[cfg(test)]
mod tests {
    use crate::style::Span;

    use super::{added, compute, removed};

    #[test]
    fn a_replaced_line_is_taken_away_before_it_is_put_back() {
        let block = compute("keep\nold\ntail\n", "keep\nnew\ntail\n");

        assert_eq!(" keep\n-old\n+new\n tail", block.source());
        assert_eq!(
            &[Span::new(6..10, removed()), Span::new(11..15, added())],
            block.spans()
        );
    }

    #[test]
    fn an_insertion_marks_only_the_lines_put_in() {
        let block = compute("a\nb\n", "a\nmiddle\nb\n");

        assert_eq!(" a\n+middle\n b", block.source());
        assert_eq!(&[Span::new(3..10, added())], block.spans());
    }

    #[test]
    fn a_deletion_marks_only_the_lines_taken_away() {
        let block = compute("a\nmiddle\nb\n", "a\nb\n");

        assert_eq!(" a\n-middle\n b", block.source());
        assert_eq!(&[Span::new(3..10, removed())], block.spans());
    }

    #[test]
    fn identical_texts_diff_to_context_alone() {
        let block = compute("a\nb\n", "a\nb\n");

        assert_eq!(" a\n b", block.source());
        assert_eq!(&[] as &[Span], block.spans());
    }

    #[test]
    fn a_text_replaced_wholesale_keeps_no_context() {
        let block = compute("one\ntwo\n", "three\nfour\n");

        assert_eq!("-one\n-two\n+three\n+four", block.source());
        assert_eq!(
            &[
                Span::new(0..4, removed()),
                Span::new(5..9, removed()),
                Span::new(10..16, added()),
                Span::new(17..22, added()),
            ],
            block.spans()
        );
    }

    #[test]
    fn every_span_names_the_line_it_marks() {
        let block = compute("keep\nold\n", "keep\nnew\n");
        let marked: Vec<&str> = block
            .spans()
            .iter()
            .map(|span| {
                block
                    .slice(span.range().clone())
                    .expect("a span names a range of the source")
            })
            .collect();

        assert_eq!(vec!["-old", "+new"], marked);
    }

    #[test]
    fn a_trailing_separator_adds_no_line_of_its_own() {
        assert_eq!(compute("a\n", "b\n").source(), compute("a", "b").source());
    }

    #[test]
    fn a_line_left_empty_is_a_line_of_the_diff() {
        let block = compute("a\n\n", "a\n");

        assert_eq!(" a\n-", block.source());
        assert_eq!(&[Span::new(3..4, removed())], block.spans());
    }

    #[test]
    fn diffing_nothing_against_nothing_leaves_an_empty_block() {
        let block = compute("", "");

        assert_eq!("", block.source());
        assert_eq!(&[] as &[Span], block.spans());
    }

    #[test]
    fn a_text_written_where_there_was_none_is_added_whole() {
        let block = compute("", "fresh\n");

        assert_eq!("+fresh", block.source());
        assert_eq!(&[Span::new(0..6, added())], block.spans());
    }
}
