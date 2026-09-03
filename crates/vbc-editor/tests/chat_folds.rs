//! What a fold has to be worth before a reader trusts it with what it hid.
//!
//! A fold is only useful if it is a property of the conversation rather than of the frame it was
//! drawn in. So a fold has to survive being drawn again at another width; a closed one has to
//! behave as the single row it is drawn as, so that a reader walking down the panel steps past
//! everything it covers in one keystroke rather than through it; folds nested inside one another
//! have to open and close on their own and be reachable at every depth; a tool still writing its
//! output has to leave the fold over it closed and merely say that there is more of it now; and
//! none of it may touch a byte of what was said, because the transcript is the truth a selection
//! and a yank are taken from and a fold is a way of not looking at it.
//!
//! The transcript these are checked over is the shape the nesting is about: a call to a subagent,
//! what the subagent said, a call the subagent itself made, what that answered, and what Claude
//! thought about it, each tagged with the call it arrived beneath.

use std::num::NonZeroUsize;

use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::fold::{Command, Entry, Folds, Position, Row, Tag, View};
use vbc_editor::chat::transcript::Transcript;
use vbc_layout::anchor::Wrapping;
use vbc_layout::line::Options;
use vbc_layout::width::Metrics;

/// The width a panel is read at, wide enough that no fixture wraps.
const COLUMNS: usize = 80;

/// The width the same panel is read at once it has been made narrow, at which every fixture wraps.
const NARROW: usize = 12;

/// The rows a screenful of the panel draws, more than the whole fixture takes.
const SCREEN: usize = 64;

/// The id the call to the subagent is answered under, and the id of the call the subagent made
/// inside it.
const SUBAGENT: &str = "toolu_subagent";
const NESTED: &str = "toolu_nested";

/// The blocks of the fixture, by the index they were said at.
const ASKED: usize = 0;
const SUBAGENT_CALL: usize = 1;
const SUBAGENT_SAID: usize = 2;
const NESTED_CALL: usize = 3;
const NESTED_RESULT: usize = 4;
const THOUGHT: usize = 5;
const ANSWERED: usize = 6;

/// What the tool answered while it was still running, and what it had answered by the time it was
/// done, which is the same block written further into.
const RUNNING: &str = "running 4 tests\ntest anchor ... ok\ntest wrapping ... ok";
const FINISHED: &str = "running 4 tests\ntest anchor ... ok\ntest wrapping ... ok\ntest width ... \
                        ok\ntest folds ... ok\n\ntest result: ok. 4 passed\n\n     Finished in \
                        0.04s";

#[test]
fn fold_state_survives_a_resize_and_a_re_render() {
    let (transcript, tags) = conversation(RUNNING);
    let mut folds = Folds::of(&transcript, &tags);
    folds.apply(Command::Open, SUBAGENT_CALL);
    folds.apply(Command::Open, NESTED_CALL);

    let wide = View::of(&folds, &transcript);
    assert_eq!(
        vec![
            "body 0",
            "body 1",
            "body 2",
            "body 3",
            "summary 4",
            "summary 5",
            "body 6",
        ],
        described(&wide)
    );
    let drawn = wide.render(Position::new(0, 0), SCREEN, &wrapping(COLUMNS));

    let narrow = View::of(&folds, &transcript);
    assert_eq!(
        wide.entries(),
        narrow.entries(),
        "the folds were drawn differently once the panel was resized"
    );
    let redrawn = narrow.render(Position::new(0, 0), SCREEN, &wrapping(NARROW));
    assert_ne!(
        texts(&drawn),
        texts(&redrawn),
        "the fixture drew the same rows at both widths, so nothing was re-wrapped"
    );
    assert_eq!(
        summaries(&drawn),
        summaries(&redrawn),
        "a fold said something else about itself once the panel was resized"
    );

    folds.rebuild(&transcript, &tags);
    let again = View::of(&folds, &transcript);
    assert_eq!(
        wide.entries(),
        again.entries(),
        "the folds were forgotten when the transcript was read again"
    );
    assert!(folds.is_open(SUBAGENT_CALL) && folds.is_open(NESTED_CALL));
    assert!(!folds.is_open(NESTED_RESULT) && !folds.is_open(THOUGHT));
}

