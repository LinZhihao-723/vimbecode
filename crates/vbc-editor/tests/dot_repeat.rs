//! `.`, held to the vim it repeats the way of.
//!
//! Every case here is typed at the application rather than at the engine under it, because the
//! claim `.` makes is about what a reader typing at the program gets: the keys arrive as the
//! terminal reader delivers them, the window the display motions are measured in is the one the
//! program is drawing, and what is read back is the text, the cursor and the mode the program
//! would show. What a real vim leaves behind the very same keys is what each of them is compared
//! against, so the rules being kept are vim's rather than a reading of vim someone wrote down
//! here.
//!
//! The rules a repeat has to keep are about which command it repeats as much as about what that
//! command does. A motion is not a change and neither is a yank, so `.` after one repeats the
//! change in front of it; an undo is not a change either, so `u` then `.` makes the change again
//! rather than undoing twice. An inserting mode is a change however little is typed in it, which
//! is why `i<Esc>` is a command that becomes the repeated one. Each of those is a case below, and
//! each is one vim answers differently from the reading it is easy to fall into.
//!
//! The counts are the other half. vim's rule is that a count typed in front of `.` replaces the
//! count the command was typed with rather than multiplying it, and that a command typed with two
//! counts -- `2d3w`, which travels six words -- is replaced by the one count as a whole, so `4.`
//! after it travels four words and not twelve. Both are here, along with the repeats that carry
//! the original count because no new one was typed.
//!
//! The display motion is the case this seam exists for. `dgj` is measured in the rows of the
//! window it is typed at, so a repeat that replayed where the first one landed would delete a
//! range measured at a cursor that has since moved. It is typed at a narrow window, repeated from
//! a line that wraps differently, and required to remove a different amount of text than it
//! removed the first time -- which is what a replay of the recorded landing could not do -- as
//! well as to leave what vim leaves.
//!
//! The window is the one the case declares in both engines: the vim the case is compared against
//! is laid out in the columns the application leaves its text after the gutter, so the two wrap
//! their lines in the same place rather than by coincidence.
//!
//! What a repeat does not have to survive is a macro, because this editor records none: `q` ends
//! the program rather than opening a recording and `@` is bound to nothing. That is asserted here
//! rather than assumed, so that a macro arriving later arrives with the question about `.` inside
//! one already asked.
//!
//! A cursor is compared as vim reports it, which is a byte offset within its line; the application
//! reports a grapheme offset, and every text here is one whose characters are one byte and one
//! cell wide so that the two counts are the same number. Where a grapheme is drawn is the layout
//! engine's and is held to vim by its own oracles.

use std::collections::BTreeSet;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use modalkit::env::vim::VimMode;
use ratatui::layout::Rect;
use vbc_editor::app::{App, Outcome};
use vbc_layout::buffer::Buffer;
use vbc_oracle::corpus::{Case as CorpusCase, Options as CaseOptions};
use vbc_oracle::state::{EditorState, Mode};
use vbc_oracle::vim::VimDriver;

/// One cross-check: the keys typed, and the name the case is reported under.
struct Case {
    id: &'static str,
    keys: &'static str,
}

/// What a replay left the application holding, in the terms a real vim is compared against.
#[derive(Debug, Eq, PartialEq)]
struct Landed {
    text: String,
    line: u64,
    column: u64,
    mode: Mode,
}

/// The prose the repeats that ask nothing about the window are typed at, whose first line holds
/// more words than any count below reaches and whose later lines are short.
const PROSE: &str = "aaa bbb ccc ddd eee fff ggg hhh\nsecond line here\nthird line\nfourth\nfifth";

/// The prose the display motions are typed at, whose first and third lines wrap in the window
/// below and whose second does not.
const WRAPPED: &str = "alpha beta gamma delta\none two\nepsilon zeta eta theta\ntail";

/// The rows every terminal here holds, which is more than the texts have lines.
const ROWS: u16 = 8;

/// The terminal the repeats that ask nothing about the window are typed at, wide enough that
/// nothing wraps in it.
const WIDE: u16 = 60;

/// The terminal the display motions are typed at, narrow enough that [`WRAPPED`] wraps in it.
const NARROW: u16 = 16;

