//! Diffing the text an edit replaced against the text it wrote, and drawing the diff the way a
//! reviewer reads one.
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
//!
//! A diff a reader is shown is a [`Drawn`] one: a line naming the file and how many lines were put
//! in and taken away, then the lines that changed under [`CONTEXT_LINES`] unchanged lines either
//! side, with a line standing for the unchanged lines between two changes too far apart to share
//! them. Its lines are numbered by a [`Gutter`] drawn beside the source rather than written in it,
//! and the band a changed line is drawn on is the fill of its row rather than a span of its text,
//! so what a yank takes out of a drawn diff is what it took out of one that was not drawn. Word
//! emphasis costs no more than [`MAX_WORD_CELLS`] a replaced line, and a diff is coloured as the
//! code it is only while its two texts together are no longer than [`MAX_SOURCE`], so that
//! colouring an edit costs what colouring a code block does.
//!
//! Measured in release, drawing four thousand lines against four thousand costs 3.7 MB and 48 ms,
//! because texts that long are not coloured; 2,800 lines against 2,800, which is as long as the
//! highlighter colours, cost 8.3 MB and 206 ms coloured and 3.1 MB and 33 ms where the file names
//! no language; and twenty thousand against twenty thousand, past the alignment's bound, 15 MB
//! and 30 ms.

use std::collections::HashMap;
use std::mem;
use std::ops::Range;

use ratatui::style::{Color, Modifier, Style};
use vbc_layout::buffer::LINE_SEPARATOR;

use crate::chat::highlight::{Language, MAX_SOURCE};
use crate::chat::palette::{
    Palette, Rgb, ADDED_BAND, ADDED_EMPHASIS, COMMENT, GREEN, RED, REMOVED_BAND, REMOVED_EMPHASIS,
};
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

/// The mark the line naming the file an edit was to, and how many lines it put in and took away,
/// is written with.
pub const HEADER: char = '@';

/// The mark the line standing for the unchanged lines between two hunks is written with, which is
/// all that line holds.
pub const ELIDED: char = '\u{22ef}';

/// What a header says where the lines are numbered from the edit's own first line rather than
/// from the file's.
pub const SNIPPET_NOTE: &str = "(numbered within the edit)";

/// How many unchanged lines a drawn diff shows either side of a change.
pub const CONTEXT_LINES: usize = 3;

/// The largest product of the two texts' unmatched lengths that is aligned rather than shown
/// whole, which is four thousand lines against four thousand.
pub const MAX_CELLS: usize = 16_000_000;

/// The largest product of the words a replaced line and its replacement do not open and close
/// with in common that is aligned word by word rather than emphasised whole.
pub const MAX_WORD_CELLS: usize = 4_096;

/// The sign a header writes before the number of lines taken away, which is a minus rather than a
/// hyphen so that it reads as one.
const MINUS: char = '\u{2212}';

/// A diff drawn the way a reviewer reads one: its marked lines as the source of a block, the styles
/// drawing them as its spans, and the gutter numbering them beside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Drawn {
    body: Block,
    gutter: Gutter,
}

impl Drawn {
    /// Factory function.
    ///
    /// Diffs the text `old` an edit to `path` replaced against the text `new` it wrote, as
    /// [`compute`] does, and numbers the lines from the edit's own first line, which the header
    /// says.
    ///
    /// # Returns
    ///
    /// The diff, drawn in the colours `palette` draws.
    #[must_use]
    pub fn of_texts(path: &str, old: &str, new: &str, palette: Palette) -> Self {
        let old = lines(old);
        let new = lines(new);
        let mut numbers = Numbers::default();
        let numbered: Vec<Line<'_>> = marked(&old, &new)
            .into_iter()
            .map(|(mark, text)| {
                let line = Line {
                    mark,
                    text,
                    numbers,
                    sides: numbers,
                };
                numbers = numbers.below(mark);
                line
            })
            .collect();

        draw(
            path,
            &trimmed(numbered),
            Sides {
                old: &old,
                new: &new,
            },
            true,
            palette,
        )
    }

