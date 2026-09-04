//! Taking what a transcript said out of it, as the thing it is rather than as the picture of it.
//!
//! This is what the block model was for. A reader who has found the code Claude wrote wants that
//! code in a file or in a pull request, and what a terminal offers them is a rectangle of cells:
//! the numbers down the left, the mark a diff writes its lines under, the row a closed fold is
//! collapsed to, the colours, and a break through the middle of every line longer than the panel
//! was wide. None of that is what was said. It is what what was said was drawn as.
//!
//! Nothing here strips any of it, because nothing here reads a drawn row. A selection and an
//! object are both ranges of the source a block holds, so a yank is a slice of that string: a
//! gutter is drawn beside the rows rather than written among the bytes, a fold is a state rather
//! than a character, a style names a byte range rather than living in one, the ANSI escapes were
//! read as styles when the block was built, and a wrapped line is one logical line however many
//! rows it was drawn in. What a yank cannot pick up is what a yank never touches.
//!
//! Four things are addressed by keys of their own: `yac` takes the code block the cursor is in,
//! `yam` the message, `yat` what a tool answered, and `yad` the diff. Each takes the thing without
//! what delimits it -- the code between the fence lines rather than the fences, the prose without
//! the blank lines around it -- because a reader yanking a code block means to paste code.
//!
//! A diff is the one block whose source is not what an edit wrote. Its lines are marked, and the
//! mark is the diff's own gutter, so whole lines taken out of a diff come back as the text those
//! lines hold. `yad` is the exception and takes the diff as a unified patch, headers, hunks and
//! line numbers, because a diff worth taking out of a transcript is one a reader means to apply.
//!
//! A yank of more than one block writes their sources one after another, separated by the one line
//! break that separates them on the screen, and that is what a closed fold yanks as well: the
//! blocks it covers, whole, rather than the row its summary is drawn in. A fold hides what is
//! drawn and never what is held.
//!
//! Where a yank lands is the [`Registers`] the file editor puts from, because a reader who yanks a
//! code block out of an answer means to paste it into a file. A plain `y` fills the unnamed
//! register, the yank register and `"+`, so that what a reader took reaches the system clipboard as
//! well as the editor's own. A plain `p` reads the unnamed register and nothing besides: a vim user
//! who yanks and then puts expects what they yanked rather than whatever the desktop last copied,
//! so the mirroring is one way on purpose.

use std::ops::Range;

use vbc_layout::buffer::LINE_SEPARATOR;
use vbc_layout::width::Metrics;

use crate::chat::block::{self, Block};
use crate::chat::diff;
use crate::chat::fold::Fold;
use crate::chat::object::{Kind as ObjectKind, Object, Position, Scope};
use crate::chat::selection::{Mode, Selection, Source};
use crate::chat::transcript::Transcript;
use crate::clipboard;
use crate::engine::{Held, Registers, Shape};

/// What is written between the sources of two blocks a yank spans, which is the one line break the
/// rows of the second follow the rows of the first across.
pub const BLOCK_SEPARATOR: char = LINE_SEPARATOR;

/// The register a yank fills and a put reads back.
pub const UNNAMED: char = '"';

/// The register holding the last thing yanked, as vim's own does.
pub const YANK: char = '0';

/// The register standing for the system clipboard, which a yank in the transcript panel mirrors
/// into and which a put never reads.
pub const CLIPBOARD: char = clipboard::REGISTER;

/// What a patch names the file either side of the edit, whose leading component `git apply` strips
/// before it looks for the file.
const OLD_PREFIX: &str = "a/";
const NEW_PREFIX: &str = "b/";

/// How many unchanged lines a hunk carries either side of what changed, which is what a unified
/// diff carries.
const HUNK_CONTEXT: usize = 3;

/// What a hunk header is written between.
const HUNK_MARK: &str = "@@";

