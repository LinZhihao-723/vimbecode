//! What a reader takes out of a transcript, held against what the panel drew.
//!
//! The claim the block model was built for is that a yank is content and never the picture of it,
//! and the way to hold that claim honest is to draw the picture first. So every one of these cases
//! renders the panel a reader is looking at -- the numbers in the gutter, the marks a diff writes
//! its lines under, the row a closed fold is collapsed to, the styles a segment carries and the
//! places a narrow panel broke the lines -- shows that the thing is there on the screen, and then
//! requires it to be absent from what the yank handed back. Each artefact is one case of its own,
//! so a leak is named rather than reported as "the yank was wrong".
//!
//! `yad` is checked against `git apply` rather than against a string this file wrote down. A patch
//! that reads right and does not apply is no patch, and the only way to know is to hand it to the
//! program that would be handed it.

use std::fs;
use std::num::NonZeroUsize;
use std::process::Command;

use ratatui::style::{Color, Modifier, Style};
use vbc_layout::anchor::Wrapping;
use vbc_layout::line::Options as LineOptions;
use vbc_layout::width::Metrics;

use vbc_editor::chat::block::{Block, Kind, Role, RowWindow};
use vbc_editor::chat::fold::{self, Folds, Position as FoldPosition, Tag, View};
use vbc_editor::chat::object::Position;
use vbc_editor::chat::selection::{Mode, Motion, Selection, Source};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::chat::yank::{file, patch, Structure, Yank, CLIPBOARD, UNNAMED, YANK};
use vbc_editor::engine::{Held, Registers, Shape};
use vbc_editor::gutter::{Label, Options as GutterOptions};
use vbc_editor::style::Span;

/// The blocks of the fixture transcript, named by where they sit in it.
const ASKED: usize = 0;
const ANSWERED: usize = 1;
const CALLED: usize = 2;
const RESULT: usize = 3;
const EDIT: usize = 4;

/// The columns the fixture panel is drawn in, narrow enough that the code in it wraps.
const WIDTH: usize = 20;

/// More rows than the fixture is drawn in, so that a render of the panel draws all of it.
const ROWS: usize = 64;

/// The id the fixture's call to a tool is answered under.
const CALL_ID: &str = "toolu_fixture";

/// The question the fixture opens with.
const ASKED_TEXT: &str = "make the panel wrap";

/// The answer, which fences the code the yank is after in the middle of its prose.
const ANSWERED_TEXT: &str = concat!(
    "here is the fix\n",
    "\n",
    "```rust\n",
    "fn main() {\n",
    "    println!(\"a line long enough to wrap\");\n",
    "}\n",
    "```",
);

/// The code that fence holds, which is what `yac` is required to hand back.
const CODE: &str = concat!(
    "fn main() {\n",
    "    println!(\"a line long enough to wrap\");\n",
    "}",
);

/// The call the fixture makes, and the bytes the tool answered with.
const CALLED_TEXT: &str = "cargo test";
const COLOURED: &str = "\u{1b}[1;31merror\u{1b}[0m: nope\n\u{1b}[2Kdone";

/// The text those bytes say, which is what `yat` is required to hand back.
const ANSWERED_BY_THE_TOOL: &str = "error: nope\ndone";

/// The file the fixture's edit was to, and the text either side of it.
const PATH: &str = "src/main.rs";
const BEFORE: &str = "fn main() {}\n";
const AFTER: &str = "fn main() {\n    todo!();\n}\n";

/// The word the fixture styles, which is the syntax decoration a yank must not carry.
const DECORATED: &str = "println!";

/// A grapheme cluster the terminal draws in one column and holds several characters of, and one
/// character of it the cursor is put on.
const CLUSTER: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
const INSIDE_THE_CLUSTER: &str = "\u{1f467}";

/// The cluster the edit under test writes in that one's place.
const REPLACEMENT: &str = "\u{1f9d1}\u{200d}\u{1f680}";

/// Code whose characters and whose columns do not stand in one another's places: the wide ones are
/// drawn in two columns each and the joiners in none at all.
const CLUSTERED_CODE: &str = concat!(
    "let family = \"\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}\";\n",
    "let name = \"\u{65e5}\u{672c}\u{8a9e}\";"
);

/// What the patch under test is written to inside the directory it is applied in.
const PATCH_FILE: &str = "yanked.patch";

/// What `git apply` says when it found the hunk somewhere other than where the patch numbered it,
/// which is a patch that applies by luck rather than one that is written right.
const MOVED: &str = "offset";

/// The escape every ANSI sequence starts with.
const ESCAPE: char = '\u{1b}';

