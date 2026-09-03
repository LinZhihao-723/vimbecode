//! Cross-checks the keys the vim engine is driven by against the vim they were adopted from.
//!
//! The engine's motions and operators are modalkit's, and the point of this file is that the
//! wiring around them delivers what they compute. Each case types a key sequence at both the
//! engine and a real vim and compares the four dimensions the engine is the authority on: the
//! text, the cursor, the mode and the registers. Where a line is drawn and how wide a grapheme is
//! belong to the layout engine, which is held to vim by its own oracle, so they are not compared
//! here and no case pretends otherwise.
//!
//! The sequences are the ones whose answers are counted in characters rather than in cells --
//! `w`, `b`, `e`, `dw`, `d$`, `x` and `dd` -- because those are the ones modalkit is already right
//! about. They are the control group: a divergence in one of them is a fault in the wiring rather
//! than a grapheme the two engines measure differently.
//!
//! They are typed at two texts and, in one case, past the end of the first line. A cursor is
//! reported as a byte offset where modalkit keeps a character index, and on a text of one-byte
//! characters resting on the first line the two counts agree, so a seam that never converted and a
//! seam that never left the first line would both go unreported. The second text is one whose
//! characters are two bytes wide and one cell wide, which separates the two counts without asking
//! the layout question the shim above this seam is for.
//!
//! A comparison is only worth what it would catch, so the same comparison is run against an engine
//! that was handed no keys at all and against one handed the wrong count, and it is required to
//! report the divergence rather than agree.
//!
//! The events an application loop delivers are checked against the keys they stand for, so the
//! path the application reaches the engine by is held to the one vim was compared against.

use std::collections::BTreeMap;

use vbc_editor::engine::{typed, Engine, Held, Shape};
use vbc_editor::event::{Event, Paste};
use vbc_oracle::state::{Mode, Register, RegisterType};
use vbc_oracle::vim::VimDriver;

/// One cross-check: a starting text, the keys typed at it, and the name it is reported under.
struct Case {
    id: &'static str,
    text: &'static str,
    keys: &'static str,
}

/// The prose the character-counted sequences are typed at.
const PROSE: &str = "the quick brown fox\njumps over it\n";

/// The prose the same sequences are typed at to separate the byte offset a cursor is reported at
/// from the character index modalkit keeps: every accented character is two bytes wide and one
/// cell wide, so the two counts differ without the width of a grapheme coming into it.
const ACCENTED: &str = "héllo wörld\nvoilà là\n";

/// The sequences whose answers modalkit counts in characters, which is what makes them the control
/// group for the wiring rather than a test of how a grapheme is measured.
const CASES: [Case; 13] = [
    Case {
        id: "w",
        text: PROSE,
        keys: "ww",
    },
    Case {
        id: "b",
        text: PROSE,
        keys: "$bb",
    },
    Case {
        id: "e",
        text: PROSE,
        keys: "ee",
    },
    Case {
        id: "dw",
        text: PROSE,
        keys: "wdw",
    },
    Case {
        id: "d$",
        text: PROSE,
        keys: "wd$",
    },
    Case {
        id: "x",
        text: PROSE,
        keys: "xx",
    },
    Case {
        id: "dd",
        text: PROSE,
        keys: "dd",
    },
    Case {
        id: "j then w",
        text: PROSE,
        keys: "jw",
    },
    Case {
        id: "w over two-byte characters",
        text: ACCENTED,
        keys: "ww",
    },
    Case {
        id: "e over two-byte characters",
        text: ACCENTED,
        keys: "ee",
    },
    Case {
        id: "x over two-byte characters",
        text: ACCENTED,
        keys: "lx",
    },
    Case {
        id: "dw over two-byte characters",
        text: ACCENTED,
        keys: "wdw",
    },
    Case {
        id: "j then w over two-byte characters",
        text: ACCENTED,
        keys: "jw",
    },
];

/// What both engines are compared on: everything the vim engine decides, and nothing the layout
/// decides.
#[derive(Debug, Eq, PartialEq)]
struct Outcome {
    text: String,
    line: u64,
    column: u64,
    mode: Mode,
    registers: BTreeMap<char, Register>,
}