/// The repeats whose answers the window has no say in: what becomes the repeated change, what a
/// count does to it, and what a command that is not a change leaves it as.
const CASES: [Case; 14] = [
    Case {
        id: "a word delete",
        keys: "dw.",
    },
    Case {
        id: "a line delete",
        keys: "dj.",
    },
    Case {
        id: "a screen line delete in a window nothing wraps in",
        keys: "dgj.",
    },
    Case {
        id: "a change carrying the text it inserted",
        keys: "ciwfoo<Esc>w.",
    },
    Case {
        id: "an operator carrying the count it was typed with",
        keys: "3dd.",
    },
    Case {
        id: "an operator carrying the count behind it",
        keys: "d3w.",
    },
    Case {
        id: "a new count standing in for the one that was typed",
        keys: "dw3.",
    },
    Case {
        id: "a new count standing in for both counts of the command",
        keys: "2d3w4.",
    },
    Case {
        id: "a repeat carrying the count of the repeat in front of it",
        keys: "dw3..",
    },
    Case {
        id: "a motion is not the change a repeat repeats",
        keys: "dwwgj.",
    },
    Case {
        id: "a yank is not the change a repeat repeats",
        keys: "dwyy.",
    },
    Case {
        id: "a visual selection that changed nothing is not the change a repeat repeats",
        keys: "dwv<Esc>.",
    },
    Case {
        id: "an undo leaves the change in front of it the repeated one",
        keys: "dwu0.",
    },
    Case {
        id: "an insert placed at the end of a line",
        keys: "A!<Esc>j.",
    },
];

/// The repeats of the commands nothing above repeats: the ones that put text back, the ones that
/// shift it, and the ones made from a visual selection.
const EDITS: [Case; 7] = [
    Case {
        id: "a linewise selection",
        keys: "Vjd.",
    },
    Case {
        id: "a charwise selection made within one line",
        keys: "vlldw.",
    },
    Case {
        id: "a put",
        keys: "yyjp.",
    },
    Case {
        id: "a shift",
        keys: ">>j.",
    },
    Case {
        id: "a replaced character",
        keys: "rzw.",
    },
    Case {
        id: "an opened line",
        keys: "o- <Esc>.",
    },
    Case {
        id: "a joined line",
        keys: "J.",
    },
];

/// The repeats vim answers exactly as it answers the keys without them, which is what keeps them
/// out of the check that every case here is one a repeat that did nothing would fail.
///
/// A change that rewrites the word the cursor is left standing inside rewrites it to the text it
/// already holds, and a repeat of an insert that inserted nothing inserts nothing again. Being
/// answered the same way with the repeat and without it is what these two are here for rather than
/// a weakness in them: an editor that took the empty insert for no change at all would repeat the
/// delete in front of it, and that is a text neither of them leaves.
const IDEMPOTENT: [Case; 2] = [
    Case {
        id: "a change repeated over the word it wrote",
        keys: "ciwfoo<Esc>.",
    },
    Case {
        id: "an inserting mode is a change however little is typed in it",
        keys: "dwi<Esc>.",
    },
];

#[test]
fn a_repeat_leaves_the_application_where_vim_leaves_it() -> Result<()> {
    let vim = VimDriver::new()?;

    for case in CASES.iter().chain(EDITS.iter()).chain(IDEMPOTENT.iter()) {
        let mut app = holding(PROSE);
        press(&mut app, WIDE, case.keys);

        assert_eq!(
            vim_landed(&vim, PROSE, case.keys, WIDE)?,
            landed(&app),
            "`{}` left the program somewhere other than where vim left it",
            case.id
        );
    }

    Ok(())
}

#[test]
fn every_repeat_is_one_the_keys_in_front_of_it_could_not_have_left_behind() -> Result<()> {
    let vim = VimDriver::new()?;

    for case in CASES.iter().chain(EDITS.iter()) {
        let repeated = vim_landed(&vim, PROSE, case.keys, WIDE)?;
        let bare = case
            .keys
            .strip_suffix('.')
            .expect("every case here ends in the repeat it is about");

        assert_ne!(
            vim_landed(&vim, PROSE, bare, WIDE)?,
            repeated,
            "vim answers `{}` where it answers the same keys without the repeat, so the case \
             cannot tell a repeat that ran from one that did nothing at all",
            case.id
        );
    }

    Ok(())
}

#[test]
fn a_repeat_of_a_display_motion_is_measured_where_it_is_typed() -> Result<()> {
    let vim = VimDriver::new()?;
    let mut app = holding(WRAPPED);

    press(&mut app, NARROW, "dgj");
    let once = landed(&app);
    press(&mut app, NARROW, "j.");
    let again = landed(&app);

    assert_eq!(
        vim_landed(&vim, WRAPPED, "dgj", NARROW)?,
        once,
        "`dgj` left the program somewhere other than where vim left it"
    );
    assert_eq!(
        vim_landed(&vim, WRAPPED, "dgjj.", NARROW)?,
        again,
        "the repeat of `dgj` left the program somewhere other than where vim left it"
    );
    assert_ne!(
        format!("{WRAPPED}\n").len() - once.text.len(),
        once.text.len() - again.text.len(),
        "the repeat took away exactly what the first `dgj` took away, which is what replaying \
         where the motion landed rather than walking the rows below the cursor would do"
    );

    Ok(())
}