/// What a closed fold's summary row is written under.
const SUMMARY_MARK: &str = "+--";

#[test]
fn a_yank_carries_none_of_the_numbers_the_gutter_drew_beside_the_rows() {
    let transcript = said();
    let folds = folded(&transcript);
    let numbers = numbers(&transcript, &folds);
    let yanked = structural(&transcript, ANSWERED, CODE, Structure::Code);

    assert_ne!(
        Vec::<String>::new(),
        numbers,
        "the fixture drew no numbered row, so there was no gutter to leak"
    );
    let drawn = drawn(&transcript, &folds);
    assert!(
        numbers
            .iter()
            .all(|number| drawn.iter().any(|row| row.starts_with(number.as_str()))),
        "the panel drew a number the gutter did not put in front of a row"
    );

    let leaked: Vec<&str> = yanked
        .lines()
        .filter(|line| numbers.iter().any(|number| line.starts_with(number)))
        .collect();
    assert_eq!(Vec::<&str>::new(), leaked, "the yank carries gutter cells");
    for number in &numbers {
        assert!(
            !yanked.contains(number.as_str()),
            "the yank carries the gutter cell {number:?}: {yanked:?}"
        );
    }
}

#[test]
fn a_yank_of_a_diff_carries_none_of_the_marks_the_diff_wrote_its_lines_under() {
    let transcript = said();
    let block = transcript.block(EDIT).expect("the fixture holds an edit");
    let source = Source::new(block.source(), Metrics::default());
    let added = block
        .source()
        .find("+fn main() {")
        .expect("the fixture's diff writes the lines the edit added");
    let mut selection = Selection::new(Mode::Linewise, source, added);
    selection.extend(source, Motion::Down(2));

    let drawn = drawn(&transcript, &folded(&transcript));
    assert!(
        drawn.iter().any(|row| row.contains("+fn main() {")),
        "the panel drew no marked line, so there was no mark to leak: {drawn:?}"
    );

    let yanked = Yank::selected(block, &selection, Metrics::default());
    let marked: Vec<&str> = yanked
        .text()
        .lines()
        .filter(|line| line.starts_with('+') || line.starts_with('-'))
        .collect();
    assert_eq!(
        Vec::<&str>::new(),
        marked,
        "the yank carries the diff's own marks"
    );
    assert_eq!("fn main() {\n    todo!();\n}", yanked.text());
}

#[test]
fn a_yank_of_a_closed_fold_carries_none_of_the_marks_the_summary_row_is_written_under() {
    let transcript = said();
    let folds = folded(&transcript);
    let fold = folds
        .at(CALLED)
        .expect("a call to a tool heads a fold of its own");
    let drawn = drawn(&transcript, &folds);

    assert!(
        drawn.iter().any(|row| row.starts_with(SUMMARY_MARK)),
        "the panel drew no summary row, so there was no fold mark to leak: {drawn:?}"
    );

    let yanked = Yank::folded(&transcript, fold).expect("the fold covers blocks the fixture holds");
    assert!(
        !yanked.text().contains(SUMMARY_MARK),
        "the yank carries the summary's own mark: {:?}",
        yanked.text()
    );
}

#[test]
fn a_yank_carries_none_of_the_decoration_the_styles_painted_over_it() {
    let transcript = said();
    let block = transcript
        .block(ANSWERED)
        .expect("the fixture holds an answer");
    let rendered = block.render(RowWindow::new(0, ROWS), &wrapping());
    let painted: Vec<&str> = rendered
        .rows()
        .iter()
        .flat_map(|row| row.styled().segments())
        .filter(|segment| decoration() == segment.style())
        .filter_map(|segment| block.slice(segment.source().clone()))
        .collect();

    assert_eq!(
        vec![DECORATED],
        painted,
        "the fixture painted no decoration over the code, so there was none to leak"
    );

    let yanked = structural(&transcript, ANSWERED, CODE, Structure::Code);
    assert_eq!(
        CODE, yanked,
        "the yank is not the bytes the block holds, so something of the styling came with it"
    );
}

#[test]
fn a_yank_carries_none_of_the_ansi_the_tool_wrote_its_output_in() {
    let transcript = said();

    assert!(
        COLOURED.contains(ESCAPE),
        "the fixture's tool output holds no escape, so there was none to leak"
    );

    let yanked = structural(
        &transcript,
        RESULT,
        ANSWERED_BY_THE_TOOL,
        Structure::ToolResult,
    );
    assert!(
        !yanked.contains(ESCAPE),
        "an escape survived into the yank: {yanked:?}"
    );
}