#[test]
fn a_closed_fold_is_one_row_and_stepping_down_steps_past_the_whole_of_it() {
    let (transcript, tags) = conversation(RUNNING);
    let mut folds = Folds::of(&transcript, &tags);
    folds.apply(Command::CloseAll, ASKED);
    let wrapping = wrapping(NARROW);

    let closed = View::of(&folds, &transcript);
    assert_eq!(vec!["body 0", "summary 1", "body 6"], described(&closed));
    let covered = folds
        .at(SUBAGENT_CALL)
        .expect("the call to the subagent heads a fold")
        .covered()
        .to_vec();
    assert_eq!(
        vec![
            SUBAGENT_CALL,
            SUBAGENT_SAID,
            NESTED_CALL,
            NESTED_RESULT,
            THOUGHT
        ],
        covered
    );

    let hidden: usize = (1..6)
        .map(|entry| open_rows(&transcript, &tags, entry))
        .sum();
    assert!(
        1 < hidden,
        "the fixture folded away one row, so a fold that drew them all would look the same"
    );
    assert_eq!(1, closed.rows(1, &wrapping));

    let below = closed.rows(0, &wrapping) - 1;
    let onto = closed
        .down(Position::new(0, below), &wrapping)
        .expect("a row follows the block the fold begins after");
    assert_eq!(Position::new(1, 0), onto);
    let past = closed
        .down(onto, &wrapping)
        .expect("a row follows the closed fold");
    assert_eq!(Position::new(2, 0), past);
    assert_eq!(
        Some(&Entry::Body(ANSWERED)),
        closed.entries().get(past.entry()),
        "stepping past the closed fold did not land on the block after it"
    );
    assert_eq!(None, closed.down(past, &wrapping));

    folds.apply(Command::OpenAll, ASKED);
    let opened = View::of(&folds, &transcript);
    let onto = opened
        .down(Position::new(0, below), &wrapping)
        .expect("a row follows the block the fold begins after");
    let inside = opened
        .down(onto, &wrapping)
        .expect("a row follows the first row of an open fold");
    assert_eq!(
        Some(&Entry::Body(SUBAGENT_CALL)),
        opened.entries().get(onto.entry()),
        "the fixture drew the open fold as something other than its own blocks"
    );
    assert!(
        inside.entry() <= SUBAGENT_SAID,
        "two steps into an open fold left it: {inside:?}"
    );
}

#[test]
fn nested_folds_open_and_close_on_their_own_and_every_depth_is_reached() {
    let (transcript, tags) = conversation(RUNNING);
    let mut folds = Folds::of(&transcript, &tags);

    let heads: Vec<(usize, usize)> = folds
        .folds()
        .iter()
        .map(|fold| (fold.head(), fold.depth()))
        .collect();
    assert_eq!(
        vec![
            (SUBAGENT_CALL, 0),
            (NESTED_CALL, 1),
            (NESTED_RESULT, 2),
            (THOUGHT, 1)
        ],
        heads,
        "the fixture does not nest a fold three deep, so nothing here reaches a third depth"
    );

    folds.apply(Command::OpenAll, ASKED);
    assert_eq!(
        vec!["body 0", "body 1", "body 2", "body 3", "body 4", "body 5", "body 6",],
        described(&View::of(&folds, &transcript)),
        "`zR` left a fold closed at some depth"
    );

    folds.apply(Command::Close, NESTED_RESULT);
    assert_eq!(
        vec![
            "body 0",
            "body 1",
            "body 2",
            "body 3",
            "summary 4",
            "body 5",
            "body 6",
        ],
        described(&View::of(&folds, &transcript)),
        "closing the innermost fold closed something else as well"
    );
    assert!(folds.is_open(NESTED_CALL) && folds.is_open(THOUGHT));

    folds.apply(Command::Close, NESTED_RESULT);
    assert_eq!(
        vec![
            "body 0",
            "body 1",
            "body 2",
            "summary 3",
            "body 5",
            "body 6"
        ],
        described(&View::of(&folds, &transcript)),
        "closing a fold from inside a closed one did not close the fold around it"
    );
    assert!(folds.is_open(SUBAGENT_CALL) && folds.is_open(THOUGHT));

    folds.apply(Command::Toggle, NESTED_CALL);
    folds.apply(Command::Toggle, NESTED_RESULT);
    assert_eq!(
        vec!["body 0", "body 1", "body 2", "body 3", "body 4", "body 5", "body 6",],
        described(&View::of(&folds, &transcript)),
        "two toggles did not open the two folds they were typed over"
    );

    folds.apply(Command::CloseAll, ANSWERED);
    assert_eq!(
        vec!["body 0", "summary 1", "body 6"],
        described(&View::of(&folds, &transcript)),
        "`zM` left a fold open at some depth"
    );
    assert!(folds.folds().iter().all(|fold| !folds.is_open(fold.head())));
}