    /// Factory function.
    ///
    /// Draws the patch `hunks` a tool reported applying to `path`, numbered as the file numbers
    /// its lines and with the unchanged lines between two hunks drawn as one line of their own.
    ///
    /// # Returns
    ///
    /// The diff, drawn in the colours `palette` draws.
    #[must_use]
    pub fn of_hunks(path: &str, hunks: &[Hunk], palette: Palette) -> Self {
        let mut shown = Vec::new();
        let mut old = Vec::new();
        let mut new = Vec::new();
        for (index, hunk) in hunks.iter().enumerate() {
            if 0 < index {
                shown.push(Line::elided());
            }

            let mut numbers = Numbers {
                replaced: hunk.old_start.saturating_sub(1),
                written: hunk.new_start.saturating_sub(1),
            };
            for written in &hunk.lines {
                let mut characters = written.chars();
                let Some(mark) = characters.next() else {
                    continue;
                };
                let text = characters.as_str();
                let sides = Numbers {
                    replaced: old.len(),
                    written: new.len(),
                };
                match mark {
                    CONTEXT => {
                        old.push(text);
                        new.push(text);
                    }
                    REMOVED => old.push(text),
                    ADDED => new.push(text),
                    _ => continue,
                }
                shown.push(Line {
                    mark,
                    text,
                    numbers,
                    sides,
                });
                numbers = numbers.below(mark);
            }
        }

        draw(
            path,
            &shown,
            Sides {
                old: &old,
                new: &new,
            },
            false,
            palette,
        )
    }

    /// # Returns
    ///
    /// The block the diff is written as, and the gutter numbering its lines.
    #[must_use]
    pub fn into_parts(self) -> (Block, Gutter) {
        (self.body, self.gutter)
    }
}

/// How many lines of the replaced text and of the written text stand above a line of a diff.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Numbers {
    replaced: usize,
    written: usize,
}

impl Numbers {
    #[must_use]
    pub fn replaced(&self) -> usize {
        self.replaced
    }

    #[must_use]
    pub fn written(&self) -> usize {
        self.written
    }

    /// # Returns
    ///
    /// The numbers of the line below a line marked `mark` that these are the numbers of.
    fn below(self, mark: char) -> Self {
        match mark {
            CONTEXT => Self {
                replaced: self.replaced + 1,
                written: self.written + 1,
            },
            REMOVED => Self {
                replaced: self.replaced + 1,
                ..self
            },
            ADDED => Self {
                written: self.written + 1,
                ..self
            },
            _ => self,
        }
    }
}

/// The numbers a drawn diff writes beside its lines, and the chrome a row of it is drawn with.
///
/// A line is numbered as the text it belongs to numbers it: a line both texts hold carries both
/// numbers, a line taken away the replaced text's, and a line put in the written text's. The
/// header, the line saying the texts were not aligned and the line standing for unchanged lines
/// between hunks belong to neither and carry no number.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Gutter {
    digits: usize,
    numbers: Vec<Numbers>,
    number: Style,
    removed: Style,
    added: Style,
}

impl Gutter {
    /// # Returns
    ///
    /// The number of columns the gutter takes in front of a line it numbers.
    #[must_use]
    pub fn width(&self) -> usize {
        2 * self.digits + 2
    }

    /// # Returns
    ///
    /// How many lines of each text stand above the logical line `line` of the diff, which is
    /// marked `mark`, or `None` where the gutter numbers no such line.
    #[must_use]
    pub fn numbered(&self, line: usize, mark: char) -> Option<Numbers> {
        if ![CONTEXT, REMOVED, ADDED].contains(&mark) {
            return None;
        }

        self.numbers.get(line).copied()
    }

    /// # Returns
    ///
    /// What the gutter writes in front of the first row of the logical line `line`, which is
    /// marked `mark`, and which is [`Gutter::width`] columns wide; or `None` where the gutter
    /// numbers no such line.
    #[must_use]
    pub fn label(&self, line: usize, mark: char) -> Option<String> {
        let numbers = self.numbered(line, mark)?;
        let digits = self.digits;
        let cell = |shown: bool, above: usize| {
            if shown {
                format!("{:>digits$}", above + 1)
            } else {
                " ".repeat(digits)
            }
        };

        Some(format!(
            "{} {} ",
            cell(ADDED != mark, numbers.replaced),
            cell(REMOVED != mark, numbers.written)
        ))
    }

    /// # Returns
    ///
    /// The style the gutter of a line marked `mark` is drawn in.
    #[must_use]
    pub fn decoration(&self, mark: char) -> Style {
        match mark {
            CONTEXT | REMOVED | ADDED => self.number,
            _ => Style::default(),
        }
    }

