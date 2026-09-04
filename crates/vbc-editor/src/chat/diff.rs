//! Diffing the text an edit replaced against the text it wrote.
//!
//! An edit reaches a transcript as the text before it and the text after it, and reprinting both
//! whole says almost nothing: what a reader wants is the lines that changed. The diff is computed
//! here rather than read out of prose, and what it computes is a block like any other -- the
//! marked lines as its source, the marks' colours as its spans -- so a yank of a diff yields the
//! diff a reader saw rather than a rendering of one.
//!
//! An edit is to whatever file was edited, so the size of the texts is that file's business rather
//! than the transcript's, and the memory the diff takes is the thing that has to be bounded. The
//! lines kept in common are the longest common subsequence of the two texts, found by Hirschberg's
//! divide and conquer, which holds two rows of the dynamic program rather than the whole table:
//! linear in the lines of one text rather than quadratic in the lines of both. The runs the two
//! texts open and close with are matched off before that starts, which is what makes an edit to a
//! few lines of a long file cost those lines.
//!
//! Time is bounded by [`MAX_CELLS`] rather than by the algorithm, because linear space does not
//! make a quadratic walk quick: two texts whose product of lengths is larger than that are not
//! aligned at all, and the diff says so in a line of its own rather than spending a second of a
//! frame's budget saying the same thing more finely.
//!
//! Measured in release, with no line in common so that nothing is matched off first: a thousand
//! lines against a thousand cost 3.0 ms and 588 KB, two thousand against two thousand 16 ms and
//! 1.2 MB, and four thousand against four thousand -- the bound -- 56 ms and 2.4 MB, most of which
//! is the eight thousand marked lines the diff hands back rather than the alignment. Past the
//! bound nothing is aligned: twenty thousand lines against twenty thousand cost 10 ms and 11 MB,
//! which is the two texts written out. An edit of one line into a twenty-thousand-line file is
//! matched off to a middle of one line either side and costs 3.6 ms.
//!
//! Where a line was replaced, the line taken away is written before the line put in its place, as
//! a unified diff writes it.

use std::collections::HashMap;
use std::mem;
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

/// The mark the line saying the texts were shown rather than aligned is written with.
pub const BOUNDED: char = '!';

/// What that line says.
pub const BOUNDED_NOTE: &str = "the texts are too large to align, so both are shown whole";

/// The largest product of the two texts' unmatched lengths that is aligned rather than shown
/// whole, which is four thousand lines against four thousand.
pub const MAX_CELLS: usize = 16_000_000;

/// Diffs the text an edit replaced against the text it wrote.
///
/// The texts are aligned only while the product of the lengths their common opening and closing
/// runs leave behind is at most [`MAX_CELLS`]. Past that the diff marks every line the alignment
/// would have read away and back in again, under a line of its own saying that is what it did.
///
/// # Returns
///
/// A block of the marked lines of the diff, the lines taken away and the lines put in styled
/// apart from the lines the two texts share.
#[must_use]
pub fn compute(old: &str, new: &str) -> Block {
    let old = lines(old);
    let new = lines(new);
    let (before, after) = identify(&old, &new);
    let head = common_head(&before, &after);
    let tail = common_tail(&before[head..], &after[head..]);
    let old_middle = &before[head..before.len() - tail];
    let new_middle = &after[head..after.len() - tail];

    let bounded = MAX_CELLS < old_middle.len().saturating_mul(new_middle.len());
    let mut matched = Vec::new();
    if !bounded {
        align(old_middle, new_middle, 0, 0, &mut matched);
    }

    write(&marks(&old, &new, &matched, head, tail, bounded))
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

/// Gives every distinct line of the two texts a number of its own, so that the alignment compares
/// numbers rather than strings.
///
/// # Returns
///
/// The lines of the replaced text and the lines of the written text, as those numbers.
fn identify<'text>(old: &[&'text str], new: &[&'text str]) -> (Vec<usize>, Vec<usize>) {
    let mut numbers: HashMap<&'text str, usize> = HashMap::new();
    let mut before = Vec::with_capacity(old.len());
    let mut after = Vec::with_capacity(new.len());
    for (text, numbered) in [(old, &mut before), (new, &mut after)] {
        for line in text {
            let next = numbers.len();
            numbered.push(*numbers.entry(*line).or_insert(next));
        }
    }

    (before, after)
}

/// # Returns
///
/// The number of lines the two texts open with in common.
fn common_head(old: &[usize], new: &[usize]) -> usize {
    old.iter()
        .zip(new)
        .take_while(|(one, other)| one == other)
        .count()
}

/// # Returns
///
/// The number of lines the two texts close with in common.
fn common_tail(old: &[usize], new: &[usize]) -> usize {
    old.iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(one, other)| one == other)
        .count()
}