/// What a transcript hands over whole.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Structure {
    /// `yac`: the code of the fenced region or the code block the cursor is in.
    Code,

    /// `yam`: the message the cursor is in.
    Message,

    /// `yat`: what a tool answered.
    ToolResult,

    /// `yad`: the diff the cursor is in, written as a patch.
    Diff,
}

impl Structure {
    /// # Returns
    ///
    /// The text object the structure resolves through, or `None` for the diff, which is no object
    /// of a transcript and is named by the block the cursor is in instead.
    fn object(self) -> Option<Object> {
        let kind = match self {
            Self::Code => ObjectKind::Code,
            Self::Message => ObjectKind::Message,
            Self::ToolResult => ObjectKind::ToolResult,
            Self::Diff => return None,
        };

        Some(Object::new(Scope::Inner, kind))
    }
}

/// What was taken out of a transcript: the text, and the layout a put would reinsert it with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Yank {
    text: String,
    shape: Shape,
}

impl Yank {
    /// Takes the structure `structure` at `at`.
    ///
    /// # Returns
    ///
    /// What was taken, or `None` where the transcript holds nothing of that structure there: no
    /// such block, a position past the last byte of one, or a position in no object of the kind.
    /// [`Structure::Diff`] names the block itself wherever in it the cursor sits, and takes
    /// nothing where that block is no diff or is no diff a patch can be written from.
    #[must_use]
    pub fn structural(transcript: &Transcript, at: Position, structure: Structure) -> Option<Self> {
        let block = transcript.block(at.block())?;
        let Some(object) = structure.object() else {
            return Some(Self {
                text: patch(block)?,
                shape: Shape::Linewise,
            });
        };
        let region = object.resolve(transcript, at)?;

        Some(Self {
            text: written(block.kind(), region.text(transcript)?),
            shape: Shape::Linewise,
        })
    }

    /// Takes what `selection` covers of `block`, which is what a plain `y` takes.
    ///
    /// The selection's own coordinates are the block's source, so the metrics are all this needs
    /// of the panel: the width the block was drawn at decides where the rows broke and decides
    /// nothing about what is taken.
    ///
    /// # Returns
    ///
    /// What was taken.
    #[must_use]
    pub fn selected(block: &Block, selection: &Selection, metrics: Metrics) -> Self {
        let source = Source::new(block.source(), metrics);
        let text = selection.text(source);
        let shape = match selection.mode() {
            Mode::Charwise => Shape::Charwise,
            Mode::Linewise => Shape::Linewise,
            Mode::Blockwise => Shape::Blockwise,
        };
        let text = match shape {
            Shape::Linewise => written(block.kind(), &text),
            Shape::Charwise | Shape::Blockwise => text,
        };

        Self { text, shape }
    }

    /// Takes the blocks of `transcript` from `first` to `last`, whole.
    ///
    /// # Returns
    ///
    /// What was taken, or `None` where the transcript holds no block of that run.
    #[must_use]
    pub fn spanning(transcript: &Transcript, first: usize, last: usize) -> Option<Self> {
        Self::joined(transcript, first.min(last)..=first.max(last))
    }

