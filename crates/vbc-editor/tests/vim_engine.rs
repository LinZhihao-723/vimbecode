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
//! A comparison is only worth what it would catch, so the same comparison is run against an engine
//! that was handed no keys at all and against one handed the wrong count, and it is required to
//! report the divergence rather than agree.

use std::collections::BTreeMap;

use vbc_editor::engine::{typed, Engine, Held, Shape};
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

/// The sequences whose answers modalkit counts in characters, which is what makes them the control
/// group for the wiring rather than a test of how a grapheme is measured.
const CASES: [Case; 7] = [
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