#[test]
fn a_repeat_of_a_display_motion_walks_the_rows_of_the_terminal_it_is_typed_at() -> Result<()> {
    let vim = VimDriver::new()?;
    let mut narrow = holding(WRAPPED);
    let mut wide = holding(WRAPPED);
    press(&mut narrow, NARROW, "dgj.");
    press(&mut wide, WIDE, "dgj.");

    assert_eq!(vim_landed(&vim, WRAPPED, "dgj.", NARROW)?, landed(&narrow));
    assert_eq!(vim_landed(&vim, WRAPPED, "dgj.", WIDE)?, landed(&wide));
    assert_ne!(
        landed(&narrow).text,
        landed(&wide).text,
        "the same repeat left the same text in two windows of different widths, so the case \
         cannot tell a repeat measured in rows from one measured in lines"
    );

    Ok(())
}

#[test]
fn a_display_motion_does_not_become_the_change_a_repeat_repeats() -> Result<()> {
    let vim = VimDriver::new()?;
    let mut app = holding(WRAPPED);
    press(&mut app, NARROW, "dwgj.");

    assert_eq!(
        vim_landed(&vim, WRAPPED, "dwgj.", NARROW)?,
        landed(&app),
        "the repeat behind a display motion left the program somewhere other than where vim left it"
    );
    assert_ne!(
        vim_landed(&vim, WRAPPED, "dwgj", NARROW)?,
        vim_landed(&vim, WRAPPED, "dwgj.", NARROW)?,
        "vim answers `dwgj.` where it answers `dwgj`, so the case cannot tell a repeat that ran \
         from one that did nothing at all"
    );

    Ok(())
}

/// The one repeat this editor answers differently from vim, named rather than passed over.
///
/// vim repeats a change made from a visual selection over the same amount of text rather than over
/// the same keys: a charwise selection that crossed a line is repeated over the same number of
/// lines and, in the last of them, over the same number of characters, wherever the column the
/// cursor now stands in falls. What is repeated here is the keys, so the selection is made again
/// from the cursor and carries its column into the line below. The two agree for a selection made
/// within one line and for a linewise one, which are the cases above, and part over this one. The
/// keys typed again by hand are required to leave what the repeat leaves, so what is pinned here
/// is vim's rule for the extent rather than a replay that failed to type what it recorded.
#[test]
fn a_repeat_of_a_charwise_selection_that_crossed_a_line_is_made_again_from_the_keys() -> Result<()>
{
    let vim = VimDriver::new()?;
    let mut repeated = holding(PROSE);
    let mut retyped = holding(PROSE);
    press(&mut repeated, WIDE, "vjdw.");
    press(&mut retyped, WIDE, "vjdwvjd");

    assert_eq!(
        landed(&retyped),
        landed(&repeated),
        "the repeat left something other than what its own keys leave when they are typed again"
    );
    assert_eq!(
        vim_landed(&vim, PROSE, "vjdwvjd", WIDE)?,
        landed(&retyped),
        "the keys the repeat types again are not the keys vim answers this way"
    );
    assert_eq!(
        "econd hird line\nfourth\nfifth\n",
        vim_landed(&vim, PROSE, "vjdw.", WIDE)?.text,
        "vim no longer repeats a charwise selection over the extent of the last one"
    );
    assert_eq!("econd ine\nfourth\nfifth\n", landed(&repeated).text);

    Ok(())
}

/// The one thing a repeat after an undo is not held to vim by, named rather than passed over.
///
/// vim leaves the cursor at the start of what an undo put back and the buffer under this editor
/// leaves it at the end of it, which is a divergence in the undo rather than in the repeat: it is
/// there whether or not a repeat follows, and the cases above type a `0` after the undo so that
/// the repeat they compare is typed from the column both editors agree on. Fixing the undo makes
/// this case fail, which is where it is to be struck off.
#[test]
fn an_undo_leaves_the_cursor_where_vim_does_not() -> Result<()> {
    let vim = VimDriver::new()?;
    let mut app = holding(PROSE);
    press(&mut app, WIDE, "dwu");
    let undone = landed(&app);
    let undone_by_vim = vim_landed(&vim, PROSE, "dwu", WIDE)?;

    assert_eq!(
        undone_by_vim.text, undone.text,
        "the undo put back something other than what vim's undo put back"
    );
    assert_eq!(0, undone_by_vim.column);
    assert_eq!(4, undone.column);

    Ok(())
}