    /// Takes what the fold `fold` covers, whole, however much of it is drawn.
    ///
    /// # Returns
    ///
    /// What was taken, or `None` where the transcript holds none of the blocks the fold covers.
    #[must_use]
    pub fn folded(transcript: &Transcript, fold: &Fold) -> Option<Self> {
        Self::joined(transcript, fold.covered().iter().copied())
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// # Returns
    ///
    /// What was taken of the blocks `blocks` of `transcript`, whole and separated by
    /// [`BLOCK_SEPARATOR`], or `None` where the transcript holds none of them.
    ///
    /// # Type Parameters
    ///
    /// * `BlocksType` - The indices of the blocks to take, in the order they are written.
    fn joined<BlocksType: IntoIterator<Item = usize>>(
        transcript: &Transcript,
        blocks: BlocksType,
    ) -> Option<Self> {
        let mut text = String::new();
        let mut taken = 0;
        for index in blocks {
            let Some(block) = transcript.block(index) else {
                continue;
            };
            if 0 < taken {
                text.push(BLOCK_SEPARATOR);
            }
            text.push_str(&written(block.kind(), block.source()));
            taken += 1;
        }

        (0 < taken).then_some(Self {
            text,
            shape: Shape::Linewise,
        })
    }
}

/// Files `yank` in `registers`, in every register a yank in the transcript panel fills: the unnamed
/// register, the yank register and the clipboard's, which is how what a reader took leaves the
/// editor for the desktop.
///
/// The registers are the file editor's own, so what is filed here is what `p` in the file puts.
/// A linewise register holds whole lines, the last of them ended as every other one is, because a
/// put lays the register's bytes down between two lines rather than between two lines and a break
/// it supplies itself. A structure taken out of a transcript ends where the structure ends and a
/// code block's last line carries no break of its own, so one is written here or `p` in the middle
/// of a file joins the last line of what was yanked to the line it was put above.
pub fn file(registers: &Registers, yank: &Yank) {
    let mut text = yank.text.clone();
    if yank.shape == Shape::Linewise && !text.ends_with(LINE_SEPARATOR) {
        text.push(LINE_SEPARATOR);
    }
    let held = Held {
        text,
        shape: yank.shape,
    };
    for name in [UNNAMED, YANK, CLIPBOARD] {
        registers.fill(name, &held);
    }
}

/// Writes the diff `block` holds as a unified patch.
///
/// The marked lines hold everything a patch needs but the numbers: a line the two texts share
/// advances both, a line taken away advances the replaced text alone and a line put in advances
/// the written text alone, so the hunks and their line numbers are counted off the marks. What is
/// written is the runs that changed under [`HUNK_CONTEXT`] unchanged lines either side, the way a
/// unified diff writes them.
///
/// # Returns
///
/// The patch, or `None` where the block is no diff, names no file, changed nothing, or is one of
/// the diffs [`diff::compute`] declined to align and showed whole instead.
#[must_use]
pub fn patch(block: &Block) -> Option<String> {
    let block::Kind::Diff { path } = block.kind() else {
        return None;
    };
    if path.is_empty() {
        return None;
    }

    let lines = numbered(block.source())?;
    let covered = hunks(&lines);
    if covered.is_empty() {
        return None;
    }

    let mut unified = format!("--- {OLD_PREFIX}{path}\n+++ {NEW_PREFIX}{path}\n");
    for hunk in covered {
        unified.push_str(&header(&lines[hunk.clone()]));
        for line in &lines[hunk] {
            unified.push(line.mark);
            unified.push_str(line.text);
            unified.push(LINE_SEPARATOR);
        }
    }

    Some(unified)
}

/// One line of a diff, read back out of the marked source the diff was written as.
struct Line<'block> {
    mark: char,
    text: &'block str,
    old: usize,
    new: usize,
}

/// # Returns
///
/// The text a yank of whole lines of a block of `kind` hands back, which for a diff is those lines
/// without the mark the diff wrote each of them under and for every other kind is `text` itself.
fn written(kind: &block::Kind, text: &str) -> String {
    if !matches!(kind, block::Kind::Diff { .. }) {
        return text.to_owned();
    }

    let mut written = String::with_capacity(text.len());
    for (index, line) in text.split(LINE_SEPARATOR).enumerate() {
        if 0 < index {
            written.push(LINE_SEPARATOR);
        }
        let mark = line.chars().next().map_or(0, char::len_utf8);
        written.push_str(&line[mark..]);
    }

    written
}

/// Reads the marked lines of a diff back, counting how many lines of each text stand above every
/// one of them.
///
/// # Returns
///
/// The lines of the diff, or `None` where `source` holds a line under no mark of a diff or under
/// the mark saying the two texts were shown rather than aligned.
fn numbered(source: &str) -> Option<Vec<Line<'_>>> {
    if source.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    let (mut old, mut new) = (0, 0);
    for line in source.split(LINE_SEPARATOR) {
        let mark = line.chars().next()?;
        let text = line.get(mark.len_utf8()..)?;
        lines.push(Line {
            mark,
            text,
            old,
            new,
        });
        match mark {
            diff::CONTEXT => {
                old += 1;
                new += 1;
            }
            diff::REMOVED => old += 1,
            diff::ADDED => new += 1,
            _ => return None,
        }
    }

    Some(lines)
}