#[test]
fn a_yank_carries_no_newline_the_wrap_put_there() {
    let transcript = said();
    let block = transcript
        .block(ANSWERED)
        .expect("the fixture holds an answer");
    let rows = block
        .render(RowWindow::new(0, ROWS), &wrapping())
        .rows()
        .len();
    let lines = 1 + block.source().matches('\n').count();

    assert!(
        lines < rows,
        "the fixture was drawn in {rows} rows for {lines} lines, so nothing wrapped"
    );

    let yanked = structural(&transcript, ANSWERED, CODE, Structure::Code);
    assert_eq!(
        2,
        yanked.matches('\n').count(),
        "the yank holds a newline no logical line of the code does: {yanked:?}"
    );
}

#[test]
fn a_yank_carries_none_of_the_columns_the_clusters_were_drawn_across() {
    let transcript = clustered();
    let block = transcript
        .block(0)
        .expect("the fixture holds an answer holding clusters");
    let first = CLUSTERED_CODE
        .lines()
        .next()
        .expect("the fixture's code holds a line");

    assert_ne!(
        first.chars().count(),
        Metrics::default().text_width(first, 0),
        "the fixture's code is drawn in one column per character, so no cluster is at stake"
    );

    let at = block
        .source()
        .find(INSIDE_THE_CLUSTER)
        .expect("the fixture's code holds the cluster the cursor is put inside");
    let yanked = Yank::structural(&transcript, Position::new(0, at), Structure::Code)
        .expect("the cursor is in the fenced code");

    assert_eq!(CLUSTERED_CODE, yanked.text());
}

#[test]
fn a_patch_yanked_from_a_diff_applies_cleanly() -> anyhow::Result<()> {
    let transcript = said();
    let yanked = Yank::structural(&transcript, Position::new(EDIT, 0), Structure::Diff)
        .expect("the fixture's edit is a diff a patch can be written from");

    assert_eq!(
        Some(AFTER.to_owned()),
        applied(PATH, BEFORE, yanked.text())?
    );

    Ok(())
}

#[test]
fn a_patch_over_changes_far_apart_applies_cleanly_in_every_hunk() -> anyhow::Result<()> {
    let lines: Vec<String> = (1..=20).map(|number| format!("line {number}")).collect();
    let old = format!("{}\n", lines.join("\n"));
    let mut rewritten = lines;
    rewritten[0] = "first".to_owned();
    rewritten[19] = "last".to_owned();
    let new = format!("{}\n", rewritten.join("\n"));
    let block = Block::diff(PATH.to_owned(), &old, &new);
    let written = patch(&block).expect("the diff is a patch");

    assert_eq!(
        2,
        written
            .lines()
            .filter(|line| line.starts_with("@@"))
            .count(),
        "the fixture was written in one hunk, so no second hunk was applied: {written:?}"
    );
    assert_eq!(Some(new), applied(PATH, &old, &written)?);

    Ok(())
}

#[test]
fn a_patch_over_lines_no_column_lines_up_with_applies_cleanly() -> anyhow::Result<()> {
    let old = format!("\u{58f1}\n{CLUSTER}\n\u{53c2}\n");
    let new = format!("\u{58f1}\n{REPLACEMENT}\n\u{53c2}\n");
    let block = Block::diff(PATH.to_owned(), &old, &new);
    let written = patch(&block).expect("the diff is a patch");

    assert_eq!(Some(new), applied(PATH, &old, &written)?);

    Ok(())
}

#[test]
fn a_patch_over_a_text_that_does_not_end_in_a_line_separator_is_refused() -> anyhow::Result<()> {
    let old = "alpha\nbeta";
    let block = Block::diff("notes.txt".to_owned(), old, "alpha\ngamma");
    let written = patch(&block).expect("the diff is a patch");

    assert_eq!(None, applied("notes.txt", old, &written)?);

    Ok(())
}

#[test]
fn a_yank_of_a_closed_fold_takes_what_it_covers_and_never_the_row_it_is_drawn_in() {
    let transcript = said();
    let folds = folded(&transcript);
    let fold = folds
        .at(CALLED)
        .expect("a call to a tool heads a fold of its own");
    let view = View::of(&folds, &transcript);
    let summaries: Vec<&str> = view
        .render(FoldPosition::top(0), ROWS, &wrapping())
        .into_iter()
        .filter_map(|row| match row {
            fold::Row::Summary(summary) => Some(summary.text()),
            fold::Row::Body { .. } => None,
        })
        .collect();

    assert_eq!(
        1,
        summaries.len(),
        "the fixture drew {summaries:?} rather than one closed fold"
    );
    assert_eq!(vec![CALLED, RESULT], fold.covered());

    let yanked = Yank::folded(&transcript, fold).expect("the fold covers blocks the fixture holds");
    assert_eq!(
        format!("{CALLED_TEXT}\n{ANSWERED_BY_THE_TOOL}"),
        yanked.text()
    );
    assert!(
        !summaries.contains(&yanked.text()),
        "the yank handed back the summary row"
    );
}