/// Matches the lines the two texts hold in common, by Hirschberg's divide and conquer over the
/// dynamic program for their longest common subsequence.
///
/// `old_at` and `new_at` are the positions the two slices start at within the texts they were cut
/// from, which is what the pairs are reported in. The pairs are appended to `matched` in ascending
/// order.
fn align(
    old: &[usize],
    new: &[usize],
    old_at: usize,
    new_at: usize,
    matched: &mut Vec<(usize, usize)>,
) {
    if old.is_empty() || new.is_empty() {
        return;
    }
    if 1 == old.len() {
        if let Some(at) = new.iter().position(|line| *line == old[0]) {
            matched.push((old_at, new_at + at));
        }

        return;
    }

    let middle = old.len() / 2;
    let split = {
        let head = prefix_lengths(&old[..middle], new);
        let tail = suffix_lengths(&old[middle..], new);
        let mut longest = 0;
        let mut split = 0;
        for at in 0..=new.len() {
            let total = head[at] + tail[at];
            if longest < total {
                longest = total;
                split = at;
            }
        }

        split
    };

    align(&old[..middle], &new[..split], old_at, new_at, matched);
    align(
        &old[middle..],
        &new[split..],
        old_at + middle,
        new_at + split,
        matched,
    );
}

/// # Returns
///
/// The length of the longest common subsequence of `old` and each prefix of `new`, indexed by the
/// length of that prefix.
fn prefix_lengths(old: &[usize], new: &[usize]) -> Vec<usize> {
    let mut previous = vec![0; new.len() + 1];
    let mut current = vec![0; new.len() + 1];
    for one in old {
        for at in 0..new.len() {
            current[at + 1] = if *one == new[at] {
                previous[at] + 1
            } else {
                current[at].max(previous[at + 1])
            };
        }
        mem::swap(&mut previous, &mut current);
    }

    previous
}

/// # Returns
///
/// The length of the longest common subsequence of `old` and each suffix of `new`, indexed by the
/// position that suffix starts at.
fn suffix_lengths(old: &[usize], new: &[usize]) -> Vec<usize> {
    let mut previous = vec![0; new.len() + 1];
    let mut current = vec![0; new.len() + 1];
    for one in old.iter().rev() {
        for at in (0..new.len()).rev() {
            current[at] = if *one == new[at] {
                previous[at + 1] + 1
            } else {
                current[at + 1].max(previous[at])
            };
        }
        mem::swap(&mut previous, &mut current);
    }

    previous
}

