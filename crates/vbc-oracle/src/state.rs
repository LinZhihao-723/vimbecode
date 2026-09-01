//! The state schema through which the vimbecode editor and vim are compared.
//!
//! A snapshot captures everything the differential harness observes about an engine, and comparing
//! two snapshots reports each dimension in which they diverge.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The name a register is addressed by, for example `"` for the unnamed register.
pub type RegisterName = char;

/// How a register's text is laid out, which determines how a put reinserts it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RegisterType {
    /// The text is inserted at the cursor, without starting a new line.
    Charwise,

    /// The text is inserted as whole lines, above or below the cursor's line.
    Linewise,

    /// The text is inserted as a rectangular block spanning consecutive lines.
    Blockwise,
}

/// The text held by a register, together with the layout it was yanked or deleted with.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Register {
    /// The register's text, byte-exact.
    pub text: String,

    /// The layout the text is put back with.
    pub register_type: RegisterType,
}

/// The cursor's position, as a zero-based line index and a zero-based byte offset within that line.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Cursor {
    /// The zero-based index of the line the cursor is on.
    pub line: u64,

    /// The zero-based byte offset of the cursor within its line.
    pub column: u64,
}

/// The mode an engine is in.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Mode {
    /// Keys are interpreted as commands.
    Normal,

    /// Keys are inserted into the buffer.
    Insert,

    /// Keys overwrite the text under the cursor.
    Replace,

    /// A characterwise selection is active.
    Visual,

    /// A linewise selection is active.
    VisualLine,

    /// A blockwise selection is active.
    VisualBlock,

    /// An operator is waiting for the motion that completes it.
    OperatorPending,

    /// A command line, search, or filter prompt is being typed.
    CommandLine,
}

/// A snapshot of everything the differential harness compares between two engines.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EditorState {
    /// The buffer's full text, compared byte for byte.
    pub buffer: String,

    /// The cursor's position.
    pub cursor: Cursor,

    /// The mode the engine is in.
    pub mode: Mode,

    /// The registers holding text, keyed by name. A register absent from the map holds nothing.
    pub registers: BTreeMap<RegisterName, Register>,
}

impl EditorState {
    /// Compares two snapshots dimension by dimension.
    ///
    /// # Returns
    ///
    /// Every dimension in which the two snapshots disagree, empty if they are equal.
    #[must_use]
    pub fn diff(&self, other: &Self) -> Vec<Divergence> {
        let mut divergences = Vec::new();

        if self.buffer != other.buffer {
            divergences.push(Divergence::Buffer {
                left: self.buffer.clone(),
                right: other.buffer.clone(),
            });
        }
        if self.cursor != other.cursor {
            divergences.push(Divergence::Cursor {
                left: self.cursor,
                right: other.cursor,
            });
        }
        if self.mode != other.mode {
            divergences.push(Divergence::Mode {
                left: self.mode,
                right: other.mode,
            });
        }

        let names: BTreeSet<RegisterName> = self
            .registers
            .keys()
            .chain(other.registers.keys())
            .copied()
            .collect();
        for name in names {
            let left = self.registers.get(&name);
            let right = other.registers.get(&name);
            if left != right {
                divergences.push(Divergence::Register {
                    name,
                    left: left.cloned(),
                    right: right.cloned(),
                });
            }
        }

        divergences
    }
}

/// A single dimension in which two snapshots disagree, holding both sides' values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Divergence {
    /// The buffers' texts differ.
    Buffer {
        /// The left snapshot's buffer.
        left: String,

        /// The right snapshot's buffer.
        right: String,
    },

    /// The cursors' positions differ.
    Cursor {
        /// The left snapshot's cursor.
        left: Cursor,

        /// The right snapshot's cursor.
        right: Cursor,
    },

    /// The modes differ.
    Mode {
        /// The left snapshot's mode.
        left: Mode,

        /// The right snapshot's mode.
        right: Mode,
    },

    /// One register's content or type differs, or only one snapshot holds it.
    Register {
        /// The register the snapshots disagree on.
        name: RegisterName,

        /// The left snapshot's register, `None` if it holds nothing.
        left: Option<Register>,

        /// The right snapshot's register, `None` if it holds nothing.
        right: Option<Register>,
    },
}