#[test]
fn a_plain_yank_reaches_the_clipboards_register_and_a_plain_put_reads_the_unnamed_one() {
    let transcript = said();
    let block = transcript
        .block(ASKED)
        .expect("the fixture holds a question");
    let source = Source::new(block.source(), Metrics::default());
    let selection = Selection::new(Mode::Linewise, source, 0);
    let registers = Registers::new();
    file(
        &registers,
        &Yank::selected(block, &selection, Metrics::default()),
    );

    let yanked = Held {
        text: format!("{ASKED_TEXT}\n"),
        shape: Shape::Linewise,
    };
    assert_eq!(Some(yanked.clone()), registers.get(UNNAMED));
    assert_eq!(Some(yanked.clone()), registers.get(YANK));
    assert_eq!(
        Some(yanked.clone()),
        registers.get(CLIPBOARD),
        "a plain yank did not reach the clipboard's register"
    );

    let copied = Held {
        text: "what the desktop last copied".to_owned(),
        shape: Shape::Charwise,
    };
    registers.fill(CLIPBOARD, &copied);

    assert_eq!(Some(copied), registers.get(CLIPBOARD));
    assert_eq!(
        Some(yanked),
        registers.get(UNNAMED),
        "what the desktop copied reached the register a plain put reads"
    );
}