    /// # Returns
    ///
    /// The style a row of a line marked `mark` is filled with across its whole width: the band a
    /// line taken away or put in is drawn on, and nothing for any other line.
    #[must_use]
    pub fn fill(&self, mark: char) -> Style {
        match mark {
            REMOVED => self.removed,
            ADDED => self.added,
            _ => Style::default(),
        }
    }
}

/// One hunk of a patch a tool reported applying: the line of each text it starts at, counted from
/// one, and its lines, each under the mark a unified diff writes it with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hunk {
    old_start: usize,
    new_start: usize,
    lines: Vec<String>,
}

impl Hunk {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The hunk starting at line `old_start` of the replaced text and line `new_start` of the
    /// written text, holding `lines`.
    #[must_use]
    pub fn new(old_start: usize, new_start: usize, lines: Vec<String>) -> Self {
        Self {
            old_start,
            new_start,
            lines,
        }
    }
}

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
    write(&marked(&lines(old), &lines(new)))
}

/// One line of a diff being drawn: the mark it is written under, its text, how many lines of each
/// text are numbered above it, and where it stands among the lines of each side it was drawn
/// from.
#[derive(Clone, Copy, Debug)]
struct Line<'text> {
    mark: char,
    text: &'text str,
    numbers: Numbers,
    sides: Numbers,
}

impl Line<'_> {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// The line standing for the unchanged lines between two hunks.
    fn elided() -> Self {
        Self {
            mark: ELIDED,
            text: "",
            numbers: Numbers::default(),
            sides: Numbers::default(),
        }
    }
}

/// The lines of the replaced text and of the written text a diff was drawn from, which is what its
/// lines are coloured as the code they are.
#[derive(Clone, Copy, Debug)]
struct Sides<'sides, 'text> {
    old: &'sides [&'text str],
    new: &'sides [&'text str],
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