#[test]
fn a_fold_whose_content_changed_while_closed_stays_closed_and_says_what_it_now_holds() {
    let (running, tags) = conversation(RUNNING);
    let mut folds = Folds::of(&running, &tags);
    folds.apply(Command::Open, SUBAGENT_CALL);
    folds.apply(Command::Open, NESTED_CALL);

    let before = View::of(&folds, &running);
    assert_eq!(
        vec!["+---- 3 lines: result running 4 tests"],
        summaries(&before.render(Position::new(0, 0), SCREEN, &wrapping(COLUMNS)))
            .into_iter()
            .filter(|text| text.contains("result"))
            .collect::<Vec<String>>()
    );

    let (finished, _) = conversation(FINISHED);
    folds.rebuild(&finished, &tags);
    let after = View::of(&folds, &finished);

    assert_eq!(
        before.entries().len(),
        after.entries().len(),
        "the fold over the tool result opened when the tool wrote another line"
    );
    assert!(!folds.is_open(NESTED_RESULT));
    assert_eq!(
        Some(&Entry::Body(NESTED_CALL)),
        after.entries().get(3),
        "a fold that was open closed when the tool wrote another line"
    );
    let Some(Entry::Summary(summary)) = after.entries().get(4) else {
        panic!(
            "the tool result is still folded away: {:?}",
            after.entries()
        );
    };
    assert_eq!("+---- 9 lines: result running 4 tests", summary.text());

    folds.apply(Command::CloseAll, ASKED);
    let closed = View::of(&folds, &finished);
    assert_eq!(
        vec!["+-- 13 lines: Task review the anchor"],
        summaries(&closed.render(Position::new(0, 0), SCREEN, &wrapping(COLUMNS))),
        "the fold around the tool result did not count what the tool went on to write"
    );
}

#[test]
fn folding_never_alters_the_source_it_folds() {
    let (transcript, tags) = conversation(RUNNING);
    let said: Vec<Vec<u8>> = transcript
        .blocks()
        .iter()
        .map(|block| block.source().as_bytes().to_vec())
        .collect();

    let mut folds = Folds::of(&transcript, &tags);
    let wrapping = wrapping(NARROW);
    for command in [
        Command::CloseAll,
        Command::Toggle,
        Command::Open,
        Command::Close,
        Command::OpenAll,
    ] {
        for at in ASKED..=ANSWERED {
            folds.apply(command, at);
            let view = View::of(&folds, &transcript);
            let drawn = view.render(Position::new(0, 0), SCREEN, &wrapping);
            for row in &drawn {
                let Row::Body { block, row } = row else {
                    continue;
                };
                let source = transcript
                    .block(*block)
                    .expect("a drawn row names a block of the transcript");
                assert_eq!(
                    Some(row.styled().row().text()),
                    source.slice(row.source()),
                    "a drawn row shows something the block it came from does not hold"
                );
            }
        }
    }
    folds.rebuild(&transcript, &tags);

    assert_eq!(
        said,
        transcript
            .blocks()
            .iter()
            .map(|block| block.source().as_bytes().to_vec())
            .collect::<Vec<Vec<u8>>>(),
        "folding wrote to the transcript it folded"
    );

    folds.apply(Command::OpenAll, ASKED);
    let view = View::of(&folds, &transcript);
    let drawn = view.render(Position::new(0, 0), SCREEN, &wrapping);
    for (index, source) in transcript.blocks().iter().enumerate() {
        let shown: String = drawn
            .iter()
            .filter_map(|row| match row {
                Row::Body { block, row } if index == *block => Some(row.styled().row().text()),
                _ => None,
            })
            .collect();
        assert_eq!(
            source.source().replace('\n', ""),
            shown,
            "block {index} was not drawn from the whole of its own source"
        );
    }
}