/// An engine the differential harness can snapshot.
pub trait StateSource {
    /// The error reported when a snapshot cannot be taken.
    type Error;

    /// Captures the engine's current state.
    ///
    /// # Returns
    ///
    /// The engine's state on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine cannot report its state.
    fn capture_state(&mut self) -> Result<EditorState, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// # Returns
    ///
    /// A state holding one register of each type, which the tests perturb one dimension at a time.
    fn sample_state() -> EditorState {
        EditorState {
            buffer: "alpha\nbeta\ngamma\n".to_owned(),
            cursor: Cursor { line: 1, column: 2 },
            mode: Mode::Normal,
            registers: BTreeMap::from([
                (
                    '"',
                    Register {
                        text: "beta\n".to_owned(),
                        register_type: RegisterType::Linewise,
                    },
                ),
                (
                    'a',
                    Register {
                        text: "eta".to_owned(),
                        register_type: RegisterType::Charwise,
                    },
                ),
                (
                    '0',
                    Register {
                        text: "al\nbe\n".to_owned(),
                        register_type: RegisterType::Blockwise,
                    },
                ),
            ]),
        }
    }

    #[test]
    fn serialization_round_trips_unchanged() -> anyhow::Result<()> {
        let state = sample_state();
        let restored: EditorState = serde_json::from_str(&serde_json::to_string(&state)?)?;
        assert_eq!(restored, state);
        Ok(())
    }

    #[test]
    fn identical_states_do_not_diverge() {
        assert_eq!(sample_state().diff(&sample_state()), vec![]);
    }

    #[test]
    fn linewise_and_charwise_yanks_of_the_same_text_are_unequal() {
        let text = "beta\n";
        let mut linewise = sample_state();
        linewise.registers.insert(
            '"',
            Register {
                text: text.to_owned(),
                register_type: RegisterType::Linewise,
            },
        );
        let mut charwise = sample_state();
        charwise.registers.insert(
            '"',
            Register {
                text: text.to_owned(),
                register_type: RegisterType::Charwise,
            },
        );

        assert_ne!(charwise, linewise);
        assert_eq!(
            linewise.diff(&charwise),
            vec![Divergence::Register {
                name: '"',
                left: Some(Register {
                    text: text.to_owned(),
                    register_type: RegisterType::Linewise,
                }),
                right: Some(Register {
                    text: text.to_owned(),
                    register_type: RegisterType::Charwise,
                }),
            }]
        );
    }

    #[test]
    fn cursor_only_difference_reports_the_cursor() {
        let left = sample_state();
        let mut right = sample_state();
        right.cursor = Cursor { line: 1, column: 3 };

        assert_eq!(
            left.diff(&right),
            vec![Divergence::Cursor {
                left: Cursor { line: 1, column: 2 },
                right: Cursor { line: 1, column: 3 },
            }]
        );
    }

    #[test]
    fn mode_only_difference_reports_the_mode() {
        let left = sample_state();
        let mut right = sample_state();
        right.mode = Mode::Insert;

        assert_eq!(
            left.diff(&right),
            vec![Divergence::Mode {
                left: Mode::Normal,
                right: Mode::Insert,
            }]
        );
    }

    #[test]
    fn register_only_difference_reports_the_register() {
        let left = sample_state();
        let mut right = sample_state();
        right.registers.remove(&'a');

        assert_eq!(
            left.diff(&right),
            vec![Divergence::Register {
                name: 'a',
                left: Some(Register {
                    text: "eta".to_owned(),
                    register_type: RegisterType::Charwise,
                }),
                right: None,
            }]
        );
    }

    #[test]
    fn buffer_comparison_is_byte_exact() {
        let left = sample_state();
        for buffer in [
            "alpha\nbeta\ngamma",
            "alpha\nbeta \ngamma\n",
            " alpha\nbeta\ngamma\n",
            "alpha\r\nbeta\r\ngamma\r\n",
        ] {
            let mut right = sample_state();
            right.buffer = buffer.to_owned();

            assert_eq!(
                left.diff(&right),
                vec![Divergence::Buffer {
                    left: left.buffer.clone(),
                    right: buffer.to_owned(),
                }]
            );
        }
    }
}