/// Aligns the lines of the two texts, while [`MAX_CELLS`] allows, and marks every line of the
/// diff.
///
/// # Returns
///
/// Each line of the diff under the mark it is written with, in the order it is written.
fn marked<'text>(old: &[&'text str], new: &[&'text str]) -> Vec<(char, &'text str)> {
    let (before, after) = identify(old, new);
    let head = common_head(&before, &after);
    let tail = common_tail(&before[head..], &after[head..]);
    let old_middle = &before[head..before.len() - tail];
    let new_middle = &after[head..after.len() - tail];

    let bounded = MAX_CELLS < old_middle.len().saturating_mul(new_middle.len());
    let mut matched = Vec::new();
    if !bounded {
        align(old_middle, new_middle, 0, 0, &mut matched);
    }

    marks(old, new, &matched, head, tail, bounded)
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
/// `lines` with every unchanged line further than [`CONTEXT_LINES`] from a change left out, and a
/// line of [`ELIDED`] standing for each run of them between two lines that are kept. A diff that
/// changed nothing keeps every line, because there is no change to show them around.
fn trimmed(lines: Vec<Line<'_>>) -> Vec<Line<'_>> {
    let changes: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| CONTEXT != line.mark)
        .map(|(at, _)| at)
        .collect();
    if changes.is_empty() {
        return lines;
    }

    let mut shown = Vec::new();
    let mut nearest = 0;
    let mut kept: Option<usize> = None;
    for (at, line) in lines.into_iter().enumerate() {
        while changes
            .get(nearest)
            .is_some_and(|change| change + CONTEXT_LINES < at)
        {
            nearest += 1;
        }
        let near = changes
            .get(nearest)
            .is_some_and(|change| *change <= at + CONTEXT_LINES);
        if !near {
            continue;
        }

        if kept.is_some_and(|kept| kept + 1 < at) {
            shown.push(Line::elided());
        }
        kept = Some(at);
        shown.push(line);
    }

    shown
}

/// Writes the diff `shown` of an edit to `path` as a block, under a header naming the file and
/// what the edit changed there, and numbers its lines in a gutter.
///
/// `sides` holds the lines each of `shown` names by its sides, which are coloured as the code the
/// extension of `path` names. `snippet` says the lines are numbered from the edit's own first line,
/// which the header then says as well.
///
/// # Returns
///
/// The diff, drawn in the colours `palette` draws.
fn draw(
    path: &str,
    shown: &[Line<'_>],
    sides: Sides<'_, '_>,
    snippet: bool,
    palette: Palette,
) -> Drawn {
    let length = |lines: &[&str]| lines.iter().map(|line| line.len() + 1).sum::<usize>();
    let language =
        Language::of_path(path).filter(|_| length(sides.old) + length(sides.new) <= MAX_SOURCE);
    let old_colours = highlighted(sides.old, language, palette);
    let new_colours = highlighted(sides.new, language, palette);
    let changed = emphasised(shown);
    let emphases = [
        (REMOVED, band(palette, REMOVED_EMPHASIS)),
        (ADDED, band(palette, ADDED_EMPHASIS)),
    ];
    let marks = [
        (REMOVED, Style::new().fg(palette.color(RED))),
        (ADDED, Style::new().fg(palette.color(GREEN))),
        (ELIDED, Style::new().fg(palette.color(COMMENT))),
    ];

    let mut text = String::new();
    let mut spans = Vec::new();
    header(&mut text, &mut spans, path, shown, snippet, palette);
    let mut numbers = Vec::with_capacity(1 + shown.len());
    numbers.push(Numbers::default());

    for (line, words) in shown.iter().zip(&changed) {
        let range = push(&mut text, line.mark, line.text);
        numbers.push(line.numbers);
        if BOUNDED == line.mark {
            spans.push(Span::new(range, unaligned()));
            continue;
        }

        let body = range.start + line.mark.len_utf8();
        if let Some((_, style)) = marks.iter().find(|(mark, _)| *mark == line.mark) {
            spans.push(Span::new(range.start..body, *style));
        }

        let colours = match line.mark {
            REMOVED => old_colours.get(line.sides.replaced),
            CONTEXT | ADDED => new_colours.get(line.sides.written),
            _ => None,
        };
        let emphasis = emphases
            .iter()
            .find(|(mark, _)| *mark == line.mark)
            .map_or_else(Style::default, |(_, style)| *style);
        layered(
            colours.map_or(&[], Vec::as_slice),
            words,
            emphasis,
            body,
            &mut spans,
        );
    }

    let widest = shown
        .iter()
        .map(|line| match line.mark {
            CONTEXT => 1 + line.numbers.replaced.max(line.numbers.written),
            REMOVED => 1 + line.numbers.replaced,
            ADDED => 1 + line.numbers.written,
            _ => 0,
        })
        .max()
        .unwrap_or_default();

    Drawn {
        body: Block::with_spans(text, spans),
        gutter: Gutter {
            digits: widest.to_string().len(),
            numbers,
            number: Style::new().fg(palette.color(COMMENT)),
            removed: band(palette, REMOVED_BAND),
            added: band(palette, ADDED_BAND),
        },
    }
}

/// Writes the header of a diff of an edit to `path` to `text`, and the spans styling it to
/// `spans`: the file, how many lines of `shown` were put in and taken away, and, where `snippet`
/// says so, that the lines are numbered from the edit's own first line.
fn header(
    text: &mut String,
    spans: &mut Vec<Span>,
    path: &str,
    shown: &[Line<'_>],
    snippet: bool,
    palette: Palette,
) {
    let counted = |marked: char| shown.iter().filter(|line| marked == line.mark).count();
    let dim = Style::new().fg(palette.color(COMMENT));
    let mut written = |part: &str, style: Style| {
        let start = text.len();
        text.push_str(part);
        spans.push(Span::new(start..text.len(), style));
    };

    written(&HEADER.to_string(), dim);
    written(" ", Style::default());
    written(path, Style::new().add_modifier(Modifier::BOLD));
    written("  ", Style::default());
    written(
        &format!("+{}", counted(ADDED)),
        Style::new().fg(palette.color(GREEN)),
    );
    written(" ", Style::default());
    written(
        &format!("{MINUS}{}", counted(REMOVED)),
        Style::new().fg(palette.color(RED)),
    );
    if snippet {
        written("  ", Style::default());
        written(SNIPPET_NOTE, dim);
    }
}

/// # Returns
///
/// The spans colouring each of `lines` as the code `language` names, relative to the start of the
/// line, which are none at all where there is no language.
fn highlighted(lines: &[&str], language: Option<Language>, palette: Palette) -> Vec<Vec<Span>> {
    let Some(language) = language else {
        return Vec::new();
    };

    let mut starts = Vec::with_capacity(lines.len());
    let mut start = 0;
    for line in lines {
        starts.push(start);
        start += line.len() + LINE_SEPARATOR.len_utf8();
    }
    let text = lines.join(&LINE_SEPARATOR.to_string());

    let mut coloured = vec![Vec::new(); lines.len()];
    let mut line = 0;
    for span in language.spans(&text, palette) {
        let range = span.range();
        while starts
            .get(line + 1)
            .is_some_and(|next| *next <= range.start)
        {
            line += 1;
        }

        let mut at = line;
        while at < lines.len() && starts[at] < range.end {
            let first = range.start.max(starts[at]);
            let last = range.end.min(starts[at] + lines[at].len());
            if first < last {
                coloured[at].push(Span::new(
                    first - starts[at]..last - starts[at],
                    span.style(),
                ));
            }
            at += 1;
        }
    }

    coloured
}

/// # Returns
///
/// The words of each of `shown` that a replaced line changed, relative to the start of the line: a
/// run of lines taken away followed by a run put in pairs its lines off in order, and each pair is
/// emphasised where the two differ. A line in no pair, and every line of a diff whose texts were
/// not aligned, has none.
fn emphasised(shown: &[Line<'_>]) -> Vec<Vec<Range<usize>>> {
    let mut changed = vec![Vec::new(); shown.len()];
    if shown.iter().any(|line| BOUNDED == line.mark) {
        return changed;
    }

    let mut at = 0;
    while at < shown.len() {
        let taken = at;
        while shown.get(at).is_some_and(|line| REMOVED == line.mark) {
            at += 1;
        }
        let put = at;
        while shown.get(at).is_some_and(|line| ADDED == line.mark) {
            at += 1;
        }
        if taken == at {
            at += 1;
            continue;
        }

        for pair in 0..(put - taken).min(at - put) {
            let (before, after) = words_changed(shown[taken + pair].text, shown[put + pair].text);
            changed[taken + pair] = before;
            changed[put + pair] = after;
        }
    }

    changed
}

/// Aligns the words of a line taken away with the words of the line put in its place.
///
/// The words both open and close with are matched off first, and what is left between them is
/// aligned by the dynamic program for their longest common subsequence while [`MAX_WORD_CELLS`]
/// allows, and taken as changed whole where it does not.
///
/// # Returns
///
/// The ranges of `old` and of `new` holding the words that changed, adjacent words joined; or none
/// of either where the two lines share no word that is not a blank, because a line replaced whole
/// is said by its band alone.
fn words_changed(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let before = words(old);
    let after = words(new);
    let same = |one: &Range<usize>, other: &Range<usize>| old[one.clone()] == new[other.clone()];
    let head = before
        .iter()
        .zip(&after)
        .take_while(|(one, other)| same(one, other))
        .count();
    let tail = before[head..]
        .iter()
        .rev()
        .zip(after[head..].iter().rev())
        .take_while(|(one, other)| same(one, other))
        .count();
    let old_middle = &before[head..before.len() - tail];
    let new_middle = &after[head..after.len() - tail];

    let mut old_kept = vec![false; old_middle.len()];
    let mut new_kept = vec![false; new_middle.len()];
    if old_middle.len().saturating_mul(new_middle.len()) <= MAX_WORD_CELLS {
        let columns = new_middle.len() + 1;
        let mut table = vec![0_usize; (old_middle.len() + 1) * columns];
        for one in (0..old_middle.len()).rev() {
            for other in (0..new_middle.len()).rev() {
                table[one * columns + other] = if same(&old_middle[one], &new_middle[other]) {
                    table[(one + 1) * columns + other + 1] + 1
                } else {
                    table[(one + 1) * columns + other].max(table[one * columns + other + 1])
                };
            }
        }

        let (mut one, mut other) = (0, 0);
        while one < old_middle.len() && other < new_middle.len() {
            if same(&old_middle[one], &new_middle[other]) {
                old_kept[one] = true;
                new_kept[other] = true;
                one += 1;
                other += 1;
            } else if table[(one + 1) * columns + other] >= table[one * columns + other + 1] {
                one += 1;
            } else {
                other += 1;
            }
        }
    }

    let worded = |range: &Range<usize>| !old[range.clone()].trim().is_empty();
    let shares = before[..head].iter().any(worded)
        || before[before.len() - tail..].iter().any(worded)
        || old_middle
            .iter()
            .zip(&old_kept)
            .any(|(range, kept)| *kept && worded(range));
    if !shares {
        return (Vec::new(), Vec::new());
    }

    (joined(old_middle, &old_kept), joined(new_middle, &new_kept))
}

/// # Returns
///
/// The byte ranges of the words of `text`: every run of letters, digits and underscores, every run
/// of blanks, and every other character alone, which between them cover the whole of it.
fn words(text: &str) -> Vec<Range<usize>> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Class {
        Word,
        Blank,
        Other,
    }
    let class = |character: char| {
        if character.is_alphanumeric() || '_' == character {
            Class::Word
        } else if character.is_whitespace() {
            Class::Blank
        } else {
            Class::Other
        }
    };

    let mut words: Vec<Range<usize>> = Vec::new();
    let mut previous = None;
    for (at, character) in text.char_indices() {
        let current = class(character);
        let end = at + character.len_utf8();
        match words.last_mut() {
            Some(word) if Some(current) == previous && Class::Other != current => word.end = end,
            _ => words.push(at..end),
        }
        previous = Some(current);
    }

    words
}

/// # Returns
///
/// The ranges covered by the words of `words` that `kept` does not keep, adjacent words joined
/// into one range.
fn joined(words: &[Range<usize>], kept: &[bool]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (word, kept) in words.iter().zip(kept) {
        if *kept {
            continue;
        }
        match ranges.last_mut() {
            Some(range) if range.end == word.start => range.end = word.end,
            _ => ranges.push(word.clone()),
        }
    }

    ranges
}

/// Appends to `spans` the spans a line's text is drawn in, which start at byte `offset` of the
/// diff's source: the colours `colours` gives its code, with the words `words` emphasises drawn
/// on `emphasis` beneath them.
///
/// `colours` and `words` are both ascending and disjoint and relative to the line, and what is
/// appended is ascending and disjoint as well, so no span of the block is painted over another.
fn layered(
    colours: &[Span],
    words: &[Range<usize>],
    emphasis: Style,
    offset: usize,
    spans: &mut Vec<Span>,
) {
    let mut edges: Vec<usize> = colours
        .iter()
        .flat_map(|span| [span.range().start, span.range().end])
        .chain(words.iter().flat_map(|range| [range.start, range.end]))
        .collect();
    edges.sort_unstable();
    edges.dedup();

    let (mut colour, mut word) = (0, 0);
    for pair in edges.windows(2) {
        let &[start, end] = pair else {
            continue;
        };
        while colours
            .get(colour)
            .is_some_and(|span| span.range().end <= start)
        {
            colour += 1;
        }
        while words.get(word).is_some_and(|range| range.end <= start) {
            word += 1;
        }

        let mut style = Style::new();
        if let Some(span) = colours
            .get(colour)
            .filter(|span| span.range().start <= start)
        {
            style = style.patch(span.style());
        }
        if words.get(word).is_some_and(|range| range.start <= start) {
            style = style.patch(emphasis);
        }
        if Style::new() == style {
            continue;
        }

        let range = offset + start..offset + end;
        match spans.last_mut() {
            Some(last) if last.range().end == range.start && last.style() == style => {
                *last = Span::new(last.range().start..range.end, style);
            }
            _ => spans.push(Span::new(range, style)),
        }
    }
}

/// # Returns
///
/// A style drawing the cells it covers on `rgb`, as `palette` draws it.
fn band(palette: Palette, rgb: Rgb) -> Style {
    Style::new().bg(palette.color(rgb))
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
    use std::ops::Range;

    use ratatui::style::{Color, Style};

    use crate::chat::palette::{
        Palette, ADDED_BAND, ADDED_EMPHASIS, REMOVED_BAND, REMOVED_EMPHASIS,
    };
    use crate::style::Span;

    use super::{
        added, compute, removed, unaligned, Drawn, Gutter, Hunk, Numbers, ADDED, BOUNDED_NOTE,
        CONTEXT, ELIDED, HEADER, REMOVED, SNIPPET_NOTE,
    };

    /// The number of lines each side of the diff that is bounded rather than aligned, which is one
    /// more line each side than [`MAX_CELLS`] allows.
    const OVER_THE_BOUND: usize = 4_001;

    /// The file the drawn fixtures are an edit to, which names a language the highlighter colours.
    const PATH: &str = "src/main.rs";

    /// A line of that file that an edit changes one word of, and the line it changes it to.
    const GREETED: &str = "    let greeting = \"hello\";";
    const REGREETED: &str = "    let greeting = \"goodbye\";";

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

    #[test]
    fn a_drawn_diff_opens_with_the_file_and_what_the_edit_changed_there() {
        let drawn = Drawn::of_texts(PATH, "a\nb\nc\n", "a\nB\nc\nd\n", Palette::Indexed);
        let (body, _) = drawn.into_parts();

        assert_eq!(
            Some(format!("{HEADER} {PATH}  +2 \u{2212}1  {SNIPPET_NOTE}").as_str()),
            body.source().lines().next()
        );

        let reported = Drawn::of_hunks(
            PATH,
            &[Hunk::new(
                4,
                4,
                vec![" a".to_owned(), "-b".to_owned(), "+B".to_owned()],
            )],
            Palette::Indexed,
        );
        let (body, _) = reported.into_parts();
        assert_eq!(
            Some(format!("{HEADER} {PATH}  +1 \u{2212}1").as_str()),
            body.source().lines().next(),
            "a diff numbered as the file numbers it said it was numbered within the edit"
        );
    }

    #[test]
    fn a_drawn_diff_keeps_three_unchanged_lines_either_side_and_elides_the_rest() {
        let old = text(1..21);
        let mut new = text(1..21).replace("line 2\n", "second\n");
        new = new.replace("line 19\n", "nineteenth\n");
        let (body, _) = Drawn::of_texts(PATH, &old, &new, Palette::Indexed).into_parts();

        assert_eq!(
            vec![
                " line 1",
                "-line 2",
                "+second",
                " line 3",
                " line 4",
                " line 5",
                &ELIDED.to_string(),
                " line 16",
                " line 17",
                " line 18",
                "-line 19",
                "+nineteenth",
                " line 20",
            ],
            body.source().lines().skip(1).collect::<Vec<&str>>()
        );
    }

    #[test]
    fn two_changes_six_lines_apart_are_drawn_as_one_hunk_and_seven_apart_as_two() {
        for (gap, elided) in [(6, false), (7, true)] {
            let old = text(0..2 + gap);
            let new = old
                .replace("line 0\n", "first\n")
                .replace(&line(1 + gap), "last\n");
            let (body, _) = Drawn::of_texts(PATH, &old, &new, Palette::Indexed).into_parts();

            assert_eq!(
                elided,
                body.source().lines().any(|line| ELIDED.to_string() == line),
                "two changes {gap} unchanged lines apart were drawn wrongly: {:?}",
                body.source()
            );
        }
    }

    #[test]
    fn a_gutter_numbers_each_line_as_the_text_it_belongs_to_numbers_it() {
        let hunks = [
            Hunk::new(
                9,
                9,
                vec![
                    " kept".to_owned(),
                    "-taken".to_owned(),
                    "+put".to_owned(),
                    "+more".to_owned(),
                    " kept".to_owned(),
                    "\\ No newline at end of file".to_owned(),
                ],
            ),
            Hunk::new(100, 101, vec!["-gone".to_owned()]),
        ];
        let (body, gutter) = Drawn::of_hunks(PATH, &hunks, Palette::Indexed).into_parts();

        let labels: Vec<Option<String>> = body
            .source()
            .lines()
            .enumerate()
            .map(|(index, line)| {
                let mark = line.chars().next().expect("no line of a diff is empty");
                gutter.label(index, mark)
            })
            .collect();
        assert_eq!(
            vec![
                None,
                Some("  9   9 ".to_owned()),
                Some(" 10     ".to_owned()),
                Some("     10 ".to_owned()),
                Some("     11 ".to_owned()),
                Some(" 11  12 ".to_owned()),
                None,
                Some("100     ".to_owned()),
            ],
            labels
        );
        assert_eq!(8, gutter.width());
        assert_eq!(
            Some(Numbers {
                replaced: 99,
                written: 100
            }),
            gutter.numbered(7, REMOVED)
        );
    }

    #[test]
    fn a_changed_row_is_filled_with_its_band_and_an_unchanged_one_is_not() {
        let (_, gutter) = Drawn::of_texts(PATH, "a\n", "b\n", Palette::Indexed).into_parts();

        assert_eq!(
            Style::new().bg(Palette::Indexed.color(REMOVED_BAND)),
            gutter.fill(REMOVED)
        );
        assert_eq!(
            Style::new().bg(Palette::Indexed.color(ADDED_BAND)),
            gutter.fill(ADDED)
        );
        assert_eq!(Style::default(), gutter.fill(CONTEXT));
        assert_eq!(Style::default(), gutter.fill(HEADER));
    }

    #[test]
    fn a_one_word_change_emphasises_that_word_and_nothing_else_on_the_line() {
        for palette in [Palette::Indexed, Palette::Truecolor] {
            let old = format!("fn main() {{\n{GREETED}\n}}\n");
            let new = format!("fn main() {{\n{REGREETED}\n}}\n");
            let (body, _) = Drawn::of_texts(PATH, &old, &new, palette).into_parts();

            assert_eq!(
                vec!["hello"],
                emphasised(&body, palette.color(REMOVED_EMPHASIS))
            );
            assert_eq!(
                vec!["goodbye"],
                emphasised(&body, palette.color(ADDED_EMPHASIS))
            );
        }
    }

    #[test]
    fn a_line_replaced_whole_emphasises_no_word_of_it() {
        let (body, _) = Drawn::of_texts(PATH, "alpha\n", "omega\n", Palette::Indexed).into_parts();

        assert_eq!(
            Vec::<&str>::new(),
            emphasised(&body, Palette::Indexed.color(REMOVED_EMPHASIS))
        );
    }

    #[test]
    fn a_drawn_diff_colours_its_code_as_the_extension_names_it() {
        let (body, _) = Drawn::of_texts(
            PATH,
            &format!("{GREETED}\n"),
            &format!("{REGREETED}\n"),
            Palette::Indexed,
        )
        .into_parts();
        let plain = Drawn::of_texts(
            "notes.txt",
            &format!("{GREETED}\n"),
            &format!("{REGREETED}\n"),
            Palette::Indexed,
        );
        let (plain, _) = plain.into_parts();

        let coloured = |source: &str, spans: &[Span]| {
            let at = source.find("let").expect("the fixture holds a keyword");
            spans
                .iter()
                .any(|span| span.range().contains(&at) && span.style().fg.is_some())
        };
        assert!(
            coloured(body.source(), body.spans()),
            "the keyword of a Rust edit was not coloured"
        );
        assert!(
            !coloured(plain.source(), plain.spans()),
            "the text of an edit to a plain file was coloured"
        );
    }

    #[test]
    fn a_drawn_diff_holds_every_changed_line_under_the_mark_compute_writes_it_with() {
        for (old, new) in cases() {
            let computed = compute(&old, &new);
            let (drawn, _) = Drawn::of_texts(PATH, &old, &new, Palette::Indexed).into_parts();
            let changes = |source: &str| -> Vec<String> {
                source
                    .lines()
                    .filter(|line| line.starts_with([REMOVED, ADDED]))
                    .map(str::to_owned)
                    .collect()
            };

            assert_eq!(
                changes(computed.source()),
                changes(drawn.source()),
                "drawing {old:?} against {new:?} changed which lines it marks"
            );
        }
    }

    #[test]
    fn a_gutter_is_as_wide_as_its_widest_number() {
        let gutter = |hunk: Hunk| -> Gutter {
            Drawn::of_hunks(PATH, &[hunk], Palette::Indexed)
                .into_parts()
                .1
        };

        assert_eq!(4, gutter(Hunk::new(1, 1, vec!["-a".to_owned()])).width());
        assert_eq!(
            10,
            gutter(Hunk::new(9_999, 1, vec!["-a".to_owned()])).width()
        );
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
    fn text(range: Range<usize>) -> String {
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

    /// # Returns
    ///
    /// The text of `block` drawn on the background `band`, each run of it joined into one.
    fn emphasised(block: &crate::style::Block, band: Color) -> Vec<&str> {
        let mut runs: Vec<Range<usize>> = Vec::new();
        for span in block.spans() {
            if Some(band) != span.style().bg {
                continue;
            }
            match runs.last_mut() {
                Some(run) if run.end == span.range().start => run.end = span.range().end,
                _ => runs.push(span.range().clone()),
            }
        }

        runs.into_iter()
            .map(|run| {
                block
                    .slice(run)
                    .expect("a span names a range of the source")
            })
            .collect()
    }
}