/// # Returns
///
/// The lines each hunk of the patch covers, in the order they are written, which are the runs that
/// changed with [`HUNK_CONTEXT`] unchanged lines either side, runs whose contexts meet written as
/// one hunk.
fn hunks(lines: &[Line<'_>]) -> Vec<Range<usize>> {
    let mut hunks: Vec<Range<usize>> = Vec::new();
    for (at, _) in lines
        .iter()
        .enumerate()
        .filter(|(_, line)| diff::CONTEXT != line.mark)
    {
        let start = at.saturating_sub(HUNK_CONTEXT);
        let end = (at + HUNK_CONTEXT + 1).min(lines.len());
        match hunks.last_mut() {
            Some(last) if start <= last.end => last.end = last.end.max(end),
            _ => hunks.push(start..end),
        }
    }

    hunks
}

/// # Returns
///
/// The header the hunk covering `lines` is written under, which names the line each text's part of
/// the hunk starts at and how many lines of it the hunk covers. A hunk taking no line of a text at
/// all names the line it was written after, as a unified diff does.
///
/// # Panics
///
/// Panics if `lines` is empty, which no hunk is.
fn header(lines: &[Line<'_>]) -> String {
    let first = lines.first().expect("a hunk covers a line");
    let old = lines.iter().filter(|line| diff::ADDED != line.mark).count();
    let new = lines
        .iter()
        .filter(|line| diff::REMOVED != line.mark)
        .count();
    let start = |at: usize, count: usize| if 0 == count { at } else { at + 1 };

    format!(
        "{HUNK_MARK} -{},{old} +{},{new} {HUNK_MARK}\n",
        start(first.old, old),
        start(first.new, new)
    )
}

#[cfg(test)]
mod tests {
    use vbc_layout::width::Metrics;

    use crate::chat::block::{Block, Kind, Role};
    use crate::chat::fold::{Folds, Tag};
    use crate::chat::object::Position;
    use crate::chat::selection::{Mode, Motion, Selection, Source};
    use crate::chat::transcript::Transcript;
    use crate::engine::{Held, Registers, Shape};

    use super::{file, patch, Structure, Yank, CLIPBOARD, LINE_SEPARATOR, UNNAMED, YANK};

    /// The blocks of the fixture transcript, named by where they sit in it.
    const ASKED: usize = 0;
    const ANSWERED: usize = 1;
    const CALLED: usize = 2;
    const RESULT: usize = 3;
    const EDIT: usize = 4;

    /// The file the fixture's edit was to.
    const PATH: &str = "src/main.rs";

    /// The text that edit replaced, and the text it wrote.
    const BEFORE: &str = "fn main() {}\n";
    const AFTER: &str = "fn main() {\n    todo!();\n}\n";

    #[test]
    fn a_structural_yank_takes_the_thing_without_what_delimits_it() {
        let transcript = said();

        assert_eq!(
            Some("fn main() {\n    todo!();\n}"),
            yanked(&transcript, ANSWERED, 30, Structure::Code).as_deref(),
            "`yac` did not take the code between the fences"
        );
        assert_eq!(
            Some("make the panel wrap"),
            yanked(&transcript, ASKED, 0, Structure::Message).as_deref(),
            "`yam` did not take the message without the blank lines around it"
        );
        assert_eq!(
            Some("error: nope\ndone"),
            yanked(&transcript, RESULT, 0, Structure::ToolResult).as_deref(),
            "`yat` did not take what the tool answered"
        );
    }

    #[test]
    fn a_structural_yank_of_what_the_cursor_is_not_in_takes_nothing() {
        let transcript = said();

        for (block, structure) in [
            (ASKED, Structure::Code),
            (ASKED, Structure::ToolResult),
            (ASKED, Structure::Diff),
            (CALLED, Structure::Message),
            (RESULT, Structure::Message),
            (EDIT, Structure::Message),
        ] {
            assert_eq!(
                None,
                yanked(&transcript, block, 0, structure),
                "{structure:?} took something out of the block at {block}"
            );
        }

        assert_eq!(None, yanked(&transcript, 99, 0, Structure::Message));
    }

    #[test]
    fn a_yank_of_whole_lines_of_a_diff_leaves_the_mark_the_diff_wrote_them_under_behind() {
        let block = Block::diff(PATH.to_owned(), BEFORE, AFTER);
        let source = Source::new(block.source(), Metrics::default());
        let mut selection = Selection::new(Mode::Linewise, source, 0);
        selection.extend(source, Motion::Down(3));

        assert_eq!(
            "-fn main() {}\n+fn main() {\n+    todo!();\n+}",
            block.source(),
            "the fixture's diff is not the one the marks are being taken off"
        );
        assert_eq!(
            "fn main() {}\nfn main() {\n    todo!();\n}",
            Yank::selected(&block, &selection, Metrics::default()).text()
        );
    }

    #[test]
    fn a_charwise_yank_takes_the_columns_it_was_pointed_at() {
        let block = Block::new(Kind::Message(Role::User), "make it compile".to_owned());
        let source = Source::new(block.source(), Metrics::default());
        let mut selection = Selection::new(Mode::Charwise, source, 5);
        selection.extend(source, Motion::Right(1));
        let yank = Yank::selected(&block, &selection, Metrics::default());

        assert_eq!("it", yank.text());
        assert_eq!(Shape::Charwise, yank.shape());
    }

    #[test]
    fn a_yank_of_more_than_one_block_writes_them_the_way_the_rows_separate_them() {
        let transcript = said();
        let yank = Yank::spanning(&transcript, ASKED, CALLED).expect("the fixture holds the run");

        assert_eq!(
            concat!(
                "make the panel wrap\n",
                "here is the fix\n",
                "\n",
                "```rust\n",
                "fn main() {\n",
                "    todo!();\n",
                "}\n",
                "```\n",
                "cargo test",
            ),
            yank.text()
        );
        assert_eq!(
            Yank::spanning(&transcript, CALLED, ASKED),
            Yank::spanning(&transcript, ASKED, CALLED),
            "the ends of a span were not sorted"
        );
        assert_eq!(None, Yank::spanning(&transcript, 98, 99));
    }

    #[test]
    fn a_yank_of_a_fold_takes_every_block_it_covers() {
        let transcript = said();
        let tags = vec![Tag::untagged(); transcript.len()];
        let folds = Folds::of(&transcript, &tags);
        let fold = folds.at(CALLED).expect("a call to a tool heads a fold");

        assert_eq!(
            Some("cargo test"),
            Yank::folded(&transcript, fold).as_ref().map(Yank::text)
        );
    }

    #[test]
    fn a_patch_is_written_only_for_a_diff_that_names_a_file_and_changed_something() {
        assert_eq!(
            None,
            patch(&Block::new(Kind::Message(Role::User), "no".to_owned()))
        );
        assert_eq!(None, patch(&Block::diff(String::new(), BEFORE, AFTER)));
        assert_eq!(None, patch(&Block::diff(PATH.to_owned(), AFTER, AFTER)));
        assert_eq!(None, patch(&Block::diff(PATH.to_owned(), "", "")));
    }

    #[test]
    fn a_patch_carries_the_context_a_unified_diff_carries_and_numbers_it_from_one() {
        let old = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
        let new = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nTEN\n";
        let block = Block::diff(PATH.to_owned(), old, new);

        assert_eq!(
            Some(concat!(
                "--- a/src/main.rs\n",
                "+++ b/src/main.rs\n",
                "@@ -7,4 +7,4 @@\n",
                " seven\n",
                " eight\n",
                " nine\n",
                "-ten\n",
                "+TEN\n",
            )),
            patch(&block).as_deref()
        );
    }

    #[test]
    fn a_patch_over_two_far_apart_changes_is_written_in_two_hunks() {
        let lines: Vec<String> = (1..=20).map(|number| format!("line {number}")).collect();
        let old = format!("{}\n", lines.join("\n"));
        let mut written = lines;
        written[0] = "first".to_owned();
        written[19] = "last".to_owned();
        let new = format!("{}\n", written.join("\n"));
        let patched = patch(&Block::diff(PATH.to_owned(), &old, &new))
            .expect("the fixture's diff is a patch");

        assert_eq!(
            vec!["@@ -1,4 +1,4 @@", "@@ -17,4 +17,4 @@"],
            patched
                .lines()
                .filter(|line| line.starts_with("@@"))
                .collect::<Vec<&str>>()
        );
    }

    #[test]
    fn a_diff_the_texts_were_too_large_to_align_for_is_no_patch() {
        let block = Block::with_spans(
            Kind::Diff {
                path: PATH.to_owned(),
            },
            "!the texts are too large to align, so both are shown whole\n-old\n+new".to_owned(),
            Vec::new(),
        );

        assert_eq!(None, patch(&block));
    }

    #[test]
    fn a_yank_fills_the_unnamed_the_yank_and_the_clipboard_registers() {
        let registers = Registers::new();
        let transcript = said();
        let yank = Yank::structural(&transcript, Position::new(RESULT, 0), Structure::ToolResult)
            .expect("the fixture holds a tool result");
        file(&registers, &yank);

        let held = Held {
            text: format!("{}{LINE_SEPARATOR}", yank.text()),
            shape: yank.shape(),
        };
        for name in [UNNAMED, YANK, CLIPBOARD] {
            assert_eq!(Some(held.clone()), registers.get(name));
        }
    }

    #[test]
    fn a_linewise_yank_is_filed_as_whole_lines_and_a_charwise_one_as_it_was_taken() {
        let transcript = said();
        let block = transcript
            .block(ASKED)
            .expect("the fixture holds a question");
        let source = Source::new(block.source(), Metrics::default());
        for (mode, ended) in [(Mode::Linewise, true), (Mode::Charwise, false)] {
            let registers = Registers::new();
            let selection = Selection::new(mode, source, 0);
            file(
                &registers,
                &Yank::selected(block, &selection, Metrics::default()),
            );
            let held = registers
                .get(UNNAMED)
                .expect("the yank filled the register");

            assert_eq!(
                ended,
                held.text.ends_with(LINE_SEPARATOR),
                "a {mode:?} yank was filed as {:?}",
                held.text
            );
        }
    }

    /// # Returns
    ///
    /// The text `structure` takes at byte `offset` of the block at `block` of `transcript`.
    fn yanked(
        transcript: &Transcript,
        block: usize,
        offset: usize,
        structure: Structure,
    ) -> Option<String> {
        Yank::structural(transcript, Position::new(block, offset), structure)
            .map(|yank| yank.text().to_owned())
    }

    /// # Returns
    ///
    /// A short exchange holding one block of every kind a structural yank addresses.
    fn said() -> Transcript {
        [
            Block::new(Kind::Message(Role::User), "make the panel wrap".to_owned()),
            Block::new(
                Kind::Message(Role::Assistant),
                concat!(
                    "here is the fix\n",
                    "\n",
                    "```rust\n",
                    "fn main() {\n",
                    "    todo!();\n",
                    "}\n",
                    "```",
                )
                .to_owned(),
            ),
            Block::new(
                Kind::ToolCall {
                    name: "Bash".to_owned(),
                },
                "cargo test".to_owned(),
            ),
            Block::from_ansi(Kind::ToolResult, "\u{1b}[1;31merror\u{1b}[0m: nope\ndone"),
            Block::diff(PATH.to_owned(), BEFORE, AFTER),
        ]
        .into_iter()
        .collect()
    }
}