/// Marks every line of the diff: the lines the two texts share, the lines taken away, and the
/// lines put in.
///
/// `head` and `tail` are the numbers of lines the texts open and close with in common, which were
/// matched off before the alignment ran and which `matched` is therefore relative to.
///
/// # Returns
///
/// Each line of the diff under the mark it is written with, in the order it is written.
fn marks<'text>(
    old: &[&'text str],
    new: &[&'text str],
    matched: &[(usize, usize)],
    head: usize,
    tail: usize,
    bounded: bool,
) -> Vec<(char, &'text str)> {
    let old_middle = &old[head..old.len() - tail];
    let new_middle = &new[head..new.len() - tail];
    let mut marked: Vec<(char, &str)> = old[..head].iter().map(|line| (CONTEXT, *line)).collect();
    if bounded {
        marked.push((BOUNDED, BOUNDED_NOTE));
    }

    let (mut before, mut after) = (0, 0);
    for &(one, other) in matched {
        marked.extend(old_middle[before..one].iter().map(|line| (REMOVED, *line)));
        marked.extend(new_middle[after..other].iter().map(|line| (ADDED, *line)));
        marked.push((CONTEXT, old_middle[one]));
        before = one + 1;
        after = other + 1;
    }
    marked.extend(old_middle[before..].iter().map(|line| (REMOVED, *line)));
    marked.extend(new_middle[after..].iter().map(|line| (ADDED, *line)));
    marked.extend(old[old.len() - tail..].iter().map(|line| (CONTEXT, *line)));

    marked
}

/// # Returns
///
/// A block of the marked lines, each written under its mark and styled by what that mark says.
fn write(marked: &[(char, &str)]) -> Block {
    let mut text = String::new();
    let mut spans = Vec::new();
    for &(mark, line) in marked {
        let range = push(&mut text, mark, line);
        match mark {
            REMOVED => spans.push(Span::new(range, removed())),
            ADDED => spans.push(Span::new(range, added())),
            BOUNDED => spans.push(Span::new(range, unaligned())),
            _ => {}
        }
    }

    Block::with_spans(text, spans)
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

/// # Returns
///
/// The style the line saying the texts were shown rather than aligned is drawn in.
fn unaligned() -> Style {
    Style::new().fg(Color::Yellow)
}

#[cfg(test)]
mod tests {
    use crate::style::Span;

    use super::{added, compute, removed, unaligned, BOUNDED_NOTE};

    /// The number of lines each side of the diff that is bounded rather than aligned, which is one
    /// more line each side than [`MAX_CELLS`] allows.
    const OVER_THE_BOUND: usize = 4_001;

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

    #[test]
    fn a_line_moved_across_a_long_text_is_matched_rather_than_reprinted() {
        let old = text(0..64);
        let new: String = text(1..64) + &line(0);

        let block = compute(&old, &new);
        let marked: Vec<&str> = marked(&block);

        assert_eq!(vec!["-line 0", "+line 0"], marked);
    }

    #[test]
    fn texts_too_large_to_align_are_shown_whole_under_a_line_saying_so() {
        let old = text(0..OVER_THE_BOUND);
        let new = text(OVER_THE_BOUND..2 * OVER_THE_BOUND);

        let block = compute(&old, &new);

        let first = block
            .source()
            .lines()
            .next()
            .expect("a bounded diff holds a line of its own");
        assert_eq!(format!("!{BOUNDED_NOTE}"), first);
        assert_eq!(
            Some(&Span::new(0..1 + BOUNDED_NOTE.len(), unaligned())),
            block.spans().first()
        );
        assert_eq!(1 + 2 * OVER_THE_BOUND, block.source().lines().count());
    }

    #[test]
    fn a_common_head_and_tail_keep_a_long_edit_under_the_bound() {
        let head = text(0..OVER_THE_BOUND);
        let old = head.clone() + &line(1_000_000);
        let new = head + &line(2_000_000);

        let block = compute(&old, &new);

        assert_eq!(vec!["-line 1000000", "+line 2000000"], marked(&block));
    }

    #[test]
    fn the_alignment_keeps_as_many_lines_as_the_dynamic_program_does() {
        for (old, new) in cases() {
            let block = compute(&old, &new);
            let kept = block
                .source()
                .lines()
                .filter(|line| line.starts_with(' '))
                .count();

            assert_eq!(
                longest_common_subsequence(&old, &new),
                kept,
                "diffing {old:?} against {new:?} kept {kept} lines"
            );
        }
    }

    #[test]
    fn every_diff_replays_into_the_text_it_was_written_from() {
        for (old, new) in cases() {
            let block = compute(&old, &new);
            let (before, after) = replayed(block.source());

            assert_eq!(
                (trimmed(&old), trimmed(&new)),
                (before, after),
                "diffing {old:?} against {new:?} did not replay"
            );
        }
    }

    /// # Returns
    ///
    /// The pairs of texts the alignment is checked over, which are every pair of a small set of
    /// short texts drawn from the same few lines.
    fn cases() -> Vec<(String, String)> {
        let texts = [
            "",
            "a\n",
            "b\n",
            "a\nb\n",
            "b\na\n",
            "a\na\n",
            "a\nb\nc\n",
            "c\nb\na\n",
            "a\nb\nc\nd\n",
            "a\nx\nc\ny\n",
            "d\nc\nb\na\n",
            "a\na\nb\nb\n",
        ];

        texts
            .iter()
            .flat_map(|old| {
                texts
                    .iter()
                    .map(move |new| ((*old).to_owned(), (*new).to_owned()))
            })
            .collect()
    }

    /// # Returns
    ///
    /// The length of the longest common subsequence of the lines of the two texts, by the dynamic
    /// program the divide and conquer replaces.
    fn longest_common_subsequence(old: &str, new: &str) -> usize {
        let old: Vec<&str> = trimmed(old);
        let new: Vec<&str> = trimmed(new);
        let mut table = vec![vec![0; new.len() + 1]; old.len() + 1];
        for before in (0..old.len()).rev() {
            for after in (0..new.len()).rev() {
                table[before][after] = if old[before] == new[after] {
                    table[before + 1][after + 1] + 1
                } else {
                    table[before + 1][after].max(table[before][after + 1])
                };
            }
        }

        table[0][0]
    }

    /// # Returns
    ///
    /// The two texts a diff was written from, read back off its marked lines.
    fn replayed(diff: &str) -> (Vec<&str>, Vec<&str>) {
        let mut old = Vec::new();
        let mut new = Vec::new();
        for line in diff.lines() {
            let (mark, rest) = line.split_at(1);
            match mark {
                " " => {
                    old.push(rest);
                    new.push(rest);
                }
                "-" => old.push(rest),
                "+" => new.push(rest),
                _ => panic!("a diff holds no line marked {mark:?}"),
            }
        }

        (old, new)
    }

    /// # Returns
    ///
    /// The lines of `text`, without the empty line a trailing separator leaves behind.
    fn trimmed(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    /// # Returns
    ///
    /// The lines numbered by `range`, each ended by a separator.
    fn text(range: std::ops::Range<usize>) -> String {
        range.map(line).collect()
    }

    /// # Returns
    ///
    /// The line numbered `number`, ended by a separator.
    fn line(number: usize) -> String {
        format!("line {number}\n")
    }

    /// # Returns
    ///
    /// The text behind every span of `block`, which is every line it marked.
    fn marked(block: &crate::style::Block) -> Vec<&str> {
        block
            .spans()
            .iter()
            .map(|span| {
                block
                    .slice(span.range().clone())
                    .expect("a span names a range of the source")
            })
            .collect()
    }
}
