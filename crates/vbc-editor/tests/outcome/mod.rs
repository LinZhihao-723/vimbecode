//! What a replay left an engine holding, in the terms a real vim is compared against.
//!
//! The engine is the authority on the text, the cursor, the mode and the registers, and a file
//! that cross-checks it against vim compares those four and nothing else: where a line is drawn
//! and how wide a grapheme is are the layout engine's, and are held to vim by their own oracles.
//! The record is shared so that two such files cannot drift into comparing different things.

use std::collections::BTreeMap;

use modalkit::env::vim::VimMode;
use vbc_editor::engine::{Engine, Held, Shape};
use vbc_oracle::state::{EditorState, Mode, Register, RegisterType};

/// Everything the vim engine decides, and nothing the layout decides.
#[derive(Debug, Eq, PartialEq)]
pub struct Outcome {
    /// The text being edited.
    pub text: String,

    /// The zero-based line the cursor rests on.
    pub line: u64,

    /// The zero-based byte offset of the cursor within its line.
    pub column: u64,

    /// The mode the engine is in.
    pub mode: Mode,

    /// What every register holding text holds, keyed by the name it is addressed by.
    pub registers: BTreeMap<char, Register>,
}

impl Outcome {
    /// # Returns
    ///
    /// What the engine was left holding.
    ///
    /// # Panics
    ///
    /// Panics if the engine is in a mode the harness has no name for.
    pub fn of(engine: &mut Engine) -> Self {
        let cursor = engine.cursor();
        let mode = mode(engine);

        Self {
            text: engine.text(),
            line: cursor.line as u64,
            column: cursor.column as u64,
            mode,
            registers: engine
                .registers()
                .into_iter()
                .map(|(name, held)| (name, register(held)))
                .collect(),
        }
    }
}

impl From<EditorState> for Outcome {
    fn from(state: EditorState) -> Self {
        Self {
            text: state.buffer,
            line: state.cursor.line,
            column: state.cursor.column,
            mode: state.mode,
            registers: state.registers,
        }
    }
}

/// # Returns
///
/// The mode the engine is in, in the terms the harness compares modes in.
///
/// vim has three visual modes where modalkit has one mode and a shape beside it, and vim reports
/// which of the three it is in. So the shape of the selection is read back too: an engine in
/// visual mode with no selection to read is charwise, which is where one starts.
///
/// # Panics
///
/// Panics if the engine is in a mode the harness has no name for.
fn mode(engine: &mut Engine) -> Mode {
    match engine.mode() {
        VimMode::Normal => Mode::Normal,
        VimMode::Insert => Mode::Insert,
        VimMode::Visual | VimMode::Select => {
            match engine.selection().map(|(_cursor, _anchor, shape)| shape) {
                Some(Shape::Linewise) => Mode::VisualLine,
                Some(Shape::Blockwise) => Mode::VisualBlock,
                Some(Shape::Charwise) | None => Mode::Visual,
            }
        }
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