#[test]
fn the_engine_ends_every_control_sequence_where_vim_does() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &CASES {
        let mut engine = Engine::new(case.text);
        engine.press_all(case.keys.chars().map(typed))?;

        assert_eq!(
            vim_outcome(&vim, case.text, case.keys)?,
            engine_outcome(&mut engine),
            "`{}` left the engine somewhere other than where vim left it",
            case.id
        );
    }

    Ok(())
}

#[test]
fn an_engine_that_was_handed_no_keys_diverges_from_vim() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &CASES {
        let mut engine = Engine::new(case.text);

        assert_ne!(
            vim_outcome(&vim, case.text, case.keys)?,
            engine_outcome(&mut engine),
            "`{}` agreed with vim without a single key being typed at the engine",
            case.id
        );
    }

    Ok(())
}

#[test]
fn an_engine_handed_all_but_the_last_key_diverges_from_vim() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &CASES {
        let mut engine = Engine::new(case.text);
        let short: Vec<char> = case.keys.chars().collect();
        engine.press_all(short[..short.len() - 1].iter().copied().map(typed))?;

        assert_ne!(
            vim_outcome(&vim, case.text, case.keys)?,
            engine_outcome(&mut engine),
            "`{}` agreed with vim with the key that completes it never typed",
            case.id
        );
    }

    Ok(())
}

#[test]
fn the_events_an_application_loop_delivers_land_where_the_same_keys_would() -> anyhow::Result<()> {
    let mut typed_at = Engine::new(PROSE);
    typed_at.press_all("dwiab".chars().map(typed))?;

    let mut delivered = Engine::new(PROSE);
    delivered.handle(&Event::Key(typed('d')))?;
    delivered.handle(&Event::Resize {
        columns: 80,
        rows: 24,
    })?;
    delivered.handle(&Event::Key(typed('w')))?;
    delivered.handle(&Event::Redraw)?;
    delivered.handle(&Event::Key(typed('i')))?;
    delivered.handle(&Event::Paste(Paste {
        text: "ab".to_owned(),
        dropped_keys: 0,
    }))?;

    assert_ne!(
        engine_outcome(&mut Engine::new(PROSE)),
        engine_outcome(&mut delivered),
        "the events left the engine where an engine handed nothing stands"
    );
    assert_eq!(
        engine_outcome(&mut typed_at),
        engine_outcome(&mut delivered)
    );

    Ok(())
}

#[test]
fn a_key_no_binding_answers_leaves_the_engine_where_it_stood() -> anyhow::Result<()> {
    let mut engine = Engine::new(PROSE);
    let before = engine_outcome(&mut engine);
    engine.press(typed('\u{f8ff}'))?;

    assert_eq!(before, engine_outcome(&mut engine));

    Ok(())
}

/// # Returns
///
/// What the engine was left holding.
fn engine_outcome(engine: &mut Engine) -> Outcome {
    let cursor = engine.cursor();

    Outcome {
        text: engine.text(),
        line: cursor.line as u64,
        column: cursor.column as u64,
        mode: mode(engine),
        registers: engine
            .registers()
            .into_iter()
            .map(|(name, held)| (name, register(held)))
            .collect(),
    }
}

/// # Returns
///
/// What vim was left holding after the same keys were typed at the same text, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run`]'s return values on failure.
fn vim_outcome(vim: &VimDriver, text: &str, keys: &str) -> anyhow::Result<Outcome> {
    let state = vim.run(text, keys)?;

    Ok(Outcome {
        text: state.buffer,
        line: state.cursor.line,
        column: state.cursor.column,
        mode: state.mode,
        registers: state.registers,
    })
}

/// # Returns
///
/// The mode the engine is in, in the terms the harness compares modes in.
fn mode(engine: &Engine) -> Mode {
    use modalkit::env::vim::VimMode;

    match engine.mode() {
        VimMode::Normal => Mode::Normal,
        VimMode::Insert => Mode::Insert,
        VimMode::Visual | VimMode::Select => Mode::Visual,
        VimMode::OperationPending => Mode::OperatorPending,
        VimMode::Command => Mode::CommandLine,
        mode => panic!("`{mode:?}` is a mode the harness has no name for"),
    }
}

/// # Returns
///
/// What a register holds, in the terms the harness compares registers in.
fn register(held: Held) -> Register {
    Register {
        text: held.text,
        register_type: match held.shape {
            Shape::Charwise => RegisterType::Charwise,
            Shape::Linewise => RegisterType::Linewise,
            Shape::Blockwise => RegisterType::Blockwise,
        },
    }
}