#[test]
fn a_yank_across_blocks_of_different_kinds_writes_them_the_way_the_rows_separate_them() {
    let transcript = said();
    let yanked = Yank::spanning(&transcript, ASKED, RESULT).expect("the fixture holds the run");

    assert_eq!(
        format!("{ASKED_TEXT}\n{ANSWERED_TEXT}\n{CALLED_TEXT}\n{ANSWERED_BY_THE_TOOL}"),
        yanked.text()
    );

    let mut folds = folded(&transcript);
    folds.apply(fold::Command::OpenAll, 0);
    let drawn: Vec<String> = View::of(&folds, &transcript)
        .render(FoldPosition::top(0), ROWS, &wrapping())
        .into_iter()
        .filter_map(|row| match row {
            fold::Row::Body { block, row } if block <= RESULT => {
                Some(row.styled().row().text().to_owned())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        drawn.concat(),
        yanked.text().replace('\n', ""),
        "the yank is not the text the panel drew of the same blocks"
    );
}

/// # Returns
///
/// The text `structure` takes at the first byte of the block at `block`, having required it to be
/// `expected`.
///
/// # Panics
///
/// Panics if the structure takes nothing there.
fn structural(
    transcript: &Transcript,
    block: usize,
    expected: &str,
    structure: Structure,
) -> String {
    let at = transcript
        .block(block)
        .and_then(|held| {
            held.source()
                .find(expected.lines().next().unwrap_or_default())
        })
        .unwrap_or_default();
    let yanked = Yank::structural(transcript, Position::new(block, at), structure)
        .expect("the fixture holds the structure the case is after");

    assert_eq!(expected, yanked.text());

    yanked.text().to_owned()
}

/// Applies `written` to a copy of `path` holding `old`, by handing it to `git apply` in a directory
/// of its own.
///
/// `git apply` searches for the context a hunk names rather than trusting the line it was numbered
/// at, so a patch whose numbers are wrong still applies. It says so under `--verbose`, and a patch
/// it had to move is required here to be a failure rather than a success.
///
/// # Returns
///
/// What the file holds once the patch has been applied, or `None` where `git apply` refused it.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`tempfile::tempdir`]'s return values on failure.
/// * Forwards [`std::fs::create_dir_all`]'s return values on failure.
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`std::process::Command::output`]'s return values on failure.
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
///
/// # Panics
///
/// Panics if `git apply` had to move a hunk away from the line the patch numbered it at.
fn applied(path: &str, old: &str, written: &str) -> anyhow::Result<Option<String>> {
    let directory = tempfile::tempdir()?;
    let file = directory.path().join(path);
    fs::create_dir_all(file.parent().expect("a file in a directory has a parent"))?;
    fs::write(&file, old)?;
    fs::write(directory.path().join(PATCH_FILE), written)?;

    let ran = Command::new("git")
        .arg("apply")
        .arg("--verbose")
        .arg(PATCH_FILE)
        .current_dir(directory.path())
        .output()?;
    if !ran.status.success() {
        return Ok(None);
    }

    let reported = format!(
        "{}{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(
        !reported.contains(MOVED),
        "`git apply` moved a hunk to make the patch fit, so its line numbers are wrong: \
         {reported}"
    );

    Ok(Some(fs::read_to_string(&file)?))
}

/// # Returns
///
/// What the panel draws of `transcript` under `folds`: the gutter cells and then the row for every
/// row of a block, and its own text for the row a closed fold is collapsed to.
fn drawn(transcript: &Transcript, folds: &Folds) -> Vec<String> {
    let gutter = GutterOptions::new().with_number(true);
    let width = gutter.width(lines_of(transcript));

    View::of(folds, transcript)
        .render(FoldPosition::top(0), ROWS, &wrapping())
        .into_iter()
        .map(|row| match row {
            fold::Row::Summary(summary) => summary.text().to_owned(),
            fold::Row::Body { row, .. } => {
                let cells = gutter
                    .label(row.styled().row(), 0)
                    .map_or_else(String::new, |label| label.cells(width));

                format!("{cells}{}", row.styled().row().text())
            }
        })
        .collect()
}

/// # Returns
///
/// The gutter cells the panel drew a line number in, in the order they were drawn.
fn numbers(transcript: &Transcript, folds: &Folds) -> Vec<String> {
    let gutter = GutterOptions::new().with_number(true);
    let width = gutter.width(lines_of(transcript));

    View::of(folds, transcript)
        .render(FoldPosition::top(0), ROWS, &wrapping())
        .into_iter()
        .filter_map(|row| match row {
            fold::Row::Summary(_) => None,
            fold::Row::Body { row, .. } => gutter.label(row.styled().row(), 0),
        })
        .filter(|label| matches!(label, Label::Absolute(_)))
        .map(|label| label.cells(width))
        .collect()
}

/// # Returns
///
/// The folds of `transcript`, every one of them closed, under the tags that nest what the tool
/// answered beneath the call that asked it.
fn folded(transcript: &Transcript) -> Folds {
    let mut tags = vec![Tag::untagged(); transcript.len()];
    tags[CALLED] = Tag::new(Some(CALL_ID.to_owned()), None);
    tags[RESULT] = Tag::new(None, Some(CALL_ID.to_owned()));

    Folds::of(transcript, &tags)
}

/// # Returns
///
/// The number of logical lines the transcript holds between all of its blocks.
fn lines_of(transcript: &Transcript) -> usize {
    transcript
        .blocks()
        .iter()
        .map(|block| 1 + block.source().matches('\n').count())
        .sum()
}

/// # Returns
///
/// The wrapping the fixture panel is drawn under.
///
/// # Panics
///
/// Panics if the fixture is drawn in no columns, which it is not.
fn wrapping() -> Wrapping {
    Wrapping::new(
        NonZeroUsize::new(WIDTH).expect("the fixture is drawn in at least one column"),
        Metrics::default(),
        LineOptions::new(),
    )
}

/// # Returns
///
/// The style the fixture's syntax decoration is painted in.
fn decoration() -> Style {
    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
}

/// # Returns
///
/// An answer fencing [`CLUSTERED_CODE`], which is code the panel draws in a number of columns no
/// count of its characters gives.
fn clustered() -> Transcript {
    [Block::new(
        Kind::Message(Role::Assistant),
        format!("here it is\n\n```rust\n{CLUSTERED_CODE}\n```"),
    )]
    .into_iter()
    .collect()
}

/// # Returns
///
/// A short exchange holding one block of every kind a structural yank addresses, drawn over
/// chrome of every kind a yank must not pick up.
///
/// # Panics
///
/// Panics if the answer does not hold the word the fixture decorates, which it does.
fn said() -> Transcript {
    let decorated = ANSWERED_TEXT
        .find(DECORATED)
        .expect("the answer holds the word the fixture decorates");

    [
        Block::new(Kind::Message(Role::User), ASKED_TEXT.to_owned()),
        Block::with_spans(
            Kind::Message(Role::Assistant),
            ANSWERED_TEXT.to_owned(),
            vec![Span::new(
                decorated..decorated + DECORATED.len(),
                decoration(),
            )],
        ),
        Block::new(
            Kind::ToolCall {
                name: "Bash".to_owned(),
            },
            CALLED_TEXT.to_owned(),
        ),
        Block::from_ansi(Kind::ToolResult, COLOURED),
        Block::diff(PATH.to_owned(), BEFORE, AFTER),
    ]
    .into_iter()
    .collect()
}