#[test]
fn a_repeat_with_nothing_in_front_of_it_leaves_the_text_alone() {
    let mut app = holding(PROSE);
    press(&mut app, WIDE, "3.");

    assert_eq!(PROSE, app.text().text());
    assert_eq!(VimMode::Normal, app.mode());
    assert_eq!(
        None,
        app.notice(),
        "`.` is bound, and a bound key with no change behind it is not a key nothing answered"
    );
}

#[test]
fn the_editor_records_no_macro_for_a_repeat_to_be_typed_inside() {
    let mut asked = holding(PROSE);
    let answered = asked.press(area(WIDE), KeyEvent::from(KeyCode::Char('@')));

    assert_eq!(Outcome::Continues, answered);
    assert_eq!(
        Some("`@` is bound to nothing"),
        asked.notice(),
        "`@` runs a macro, so a repeat typed into one is a case this file would have to hold"
    );
    assert_eq!(PROSE, asked.text().text());

    let mut recording = holding(PROSE);

    assert_eq!(
        Outcome::Stops,
        recording.press(area(WIDE), KeyEvent::from(KeyCode::Char('q'))),
        "`q` opens a recording rather than ending the program"
    );
}

/// # Returns
///
/// An application holding `text`, with nothing typed at it yet.
fn holding(text: &str) -> App {
    App::new(Buffer::from_text(text))
}

/// Types the keys `keys` names at `app`, in a terminal `columns` columns wide.
fn press(app: &mut App, columns: u16, keys: &str) {
    for key in typed(keys) {
        app.press(area(columns), key);
    }
}

/// # Returns
///
/// What `app` is left holding, in the terms a real vim is compared against.
///
/// # Panics
///
/// Panics if the application is in a mode the harness has no name for.
fn landed(app: &App) -> Landed {
    let cursor = app.cursor();

    Landed {
        text: format!("{}\n", app.text().text()),
        line: cursor.line as u64,
        column: cursor.grapheme as u64,
        mode: match app.mode() {
            VimMode::Normal => Mode::Normal,
            VimMode::Insert => Mode::Insert,
            VimMode::Visual | VimMode::Select => Mode::Visual,
            VimMode::OperationPending => Mode::OperatorPending,
            VimMode::Command => Mode::CommandLine,
            mode => panic!("`{mode:?}` is a mode the harness has no name for"),
        },
    }
}

/// # Returns
///
/// What vim was left holding after `keys` were typed at `text`, laid out in the window an
/// application `columns` columns wide leaves its text, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the window is too small to draw a column of text in.
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn vim_landed(vim: &VimDriver, text: &str, keys: &str, columns: u16) -> Result<Landed> {
    let app = App::new(Buffer::from_text(text));
    let geometry = app
        .geometry(area(columns))
        .ok_or_else(|| anyhow::anyhow!("the window draws no text"))?;
    let state = vim.run_case(&CorpusCase {
        id: keys.to_owned(),
        description: keys.to_owned(),
        buffer: format!("{text}\n"),
        keys: keys.to_owned(),
        viewport_width: u16::try_from(geometry.columns().get())?,
        viewport_height: u16::try_from(geometry.window().height().get())?,
        tags: BTreeSet::new(),
        options: CaseOptions::default(),
    })?;

    Ok(state.into())
}

/// # Returns
///
/// The area a case is typed into, which is `columns` columns of a terminal [`ROWS`] rows tall.
fn area(columns: u16) -> Rect {
    Rect::new(0, 0, columns, ROWS)
}

/// # Returns
///
/// The key events a terminal reports when `keys` is typed at it, in which `<Esc>` names the escape
/// key and every other character stands for itself.
fn typed(keys: &str) -> Vec<KeyEvent> {
    let mut events = Vec::new();
    let mut rest = keys;
    while let Some(character) = rest.chars().next() {
        if let Some(remainder) = rest.strip_prefix("<Esc>") {
            events.push(KeyEvent::from(KeyCode::Esc));
            rest = remainder;
            continue;
        }
        events.push(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        rest = &rest[character.len_utf8()..];
    }

    events
}

impl From<EditorState> for Landed {
    fn from(state: EditorState) -> Self {
        Self {
            text: state.buffer,
            line: state.cursor.line,
            column: state.cursor.column,
            mode: state.mode,
        }
    }
}