/// # Returns
///
/// The fixture: a question, a call to a subagent, what the subagent said, the call the subagent
/// made, what `result` that call answered with, what Claude thought about it and the answer, each
/// tagged with the call it arrived beneath; together with those tags.
fn conversation(result: &str) -> (Transcript, Vec<Tag>) {
    let blocks = vec![
        Block::new(Kind::Message(Role::User), "make the anchor hold".to_owned()),
        Block::new(
            Kind::ToolCall {
                name: "Task".to_owned(),
            },
            "review the anchor".to_owned(),
        ),
        Block::new(
            Kind::Message(Role::Assistant),
            "reading the mapping now".to_owned(),
        ),
        Block::new(
            Kind::ToolCall {
                name: "Bash".to_owned(),
            },
            "cargo test -p vbc-layout".to_owned(),
        ),
        Block::from_ansi(Kind::ToolResult, result),
        Block::new(Kind::Thinking, "the anchor holds".to_owned()),
        Block::new(Kind::Message(Role::Assistant), "it holds".to_owned()),
    ];
    let tags = vec![
        Tag::untagged(),
        Tag::new(Some(SUBAGENT.to_owned()), None),
        Tag::new(None, Some(SUBAGENT.to_owned())),
        Tag::new(Some(NESTED.to_owned()), Some(SUBAGENT.to_owned())),
        Tag::new(None, Some(NESTED.to_owned())),
        Tag::new(None, Some(SUBAGENT.to_owned())),
        Tag::untagged(),
    ];

    (blocks.into_iter().collect(), tags)
}

/// # Returns
///
/// A wrapping drawing rows `width` columns wide under vim's own defaults.
fn wrapping(width: usize) -> Wrapping {
    Wrapping::new(
        NonZeroUsize::new(width).expect("a panel is drawn in at least one column"),
        Metrics::default(),
        Options::new(),
    )
}

/// # Returns
///
/// What each entry of `view` is and which block it draws, top to bottom.
fn described(view: &View<'_>) -> Vec<String> {
    view.entries()
        .iter()
        .map(|entry| match entry {
            Entry::Summary(summary) => format!("summary {}", summary.head()),
            Entry::Body(block) => format!("body {block}"),
        })
        .collect()
}

/// # Returns
///
/// The text of every row of `drawn`, top to bottom.
fn texts(drawn: &[Row<'_>]) -> Vec<String> {
    drawn
        .iter()
        .map(|row| match row {
            Row::Summary(summary) => summary.text().to_owned(),
            Row::Body { row, .. } => row.styled().row().text().to_owned(),
        })
        .collect()
}

/// # Returns
///
/// The text of every summary row of `drawn`, top to bottom.
fn summaries(drawn: &[Row<'_>]) -> Vec<String> {
    drawn
        .iter()
        .filter_map(|row| match row {
            Row::Summary(summary) => Some(summary.text().to_owned()),
            Row::Body { .. } => None,
        })
        .collect()
}

/// # Returns
///
/// The rows the entry `entry` of the fixture is drawn in with every fold open, which is how many
/// rows a closed fold over it is standing in for.
fn open_rows(transcript: &Transcript, tags: &[Tag], entry: usize) -> usize {
    let mut folds = Folds::of(transcript, tags);
    folds.apply(Command::OpenAll, 0);

    View::of(&folds, transcript).rows(entry, &wrapping(NARROW))
}
